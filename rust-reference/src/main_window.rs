//! 主窗口工作台（界面地图 S1）：启动即弹出的「深入查看」窗口。
//!
//! 设计对齐 `WinOCR界面地图.md` 的 S1 规范：顶栏（版本 + ●就绪 + 🌙主题 + 关闭到托盘）、
//! 工具栏（自动检测▾→中文▾ 语言选择 + 粘贴翻译 + 历史 + 生词本 + 📷截图）、
//! 视图切换（双栏 / 原图翻译）、双栏逐栏按钮（原文：缩略图+重识+复制；译文：语言▾+🔊+复制+★+⇄）、
//! 底部状态条（●OCR ●翻译 ●代理⚠ ●缓存 本月额度 — ☑静默OCR）。
//!
//! 关闭窗口 = 最小化到托盘（不退出）；托盘双击/Alt+W 再次唤起。
//! 跨线程刷新通过 `RESULT: Mutex<(String,String)>` + `WM_SHOW_RESULT` + `PostMessageW` 由主线程 `SetWindowTextW`。

use crate::config::{Config, OcrEngine};
use std::cell::RefCell;
use std::env;
use std::ffi::c_void;
use std::fs;
use std::mem;
use std::os::windows::ffi::OsStrExt;
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use winapi::shared::minwindef::{LOWORD, LRESULT, WPARAM};
use winapi::shared::windef::{HWND, HGDIOBJ, RECT};
use winapi::um::wingdi::{
    GetStockObject, DEFAULT_GUI_FONT, CreatePen, CreateSolidBrush, DeleteObject, RoundRect,
    SelectObject, SetBkMode, SetTextColor, RGB, TRANSPARENT, NULL_BRUSH, PS_SOLID,
};
use winapi::um::winuser::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, GetDlgCtrlID, GetWindowTextW,
    LoadCursorW, LoadImageW, PostMessageW, RegisterClassW, SendMessageW, SetWindowTextW,
    ShowWindow, TranslateMessage,
    BM_GETCHECK, BM_SETCHECK, BS_AUTOCHECKBOX, BS_OWNERDRAW, BST_CHECKED, BST_UNCHECKED,
    CB_ADDSTRING, CB_ERR, CB_GETCURSEL, CB_GETITEMDATA, CB_GETLBTEXT, CB_SETCURSEL,
    CB_SETITEMDATA, CBS_DROPDOWNLIST, CS_HREDRAW, CS_VREDRAW, DT_CENTER, DT_SINGLELINE,
    DT_VCENTER, DRAWITEMSTRUCT, ES_AUTOVSCROLL, ES_MULTILINE, ES_READONLY, IDC_ARROW,
    IMAGE_BITMAP, LR_CREATEDIBSECTION, LR_LOADFROMFILE, MSG, ODA_DRAWENTIRE, ODS_SELECTED,
    ODT_BUTTON, SS_BITMAP, STM_SETIMAGE, SW_HIDE, SW_RESTORE, SW_SHOW, WM_CLOSE, WM_COMMAND,
    WM_CTLCOLORLISTBOX, WM_CTLCOLORSTATIC, WM_CTLCOLOREDIT, WM_DRAWITEM, WM_ERASEBKGND,
    WM_SETFONT, WM_USER,
    WS_CHILD, WS_OVERLAPPEDWINDOW, WS_VISIBLE, WS_VSCROLL, WNDCLASSW,
};

// 控件 ID
const IDC_TITLE: i32 = 10;
const IDC_STATUS: i32 = 11;
const IDC_BTN_THEME: i32 = 12;
const IDC_BTN_CLOSE: i32 = 13;
const IDC_COMBO_SRC: i32 = 20;
const IDC_COMBO_TGT: i32 = 21;
const IDC_BTN_PASTE: i32 = 22;
const IDC_BTN_HISTORY: i32 = 23;
const IDC_BTN_VOCAB: i32 = 24;
const IDC_BTN_CAP: i32 = 25;
const IDC_BTN_VIEW_DUAL: i32 = 26;
const IDC_BTN_VIEW_IMG: i32 = 27;
const IDC_LBL_SRC: i32 = 30;
const IDC_THUMB: i32 = 31;
const IDC_BTN_REOCR: i32 = 32;
const IDC_BTN_COPY_SRC: i32 = 33;
const IDC_LBL_DST: i32 = 34;
const IDC_BTN_SPEAK: i32 = 35;
const IDC_BTN_COPY_DST: i32 = 36;
const IDC_BTN_FAV: i32 = 37;
const IDC_BTN_REPLACE: i32 = 38;
const IDC_EDIT_SRC: i32 = 200;
const IDC_EDIT_DST: i32 = 201;
const IDC_IMG_BIG: i32 = 40;
const IDC_EDIT_DST_IMG: i32 = 41;
const IDC_BTN_COPY_IMG: i32 = 42;
const IDC_BTN_SAVE_IMG: i32 = 43;
const IDC_LBL_IMG: i32 = 45;
const IDC_CHK_SILENT: i32 = 50;
const IDC_HINT: i32 = 60;

/// 主窗口双栏 / 原图模式刷新（界面地图 S1）
const WM_SHOW_RESULT: u32 = WM_USER + 1;

/// 语言列表（code, label）。src 含 auto；tgt 排除 auto。
const LANGS: &[(&str, &str)] = &[
    ("auto", "自动检测"),
    ("zh", "中文"),
    ("en", "English"),
    ("ja", "日本語"),
    ("ko", "한국어"),
    ("fr", "Français"),
    ("de", "Deutsch"),
    ("ru", "Русский"),
    ("es", "Español"),
    ("pt", "Português"),
    ("it", "Italiano"),
    ("vi", "Tiếng Việt"),
    ("th", "ไทย"),
    ("id", "Indonesia"),
];

/// 主窗口发往主循环的事件
pub enum MainEvent {
    Capture,     // 截图翻译
    Settings,    // 打开设置
    History,     // 历史记录
    Vocab,       // 生词本
    Review,      // 复习闪卡
    SelfCheck,   // 运行诊断
    Hide,        // 隐藏到托盘
    Quit,        // 退出程序
    Paste,       // 粘贴翻译（剪贴板 → 翻译 → 双栏）
    ReOcr,       // 重新识别（用上次截图重新 OCR）
    CaptureSilent, // 主窗口截图翻译（静默模式，仅入历史）
    SetLang,     // 语言选择变化：wparam=src_idx, lparam=tgt_idx
    SetSilent,   // 静默 OCR 勾选：wparam=1 选中
}

/// 主窗口 HWND 地址（usize 是 Send+Sync，规避裸指针非 Sync 问题）。0 表示尚未创建。
static MAIN_HWND: AtomicUsize = AtomicUsize::new(0);
/// 双栏 EDIT 控件 HWND
static EDIT_SRC: AtomicUsize = AtomicUsize::new(0);
static EDIT_DST: AtomicUsize = AtomicUsize::new(0);
/// 最新一次 OCR/翻译结果（跨线程传给主窗口渲染）
static RESULT: parking_lot::Mutex<Option<(String, String)>> = parking_lot::Mutex::new(None);
/// 上一次截图的 BMP 路径 + 尺寸（原图翻译模式显示用）
static LAST_SHOT: parking_lot::Mutex<Option<(String, i32, i32)>> = parking_lot::Mutex::new(None);
/// 上一次截图的 PNG 字节（「重新识别」复用，避免再次截图）
static LAST_PNG: parking_lot::Mutex<Option<Vec<u8>>> = parking_lot::Mutex::new(None);
/// 视图：0=双栏，1=原图翻译
static VIEW: AtomicUsize = AtomicUsize::new(0);
/// 主题：0=浅色，1=深色（控件完整深色重绘为 🟡 已知项）
static THEME: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    static TX: RefCell<Option<Sender<MainEvent>>> = RefCell::new(None);
}

use std::sync::mpsc::{channel, Receiver, Sender};

fn to_wide(s: &str) -> Vec<u16> {
    std::ffi::OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

unsafe fn create_static(
    parent: HWND,
    text: &str,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    id: i32,
    style: u32,
    hfont: *mut c_void,
) -> HWND {
    let t = to_wide(text);
    let hwnd = CreateWindowExW(
        0,
        to_wide("STATIC").as_ptr(),
        t.as_ptr(),
        WS_CHILD | WS_VISIBLE | style,
        x,
        y,
        w,
        h,
        parent,
        id as winapi::shared::windef::HMENU,
        ptr::null_mut(),
        ptr::null_mut(),
    );
    if !hfont.is_null() {
        SendMessageW(hwnd, WM_SETFONT, hfont as WPARAM, 1);
    }
    hwnd
}

unsafe fn create_button(
    parent: HWND,
    text: &str,
    id: i32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    hfont: *mut c_void,
) -> HWND {
    let t = to_wide(text);
    let hwnd = CreateWindowExW(
        0,
        to_wide("BUTTON").as_ptr(),
        t.as_ptr(),
        WS_CHILD | WS_VISIBLE | BS_OWNERDRAW,
        x,
        y,
        w,
        h,
        parent,
        id as winapi::shared::windef::HMENU,
        ptr::null_mut(),
        ptr::null_mut(),
    );
    if !hfont.is_null() {
        SendMessageW(hwnd, WM_SETFONT, hfont as WPARAM, 1);
    }
    hwnd
}

unsafe fn create_edit(
    parent: HWND,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    id: i32,
    readonly: bool,
    hfont: *mut c_void,
) -> HWND {
    let style = if readonly {
        ES_MULTILINE | ES_AUTOVSCROLL | WS_VSCROLL | ES_READONLY
    } else {
        ES_MULTILINE | ES_AUTOVSCROLL | WS_VSCROLL
    };
    let hwnd = CreateWindowExW(
        0,
        to_wide("EDIT").as_ptr(),
        ptr::null(),
        WS_CHILD | WS_VISIBLE | style,
        x,
        y,
        w,
        h,
        parent,
        id as winapi::shared::windef::HMENU,
        ptr::null_mut(),
        ptr::null_mut(),
    );
    if !hfont.is_null() {
        SendMessageW(hwnd, WM_SETFONT, hfont as WPARAM, 1);
    }
    hwnd
}

unsafe fn create_combo(
    parent: HWND,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    id: i32,
    hfont: *mut c_void,
) -> HWND {
    let hwnd = CreateWindowExW(
        0,
        to_wide("COMBOBOX").as_ptr(),
        ptr::null(),
        WS_CHILD | WS_VISIBLE | CBS_DROPDOWNLIST,
        x,
        y,
        w,
        h,
        parent,
        id as winapi::shared::windef::HMENU,
        ptr::null_mut(),
        ptr::null_mut(),
    );
    if !hfont.is_null() {
        SendMessageW(hwnd, WM_SETFONT, hfont as WPARAM, 1);
    }
    hwnd
}

/// 把语言列表填入下拉框（src 含 auto；tgt 排除 auto 由调用方决定从哪个切片开始）
unsafe fn fill_combo(hwnd: HWND, langs: &[(&str, &str)], select_code: &str) {
    for (i, (code, label)) in langs.iter().enumerate() {
        SendMessageW(hwnd, CB_ADDSTRING, 0, to_wide(label).as_ptr() as isize);
        if *code == select_code {
            SendMessageW(hwnd, CB_SETCURSEL, i as usize, 0);
        }
    }
}

/// 读取下拉框当前选中的语言 code（按显示文本反查 LANGS）
unsafe fn read_combo_code(hwnd: HWND) -> String {
    let idx = SendMessageW(hwnd, CB_GETCURSEL, 0, 0) as usize;
    if idx == CB_ERR as usize {
        return "auto".to_string();
    }
    let mut buf = [0u16; 64];
    let n = SendMessageW(hwnd, CB_GETLBTEXT, idx, buf.as_mut_ptr() as isize);
    if n > 0 {
        let s = String::from_utf16_lossy(&buf[..n as usize]);
        for (code, label) in LANGS {
            if *label == s {
                return code.to_string();
            }
        }
    }
    "auto".to_string()
}

/// 把截图的 BMP 加载到指定 STATIC 控件（SS_BITMAP）
unsafe fn load_image_to(hwnd: HWND, path: &str) {
    let wpath = to_wide(path);
    let hbmp = LoadImageW(
        ptr::null_mut(),
        wpath.as_ptr(),
        IMAGE_BITMAP,
        0,
        0,
        LR_LOADFROMFILE | LR_CREATEDIBSECTION,
    );
    if !hbmp.is_null() {
        SendMessageW(
            hwnd,
            winapi::um::winuser::STM_SETIMAGE,
            IMAGE_BITMAP as usize,
            hbmp as isize,
        );
    }
}

/// 依据 VIEW 全局切换 双栏 / 原图翻译 控件可见性
unsafe fn apply_view(hwnd: HWND) {
    let dual = VIEW.load(Ordering::SeqCst) == 0;
    let show = |id: i32, vis: bool| {
        let h = GetDlgItem(hwnd, id);
        if !h.is_null() {
            ShowWindow(h, if vis { SW_SHOW } else { SW_HIDE });
        }
    };
    // 双栏控件
    show(IDC_LBL_SRC, dual);
    show(IDC_THUMB, dual);
    show(IDC_EDIT_SRC, dual);
    show(IDC_BTN_REOCR, dual);
    show(IDC_BTN_COPY_SRC, dual);
    show(IDC_LBL_DST, dual);
    show(IDC_BTN_SPEAK, dual);
    show(IDC_BTN_COPY_DST, dual);
    show(IDC_BTN_FAV, dual);
    show(IDC_BTN_REPLACE, dual);
    show(IDC_EDIT_DST, dual);
    // 原图控件
    show(IDC_LBL_IMG, !dual);
    show(IDC_IMG_BIG, !dual);
    show(IDC_EDIT_DST_IMG, !dual);
    show(IDC_BTN_COPY_IMG, !dual);
    show(IDC_BTN_SAVE_IMG, !dual);

    if !dual {
        if let Some((path, _, _)) = LAST_SHOT.lock().as_ref() {
            let big = GetDlgItem(hwnd, IDC_IMG_BIG);
            if !big.is_null() {
                load_image_to(big, path);
            }
        }
    }
}

#[allow(non_snake_case)]
unsafe fn GetDlgItem(hwnd: HWND, id: i32) -> HWND {
    winapi::um::winuser::GetDlgItem(hwnd, id)
}

unsafe extern "system" fn main_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: isize,
) -> LRESULT {
    match msg {
        WM_ERASEBKGND => {
            // 上部深色顶栏带（0..76），其余浅灰背景
            let hdc = wparam as winapi::shared::windef::HDC;
            let mut rc = mem::zeroed::<RECT>();
            winapi::um::winuser::GetClientRect(hwnd, &mut rc);
            let light = CreateSolidBrush(RGB(243, 244, 248));
            let dark = CreateSolidBrush(RGB(30, 34, 45));
            winapi::um::winuser::FillRect(hdc, &rc, light);
            let band = RECT { left: 0, top: 0, right: rc.right, bottom: 76 };
            winapi::um::winuser::FillRect(hdc, &band, dark);
            DeleteObject(light as HGDIOBJ);
            DeleteObject(dark as HGDIOBJ);
            1
        }
        WM_CTLCOLORSTATIC => {
            let hdc = lparam as winapi::shared::windef::HDC;
            let ctrl = wparam as winapi::shared::windef::HWND;
            let id = GetDlgCtrlID(ctrl);
            if id == IDC_TITLE || id == IDC_STATUS {
                // 顶栏文字：白字透明背景（露出深色带）
                SetBkMode(hdc, TRANSPARENT as i32);
                SetTextColor(hdc, RGB(240, 242, 247));
                return GetStockObject(NULL_BRUSH as i32) as LRESULT;
            }
            SetBkMode(hdc, TRANSPARENT as i32);
            SetTextColor(hdc, RGB(55, 63, 81));
            return GetStockObject(NULL_BRUSH as i32) as LRESULT;
        }
        WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX => {
            let hdc = lparam as winapi::shared::windef::HDC;
            SetBkMode(hdc, TRANSPARENT as i32);
            SetTextColor(hdc, RGB(31, 36, 48));
            return GetStockObject(6 /* WHITE_BRUSH */) as LRESULT;
        }
        WM_DRAWITEM => {
            let dis = lparam as *const DRAWITEMSTRUCT;
            if (*dis).CtlType != ODT_BUTTON {
                return 0;
            }
            let id = (*dis).CtlID as i32; // ownerdraw 按钮：CtlID 即控件 ID
            let bhwnd = (*dis).hwndItem;
            let hdc = (*dis).hDC;
            let mut rc = (*dis).rcItem;
            let pressed = ((*dis).itemState & ODS_SELECTED) != 0;

            // 配色：主操作蓝 / 顶栏深灰 / 视图切换高亮 / 默认白
            let (fill, text, border): (u32, u32, u32) = if id == IDC_BTN_CAP {
                (RGB(37, 99, 235), RGB(255, 255, 255), RGB(29, 78, 216))
            } else if id == IDC_BTN_THEME || id == IDC_BTN_CLOSE {
                (RGB(58, 64, 80), RGB(235, 238, 244), RGB(78, 86, 104))
            } else if id == IDC_BTN_VIEW_DUAL || id == IDC_BTN_VIEW_IMG {
                let active = (id == IDC_BTN_VIEW_DUAL && VIEW.load(Ordering::SeqCst) == 0)
                    || (id == IDC_BTN_VIEW_IMG && VIEW.load(Ordering::SeqCst) == 1);
                if active {
                    (RGB(219, 234, 254), RGB(29, 78, 216), RGB(147, 197, 253))
                } else {
                    (RGB(255, 255, 255), RGB(55, 63, 81), RGB(209, 213, 219))
                }
            } else {
                (RGB(255, 255, 255), RGB(31, 36, 48), RGB(209, 213, 219))
            };
            let fill = if pressed {
                match fill {
                    c if c == RGB(37, 99, 235) => RGB(29, 78, 216),
                    _ => RGB(229, 231, 235),
                }
            } else {
                fill
            };

            let brush = CreateSolidBrush(fill);
            let pen = CreatePen(PS_SOLID as i32, 1, border);
            let old_brush = SelectObject(hdc, brush as HGDIOBJ);
            let old_pen = SelectObject(hdc, pen as HGDIOBJ);
            RoundRect(hdc, rc.left, rc.top, rc.right, rc.bottom, 8, 8);
            SelectObject(hdc, old_brush);
            SelectObject(hdc, old_pen);
            DeleteObject(brush as HGDIOBJ);
            DeleteObject(pen as HGDIOBJ);

            // 文本
            let mut buf = [0u16; 128];
            let n = GetWindowTextW(bhwnd, buf.as_mut_ptr(), 128);
            SetBkMode(hdc, TRANSPARENT as i32);
            SetTextColor(hdc, text);
            rc.left += 4;
            rc.right -= 4;
            winapi::um::winuser::DrawTextW(
                hdc,
                buf.as_ptr(),
                n,
                &mut rc,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE,
            );
            1
        }
        WM_COMMAND => {
            let id = LOWORD(wparam as u32) as i32;
            // 隐藏按钮：本地直接隐藏窗口，不向主循环发事件
            if id == IDC_BTN_CLOSE {
                ShowWindow(hwnd, SW_HIDE);
                return 0;
            }
            // 语言下拉变化
            if id == IDC_COMBO_SRC || id == IDC_COMBO_TGT {
                let src = read_combo_code(GetDlgItem(hwnd, IDC_COMBO_SRC));
                let tgt = read_combo_code(GetDlgItem(hwnd, IDC_COMBO_TGT));
                let _ = TX.with(|t| {
                    t.borrow()
                        .as_ref()
                        .map(|tx| tx.send(MainEvent::SetLang))
                });
                // 暂存到全局供主循环读取（通过 WM 不便带字符串，改用全局）
                pending_lang(src, tgt);
                return 0;
            }
            // 视图切换
            if id == IDC_BTN_VIEW_DUAL {
                VIEW.store(0, Ordering::SeqCst);
                apply_view(hwnd);
                return 0;
            }
            if id == IDC_BTN_VIEW_IMG {
                VIEW.store(1, Ordering::SeqCst);
                apply_view(hwnd);
                return 0;
            }
            // 静默 OCR 勾选
            if id == IDC_CHK_SILENT {
                let checked = (SendMessageW(
                    GetDlgItem(hwnd, IDC_CHK_SILENT),
                    winapi::um::winuser::BM_GETCHECK,
                    0,
                    0,
                ) as usize)
                    == winapi::um::winuser::BST_CHECKED as usize;
                let _ = TX.with(|t| {
                    t.borrow()
                        .as_ref()
                        .map(|tx| tx.send(MainEvent::SetSilent))
                });
                pending_silent(checked);
                return 0;
            }
            // 主题切换
            if id == IDC_BTN_THEME {
                let next = if THEME.load(Ordering::SeqCst) == 0 { 1 } else { 0 };
                THEME.store(next, Ordering::SeqCst);
                let label = if next == 1 { "☀ 浅色" } else { "🌙 深色" };
                SetWindowTextW(GetDlgItem(hwnd, IDC_BTN_THEME), to_wide(label).as_ptr());
                crate::window::show_toast(
                    if next == 1 { "已切换深色主题（部分控件待重绘）" } else { "已切换浅色主题" },
                    crate::window::ToastLevel::Info,
                );
                return 0;
            }
            // 列内动作（直接读本窗口控件，无需 round-trip）
            match id {
                IDC_BTN_PASTE => {
                    let _ = TX.with(|t| t.borrow().as_ref().map(|tx| tx.send(MainEvent::Paste)));
                }
                IDC_BTN_REOCR => {
                    let _ = TX.with(|t| t.borrow().as_ref().map(|tx| tx.send(MainEvent::ReOcr)));
                }
                IDC_BTN_COPY_SRC => copy_edit(EDIT_SRC.load(Ordering::SeqCst) as HWND),
                IDC_BTN_COPY_DST | IDC_BTN_COPY_IMG => {
                    copy_edit(EDIT_DST.load(Ordering::SeqCst) as HWND)
                }
                IDC_BTN_SPEAK => speak_edit(EDIT_DST.load(Ordering::SeqCst) as HWND),
                IDC_BTN_REPLACE => {
                    // 桌面端替换原文降级为复制译文（真正替换需扩展侧 content script）
                    copy_edit(EDIT_DST.load(Ordering::SeqCst) as HWND);
                    crate::window::show_toast(
                        "已复制译文（桌面替换原文需扩展侧支持）",
                        crate::window::ToastLevel::Info,
                    );
                }
                IDC_BTN_FAV => {
                    let src = get_edit(EDIT_SRC.load(Ordering::SeqCst) as HWND);
                    let dst = get_edit(EDIT_DST.load(Ordering::SeqCst) as HWND);
                    if !src.is_empty() && !dst.is_empty() {
                        crate::history::add_vocab(&src, &dst);
                        crate::window::show_toast("已加入生词本", crate::window::ToastLevel::Info);
                    }
                }
                IDC_BTN_SAVE_IMG => save_last_shot(),
                IDC_BTN_CAP => {
                    let chk = GetDlgItem(hwnd, IDC_CHK_SILENT);
                    let silent = !chk.is_null()
                        && (SendMessageW(chk, BM_GETCHECK, 0, 0) as usize)
                            == BST_CHECKED as usize;
                    let ev = if silent {
                        MainEvent::CaptureSilent
                    } else {
                        MainEvent::Capture
                    };
                    let _ = TX.with(|t| t.borrow().as_ref().map(|tx| tx.send(ev)));
                }
                IDC_BTN_HISTORY => {
                    let _ = TX.with(|t| t.borrow().as_ref().map(|tx| tx.send(MainEvent::History)));
                }
                IDC_BTN_VOCAB => {
                    let _ = TX.with(|t| t.borrow().as_ref().map(|tx| tx.send(MainEvent::Vocab)));
                }
                _ => {}
            }
            0
        }
        WM_SHOW_RESULT => {
            if let Some((o, t)) = RESULT.lock().clone() {
                let src = EDIT_SRC.load(Ordering::SeqCst) as HWND;
                let dst = EDIT_DST.load(Ordering::SeqCst) as HWND;
                if !src.is_null() {
                    SetWindowTextW(src, to_wide(&o).as_ptr());
                }
                if !dst.is_null() {
                    SetWindowTextW(dst, to_wide(&t).as_ptr());
                }
                let dst_img = GetDlgItem(hwnd, IDC_EDIT_DST_IMG);
                if !dst_img.is_null() {
                    SetWindowTextW(dst_img, to_wide(&t).as_ptr());
                }
                // 缩略图
                if let Some((path, _, _)) = LAST_SHOT.lock().as_ref() {
                    let thumb = GetDlgItem(hwnd, IDC_THUMB);
                    if !thumb.is_null() {
                        load_image_to(thumb, path);
                    }
                }
            }
            0
        }
        WM_CLOSE => {
            ShowWindow(hwnd, SW_HIDE);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn get_edit(hwnd: HWND) -> String {
    if hwnd.is_null() {
        return String::new();
    }
    let len = SendMessageW(hwnd, winapi::um::winuser::WM_GETTEXTLENGTH, 0, 0) as usize;
    if len == 0 {
        return String::new();
    }
    let mut buf = vec![0u16; len + 1];
    SendMessageW(hwnd, winapi::um::winuser::WM_GETTEXT, (len + 1) as usize, buf.as_mut_ptr() as isize);
    String::from_utf16_lossy(&buf[..len]).trim().to_string()
}

unsafe fn copy_edit(hwnd: HWND) {
    let text = get_edit(hwnd);
    if !text.is_empty() {
        crate::window::copy_text(&text);
        crate::window::show_toast("已复制到剪贴板", crate::window::ToastLevel::Info);
    }
}

unsafe fn speak_edit(hwnd: HWND) {
    let text = get_edit(hwnd);
    if !text.is_empty() {
        crate::window::speak_text(&text);
        crate::window::show_toast("正在朗读…", crate::window::ToastLevel::Info);
    }
}

/// 把上一次截图另存为图片（原图模式「保存图片」）
unsafe fn save_last_shot() {
    let path = match LAST_SHOT.lock().as_ref() {
        Some((p, _, _)) => p.clone(),
        None => {
            crate::window::show_toast("暂无截图可保存", crate::window::ToastLevel::Warn);
            return;
        }
    };
    let home = env::var("USERPROFILE").unwrap_or_else(|_| ".".to_string());
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let dest = format!("{}\\Pictures\\WinOCR_shot_{}.bmp", home, ts);
    match fs::copy(&path, &dest) {
        Ok(_) => crate::window::show_toast(
            &format!("已保存图片：{}", dest),
            crate::window::ToastLevel::Info,
        ),
        Err(e) => crate::window::show_toast(
            &format!("保存失败：{}", e),
            crate::window::ToastLevel::Error,
        ),
    }
}

/// 语言选择/静默勾选的临时存储（主循环消费前暂存）
static PENDING_LANG: parking_lot::Mutex<Option<(String, String)>> = parking_lot::Mutex::new(None);
static PENDING_SILENT: parking_lot::Mutex<Option<bool>> = parking_lot::Mutex::new(None);

fn pending_lang(src: String, tgt: String) {
    *PENDING_LANG.lock() = Some((src, tgt));
}
fn pending_silent(v: bool) {
    *PENDING_SILENT.lock() = Some(v);
}
/// 主循环读取并清空待处理语言/静默状态
pub fn take_pending_lang() -> Option<(String, String)> {
    PENDING_LANG.lock().take()
}
pub fn take_pending_silent() -> Option<bool> {
    PENDING_SILENT.lock().take()
}

/// 启动主窗口线程，返回事件 `Receiver` 给主循环消费。
/// 窗口在创建后立即显示（SW_SHOW），作为应用启动的第一个可见界面。
pub fn spawn(config: Arc<Mutex<Config>>) -> Receiver<MainEvent> {
    let (tx, rx) = channel::<MainEvent>();
    let cfg = config.lock().unwrap().clone();
    std::thread::spawn(move || {
        TX.with(|t| *t.borrow_mut() = Some(tx.clone()));
        unsafe {
            let cls = to_wide("WinOCRMainWndCls");
            let wc = WNDCLASSW {
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(main_proc),
                hInstance: ptr::null_mut(),
                hCursor: LoadCursorW(ptr::null_mut(), IDC_ARROW),
                hbrBackground: GetStockObject(6 /* COLOR_WINDOW+1 */) as *mut _,
                lpszClassName: cls.as_ptr(),
                lpszMenuName: ptr::null_mut(),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hIcon: ptr::null_mut(),
            };
            RegisterClassW(&wc);

            let title = format!("WinOCR-Html v{}  ●就绪", crate::APP_VERSION);
            let hwnd = CreateWindowExW(
                0,
                cls.as_ptr(),
                to_wide(&title).as_ptr(),
                WS_OVERLAPPEDWINDOW & !0x0008_0000 & !0x0004_0000, // 去 MAXIMIZEBOX/MINIMIZEBOX
                320,
                80,
                880,
                720,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
            );
            if hwnd.is_null() {
                return;
            }
            MAIN_HWND.store(hwnd as usize, Ordering::SeqCst);

            let hfont = GetStockObject(DEFAULT_GUI_FONT as i32) as *mut c_void;

            // 顶栏
            create_static(hwnd, &title, 16, 14, 660, 24, IDC_TITLE, 0, hfont);
            create_button(hwnd, "🌙 深色", IDC_BTN_THEME, 712, 12, 72, 28, hfont);
            create_button(hwnd, "托盘", IDC_BTN_CLOSE, 790, 12, 72, 28, hfont);

            // 底部状态条
            let ocr_state = match cfg.ocr_engine {
                OcrEngine::Local => "本地 OCR（PP-OCRv6）",
                OcrEngine::Windows => "系统 OCR",
                OcrEngine::Sf => "SiliconFlow OCR",
            };
            let status = format!(
                "●OCR {}  ●翻译 {}  ●代理{}  ●缓存 {}（{}条）  本月额度 —",
                ocr_state,
                cfg.translate_engine,
                if cfg.proxy_url.is_empty() { "⚠未配置" } else { "✓" },
                if cfg.ocr_cache_enabled { "✓" } else { "✗" },
                cfg.ocr_cache_max,
            );
            create_static(hwnd, &status, 16, 44, 720, 40, IDC_STATUS, 0, hfont);

            // 工具栏：语言选择 + 粘贴翻译 + 历史 + 生词本 + 截图
            let combo_src = create_combo(hwnd, 16, 92, 130, 240, IDC_COMBO_SRC, hfont);
            fill_combo(combo_src, LANGS, &cfg.src_lang);
            create_static(hwnd, "→", 150, 92, 16, 24, IDC_LBL_SRC + 99, 0, hfont);
            let combo_tgt = create_combo(hwnd, 170, 92, 130, 240, IDC_COMBO_TGT, hfont);
            fill_combo(combo_tgt, &LANGS[1..], &cfg.tgt_lang);
            create_button(hwnd, "粘贴翻译", IDC_BTN_PASTE, 312, 92, 110, 28, hfont);
            create_button(hwnd, "历史", IDC_BTN_HISTORY, 430, 92, 80, 28, hfont);
            create_button(hwnd, "生词本", IDC_BTN_VOCAB, 518, 92, 80, 28, hfont);
            create_button(hwnd, "📷 截图翻译", IDC_BTN_CAP, 606, 92, 150, 28, hfont);

            // 视图切换
            create_button(hwnd, "双栏", IDC_BTN_VIEW_DUAL, 16, 128, 100, 28, hfont);
            create_button(hwnd, "原图翻译", IDC_BTN_VIEW_IMG, 124, 128, 120, 28, hfont);

            // 双栏：左=原文，右=译文
            create_static(hwnd, "OCR 原文", 16, 166, 400, 20, IDC_LBL_SRC, 0, hfont);
            create_static(hwnd, "🖼", 16, 190, 200, 120, IDC_THUMB, SS_BITMAP, hfont);
            let e_src = create_edit(hwnd, 16, 318, 400, 200, IDC_EDIT_SRC, false, hfont);
            create_button(hwnd, "重新识别", IDC_BTN_REOCR, 16, 524, 190, 30, hfont);
            create_button(hwnd, "复制", IDC_BTN_COPY_SRC, 214, 524, 190, 30, hfont);
            create_static(hwnd, "翻译", 440, 166, 400, 20, IDC_LBL_DST, 0, hfont);
            create_button(hwnd, "🔊", IDC_BTN_SPEAK, 440, 190, 60, 28, hfont);
            create_button(hwnd, "复制", IDC_BTN_COPY_DST, 508, 190, 80, 28, hfont);
            create_button(hwnd, "★", IDC_BTN_FAV, 596, 190, 50, 28, hfont);
            create_button(hwnd, "⇄", IDC_BTN_REPLACE, 654, 190, 60, 28, hfont);
            let e_dst = create_edit(hwnd, 440, 226, 400, 292, IDC_EDIT_DST, true, hfont);

            // 原图模式控件（初始隐藏）
            create_static(hwnd, "原图翻译（截图原图 + 译文）", 16, 150, 400, 20, IDC_LBL_IMG, 0, hfont);
            create_static(hwnd, "", 16, 166, 400, 360, IDC_IMG_BIG, SS_BITMAP, hfont);
            let e_dst_img = create_edit(hwnd, 440, 150, 400, 376, IDC_EDIT_DST_IMG, true, hfont);
            create_button(hwnd, "复制译文", IDC_BTN_COPY_IMG, 440, 540, 190, 30, hfont);
            create_button(hwnd, "保存图片", IDC_BTN_SAVE_IMG, 638, 540, 190, 30, hfont);

            EDIT_SRC.store(e_src as usize, Ordering::SeqCst);
            EDIT_DST.store(e_dst as usize, Ordering::SeqCst);
            let _ = e_dst_img;

            // 底部：静默 OCR 勾选 + 提示
            let chk = CreateWindowExW(
                0,
                to_wide("BUTTON").as_ptr(),
                to_wide("☐ 静默OCR（截图仅入历史）").as_ptr(),
                WS_CHILD | WS_VISIBLE | BS_AUTOCHECKBOX,
                16,
                600,
                260,
                24,
                hwnd,
                IDC_CHK_SILENT as winapi::shared::windef::HMENU,
                ptr::null_mut(),
                ptr::null_mut(),
            );
            if !chk.is_null() {
                SendMessageW(
                    chk,
                    BM_SETCHECK,
                    if cfg.silent_mode {
                        BST_CHECKED
                    } else {
                        BST_UNCHECKED
                    } as usize,
                    0,
                );
            }
            create_static(
                hwnd,
                "关闭窗口将最小化到托盘，不退出；Alt+S 截图翻译 / Alt+Q 静默OCR / Alt+W 主窗口 / Alt+R 朗读 / Alt+C 复制。",
                16,
                636,
                840,
                60,
                IDC_HINT,
                0,
                hfont,
            );

            apply_view(hwnd);
            ShowWindow(hwnd, SW_SHOW);

            let mut msg: MSG = mem::zeroed();
            while GetMessageW(&mut msg, hwnd, 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    });
    rx
}

/// 从托盘/其他位置唤起已最小化的主窗口
pub fn show_main_window() {
    let hwnd = MAIN_HWND.load(Ordering::SeqCst) as HWND;
    if !hwnd.is_null() {
        unsafe {
            ShowWindow(hwnd, SW_RESTORE);
            winapi::um::winuser::SetForegroundWindow(hwnd);
        }
    }
}

/// 隐藏主窗口到托盘
#[allow(dead_code)]
pub fn hide_main_window() {
    let hwnd = MAIN_HWND.load(Ordering::SeqCst) as HWND;
    if !hwnd.is_null() {
        unsafe {
            ShowWindow(hwnd, SW_HIDE);
        }
    }
}

/// 主循环把一次 OCR/翻译结果推送到主窗口双栏（跨线程安全：存全局 + PostMessage）
pub fn show_result(original: &str, translation: &str) {
    *RESULT.lock() = Some((original.to_string(), translation.to_string()));
    let hwnd = MAIN_HWND.load(Ordering::SeqCst) as HWND;
    if !hwnd.is_null() {
        unsafe {
            PostMessageW(hwnd, WM_SHOW_RESULT, 0, 0);
        }
    }
}

/// 记录上一次截图的 BMP 路径与尺寸（原图模式显示用）
pub fn set_last_screenshot(path: String, w: i32, h: i32) {
    *LAST_SHOT.lock() = Some((path, w, h));
    let hwnd = MAIN_HWND.load(Ordering::SeqCst) as HWND;
    if !hwnd.is_null() {
        unsafe {
            PostMessageW(hwnd, WM_SHOW_RESULT, 0, 0);
        }
    }
}

/// 取最新一次 OCR/翻译结果（供 S12 全局热键 Alt+R/C/V 使用）
pub fn latest_result() -> Option<(String, String)> {
    RESULT.lock().clone()
}

/// 记录上一次截图的 PNG 字节（「重新识别」复用）
pub fn set_last_png(data: Vec<u8>) {
    *LAST_PNG.lock() = Some(data);
}

/// 取出上一次截图的 PNG 字节（消费式，避免常驻大内存）
pub fn take_last_png() -> Option<Vec<u8>> {
    LAST_PNG.lock().take()
}
