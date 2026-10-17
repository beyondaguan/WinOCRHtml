//! 设置窗口（Win32 控件版，P1-2 / P1-5 / P1-6）
//!
//! 分区：通用 / OCR / 模型 / 隐私 / 快捷键 / 关于 / 外观 / 高级。
//! - P1-2：启用设置窗口并扩为分区（源目标语言、OCR 引擎/精度、翻译引擎、SF 模型与密钥）。
//! - M12：拆分为 **OCR / 模型 / 隐私** 三个独立分区，对接 M4 OCR、M7 四引擎、M11 术语与 §15 隐私红线。
//! - P1-5：快捷键「按下即录入」+ 即时显示 + 冲突回退提示 + 「测试一下」。
//! - P1-6：关于分区「运行诊断」按钮，调用 selfCheck 并弹脱敏摘要、写日志目录。
//!
//! 在独立线程调用（主循环 spawn），避免阻塞常驻消息泵。保存时写回 Arc<Mutex<Config>> 并落盘。

use crate::config::{Config, OcrEngine, OcrTier};
use crate::selfcheck;
use std::cell::RefCell;
use std::mem;
use std::path::Path;
use std::ptr;
use std::sync::{Arc, Mutex};
use winapi::shared::minwindef::{LOWORD, LPARAM, LRESULT, UINT, WPARAM};
use winapi::shared::windef::HWND;
use winapi::um::wingdi::{GetStockObject, DEFAULT_GUI_FONT};
use winapi::um::winuser::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetAsyncKeyState, GetMessageW,
    GetScrollInfo, GetWindowLongPtrW, GetWindowTextLengthW, GetWindowTextW, LoadCursorW,
    MessageBoxW, RegisterClassW, ScrollWindowEx, SendMessageW, SetFocus, SetScrollInfo,
    SetWindowLongPtrW, SetWindowPos, SetWindowTextW, ShowWindow, TranslateMessage, BS_CHECKBOX, BST_CHECKED,
    BM_GETCHECK, BM_SETCHECK, CB_ADDSTRING, CB_GETCURSEL, CB_SETCURSEL, CBS_DROPDOWNLIST, CS_HREDRAW,
    CS_VREDRAW, ES_PASSWORD, GWLP_WNDPROC, IDC_ARROW, IDC_ARROW as _, MSG, SB_BOTTOM, SB_LINEDOWN,
    SB_LINEUP, SB_PAGEDOWN, SB_PAGEUP, SB_THUMBPOSITION, SB_THUMBTRACK, SB_TOP, SB_VERT, SCROLLINFO,
    SIF_PAGE, SIF_POS, SIF_RANGE, SW_ERASE, SW_INVALIDATE, SW_HIDE, SW_SHOW, SWP_NOACTIVATE,
    SWP_NOZORDER, VK_CONTROL, VK_F1, VK_F12, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT, WM_CLOSE,
    WM_COMMAND, WM_DESTROY, WM_KEYDOWN, WM_SETFONT, WM_SETTEXT, WM_VSCROLL, WS_BORDER, WS_CHILD,
    WS_EX_CLIENTEDGE, WS_OVERLAPPEDWINDOW, WS_VISIBLE, WS_VSCROLL, WNDCLASSW,
};

// 控件 ID
const IDC_COMBO_SRC: i32 = 101;
const IDC_COMBO_TGT: i32 = 102;
const IDC_COMBO_OCR: i32 = 103;
const IDC_COMBO_TIER: i32 = 104;
const IDC_COMBO_TENGINE: i32 = 105;
const IDC_EDIT_SFMODEL: i32 = 106;
const IDC_EDIT_SFKEY: i32 = 107;
const IDC_REC_CAP: i32 = 201;
const IDC_REC_SET: i32 = 202;
const IDC_REC_QUIT: i32 = 203;
const IDC_REC_PAUSE: i32 = 204;
const IDC_TXT_CAP: i32 = 211;
const IDC_TXT_SET: i32 = 212;
const IDC_TXT_QUIT: i32 = 213;
const IDC_TXT_PAUSE: i32 = 214;
const IDC_TXT_CONFLICT: i32 = 220;
const IDC_BTN_TEST: i32 = 230;
const IDC_CHK_TELEMETRY: i32 = 240;
const IDC_BTN_DIAG: i32 = 250;
const IDC_COMBO_THEME: i32 = 108;
const IDC_COMBO_FSCALE: i32 = 109;
const IDC_COMBO_OPACITY: i32 = 110;
const IDC_COMBO_UI_LANG: i32 = 111;
const IDC_CHK_MAGNIFIER: i32 = 241;
const IDC_CHK_SNAP: i32 = 242;
// M12 新增：OCR / 模型 / 隐私 分区控件
const IDC_CHK_OCRCACHE: i32 = 243;
const IDC_EDIT_OCRMAX: i32 = 112;
const IDC_EDIT_OLURL: i32 = 113;
const IDC_EDIT_OLMODEL: i32 = 114;
const IDC_EDIT_YDKEY: i32 = 115;
const IDC_EDIT_YDSEC: i32 = 116;
const IDC_EDIT_MM: i32 = 117;
const IDC_EDIT_PROXY: i32 = 118;
const IDC_EDIT_MONTHLY: i32 = 119;
const IDC_BTN_EXPORT: i32 = 303;
const IDC_BTN_IMPORT: i32 = 304;
const IDC_BTN_RESET: i32 = 305;
const IDC_BTN_SAVE: i32 = 301;
const IDC_BTN_CANCEL: i32 = 302;
// 侧栏 / 视口 / 导航（§17 设置页重构：可折叠左导航 + 右滚动内容）
const IDC_SIDEBAR: i32 = 401;
const IDC_VIEWPORT: i32 = 402;
const IDC_NAV_BASE: i32 = 600; // +0..+7 各分区
const IDC_NAV_TOGGLE: i32 = 610;
// DXGI 开关（§17.4，本机显卡不支持时关闭）
const IDC_CHK_USE_DXGI: i32 = 246;
/// 内容区总高度（用于滚动范围）
const CONTENT_TOTAL: i32 = 1250;
/// 视口可视高度
const VIEWPORT_H: i32 = 560;
/// 各分区在内容区的 y 偏移（导航跳转用），顺序与 SECTION_NAMES 对应
const SECTION_Y: [i32; 8] = [14, 82, 186, 544, 684, 956, 884, 1150];
const SECTION_NAMES: [&str; 8] = [
    "通用",
    "OCR",
    "模型 / 翻译",
    "隐私 / 配额",
    "快捷键",
    "外观",
    "关于",
    "高级",
];

// 样式常量
const STYLE_BUTTON: u32 = 0x0000_0000; // BS_PUSHBUTTON
const STYLE_EDIT: u32 = 0x0080_0000; // ES_AUTOHSCROLL

// 下拉选项（索引 <-> 值 映射，保存时按索引取回）
const SRC_LANGS: &[&str] = &["auto", "zh", "en", "ja", "ko", "fr", "de", "es", "ru"];
const TGT_LANGS: &[&str] = &["zh", "en", "ja", "ko", "fr", "de", "es", "ru", "auto"];
const OCR_ENGINES: &[&str] = &["local", "sf", "windows"];
const OCR_TIERS: &[&str] = &["tiny", "small", "medium"];
// 注意：翻译引擎不含 OCR 的 "local"。youdao 需在 winocr_config.json 配置
// youdao_app_key / youdao_app_secret（设置页暂无密钥输入框，避免无 GUI 验证下的布局回归）。
const TENGINES: &[&str] = &["sf", "youdao", "mymemory", "ollama"];
const THEMES: &[&str] = &["light", "dark", "auto"];
const FSCALES: &[&str] = &["small", "standard", "large"];
const OPACITIES: &[&str] = &["90", "95", "100"];
const UI_LANGS: &[&str] = &["zh", "en"];

// 录制中的热键组（0 = 无）
thread_local! {
    static RECORDING: RefCell<i32> = RefCell::new(0);
    static CONFIG_REF: RefCell<Option<(Arc<Mutex<Config>>, String)>> = RefCell::new(None);
    // 设置页重构：侧栏/视口句柄、折叠态、当前滚动位置、原视口窗口过程
    static SIDEBAR_HWND: RefCell<HWND> = RefCell::new(ptr::null_mut());
    static VIEWPORT_HWND: RefCell<HWND> = RefCell::new(ptr::null_mut());
    static COLLAPSED: RefCell<bool> = RefCell::new(false);
    static VP_POS: RefCell<i32> = RefCell::new(0);
}

/// 弹出设置窗口（在独立线程调用，模态循环不阻塞常驻主循环）
pub fn show_settings(config: Arc<Mutex<Config>>, config_path: &str) {
    CONFIG_REF.with(|r| *r.borrow_mut() = Some((config, config_path.to_string())));
    unsafe {
        let class_name = to_wide("WinOCRSettingsWindow");
        let vp_class = to_wide("WinOCRSettingsVP");
        let wnd_class = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(settings_proc),
            hInstance: ptr::null_mut(),
            hCursor: LoadCursorW(ptr::null_mut(), IDC_ARROW),
            hbrBackground: GetStockObject(6 /* COLOR_WINDOW+1 */) as *mut _,
            lpszClassName: class_name.as_ptr(),
            lpszMenuName: ptr::null_mut(),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hIcon: ptr::null_mut(),
        };
        RegisterClassW(&wnd_class);
        let vp_wc = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(DefWindowProcW),
            hInstance: ptr::null_mut(),
            hCursor: LoadCursorW(ptr::null_mut(), IDC_ARROW),
            hbrBackground: GetStockObject(6 /* COLOR_WINDOW+1 */) as *mut _,
            lpszClassName: vp_class.as_ptr(),
            lpszMenuName: ptr::null_mut(),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hIcon: ptr::null_mut(),
        };
        RegisterClassW(&vp_wc);

        let title = to_wide("WinOCR-Html 设置");
        let hwnd = CreateWindowExW(
            0,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW & !0x00080000 & !0x00040000,
            300, 40, 800, 640,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
        );
        if hwnd.is_null() {
            return;
        }

        let hfont = GetStockObject(DEFAULT_GUI_FONT as i32);

        // ===== 左侧可折叠导航侧栏 =====
        let sidebar = CreateWindowExW(
            0,
            to_wide("STATIC").as_ptr(),
            ptr::null(),
            WS_CHILD | WS_VISIBLE | WS_BORDER,
            0, 0, 168, VIEWPORT_H,
            hwnd,
            IDC_SIDEBAR as *mut _,
            ptr::null_mut(),
            ptr::null_mut(),
        );
        create_button(sidebar, "« 收起侧栏", IDC_NAV_TOGGLE, 10, 8, 148, 26, hfont);
        for i in 0..SECTION_NAMES.len() {
            create_button(
                sidebar,
                SECTION_NAMES[i],
                IDC_NAV_BASE + i as i32,
                10,
                44 + i as i32 * 46,
                148,
                36,
                hfont,
            );
        }

        // ===== 右侧可滚动内容视口 =====
        let vp = CreateWindowExW(
            0,
            vp_class.as_ptr(),
            ptr::null(),
            WS_CHILD | WS_VISIBLE | WS_VSCROLL | WS_BORDER,
            168, 0, 632, VIEWPORT_H,
            hwnd,
            IDC_VIEWPORT as *mut _,
            ptr::null_mut(),
            ptr::null_mut(),
        );

        // ===== 通用分区（父 = 视口）=====
        create_static(vp, "通用", 20, 14, 200, 20, -1, hfont);
        create_static(vp, "源语言：", 20, 42, 90, 20, -1, hfont);
        let _c_src = create_combo(vp, IDC_COMBO_SRC, 110, 40, 120, hfont, SRC_LANGS);
        create_static(vp, "目标语言：", 250, 42, 90, 20, -1, hfont);
        let _c_tgt = create_combo(vp, IDC_COMBO_TGT, 340, 40, 120, hfont, TGT_LANGS);

        // ===== OCR 分区 =====
        create_static(vp, "OCR", 20, 82, 200, 20, -1, hfont);
        create_static(vp, "OCR 引擎：", 20, 110, 90, 20, -1, hfont);
        let _c_ocr = create_combo(vp, IDC_COMBO_OCR, 110, 108, 120, hfont, OCR_ENGINES);
        create_static(vp, "OCR 精度：", 250, 110, 90, 20, -1, hfont);
        let _c_tier = create_combo(vp, IDC_COMBO_TIER, 340, 108, 120, hfont, OCR_TIERS);
        create_checkbox(vp, "启用 OCR 结果缓存", IDC_CHK_OCRCACHE, 20, 142, 220, 20, hfont);
        create_static(vp, "缓存上限(条)：", 260, 144, 110, 20, -1, hfont);
        let _e_ocrmax = create_control(vp, "EDIT", "", 370, 142, 90, 24, IDC_EDIT_OCRMAX, hfont, WS_EX_CLIENTEDGE | STYLE_EDIT);

        // ===== 模型 / 翻译引擎分区 =====
        create_static(vp, "模型 / 翻译引擎", 20, 186, 260, 20, -1, hfont);
        create_static(vp, "翻译引擎：", 20, 214, 90, 20, -1, hfont);
        let _c_te = create_combo(vp, IDC_COMBO_TENGINE, 110, 212, 160, hfont, TENGINES);
        create_static(vp, "SF 模型：", 20, 248, 90, 20, -1, hfont);
        let _e_model = create_control(vp, "EDIT", "", 110, 246, 240, 24, IDC_EDIT_SFMODEL, hfont, WS_EX_CLIENTEDGE | STYLE_EDIT);
        create_static(vp, "SF 密钥：", 20, 284, 90, 20, -1, hfont);
        let _e_key = create_control(vp, "EDIT", "", 110, 282, 280, 24, IDC_EDIT_SFKEY, hfont, WS_EX_CLIENTEDGE | STYLE_EDIT | ES_PASSWORD);
        create_static(vp, "Ollama 地址：", 20, 320, 90, 20, -1, hfont);
        let _e_olurl = create_control(vp, "EDIT", "", 110, 318, 280, 24, IDC_EDIT_OLURL, hfont, WS_EX_CLIENTEDGE | STYLE_EDIT);
        create_static(vp, "Ollama 模型：", 20, 356, 90, 20, -1, hfont);
        let _e_olmodel = create_control(vp, "EDIT", "", 110, 354, 280, 24, IDC_EDIT_OLMODEL, hfont, WS_EX_CLIENTEDGE | STYLE_EDIT);
        create_static(vp, "有道 App Key：", 20, 392, 110, 20, -1, hfont);
        let _e_ydkey = create_control(vp, "EDIT", "", 130, 390, 260, 24, IDC_EDIT_YDKEY, hfont, WS_EX_CLIENTEDGE | STYLE_EDIT);
        create_static(vp, "有道 App Secret：", 20, 428, 120, 20, -1, hfont);
        let _e_ydsec = create_control(vp, "EDIT", "", 140, 426, 250, 24, IDC_EDIT_YDSEC, hfont, WS_EX_CLIENTEDGE | STYLE_EDIT | ES_PASSWORD);
        create_static(vp, "MyMemory 邮箱：", 20, 464, 110, 20, -1, hfont);
        let _e_mm = create_control(vp, "EDIT", "", 130, 462, 260, 24, IDC_EDIT_MM, hfont, WS_EX_CLIENTEDGE | STYLE_EDIT);
        create_static(vp, "代理地址：", 20, 500, 90, 20, -1, hfont);
        let _e_proxy = create_control(vp, "EDIT", "", 110, 498, 280, 24, IDC_EDIT_PROXY, hfont, WS_EX_CLIENTEDGE | STYLE_EDIT);

        // ===== 隐私 / 配额分区 =====
        create_static(vp, "隐私 / 配额", 20, 544, 260, 20, -1, hfont);
        create_checkbox(vp, "允许匿名遥测（默认关闭，仅错误计数）", IDC_CHK_TELEMETRY, 20, 572, 360, 20, hfont);
        create_static(vp, "月度字符配额(0=不限)：", 20, 606, 170, 20, -1, hfont);
        let _e_monthly = create_control(vp, "EDIT", "", 190, 604, 100, 24, IDC_EDIT_MONTHLY, hfont, WS_EX_CLIENTEDGE | STYLE_EDIT);
        create_static(vp, "密钥经 Windows DPAPI 本地加密落盘，不离开本机。", 20, 640, 440, 20, -1, hfont);

        // ===== 快捷键分区 =====
        create_static(vp, "快捷键（点击「录制」后按下组合键）", 20, 684, 380, 20, -1, hfont);
        create_static(vp, "截图 OCR：", 20, 714, 120, 20, -1, hfont);
        create_button(vp, "录制", IDC_REC_CAP, 150, 712, 70, 24, hfont);
        create_edit_ro(vp, IDC_TXT_CAP, 230, 712, 180, 24, hfont);
        create_static(vp, "打开设置：", 20, 746, 120, 20, -1, hfont);
        create_button(vp, "录制", IDC_REC_SET, 150, 744, 70, 24, hfont);
        create_edit_ro(vp, IDC_TXT_SET, 230, 744, 180, 24, hfont);
        create_static(vp, "退出程序：", 20, 778, 120, 20, -1, hfont);
        create_button(vp, "录制", IDC_REC_QUIT, 150, 776, 70, 24, hfont);
        create_edit_ro(vp, IDC_TXT_QUIT, 230, 776, 180, 24, hfont);
        create_static(vp, "暂停/恢复：", 20, 810, 120, 20, -1, hfont);
        create_button(vp, "录制", IDC_REC_PAUSE, 150, 808, 70, 24, hfont);
        create_edit_ro(vp, IDC_TXT_PAUSE, 230, 808, 180, 24, hfont);
        create_static(vp, "", 20, 844, 420, 20, IDC_TXT_CONFLICT, hfont);
        create_button(vp, "测试一下", IDC_BTN_TEST, 420, 842, 90, 24, hfont);

        // ===== 关于分区 =====
        create_static(vp, "关于", 20, 884, 200, 20, -1, hfont);
        let ver = format!("版本 v{}", crate::APP_VERSION);
        create_static(vp, &ver, 20, 914, 200, 20, -1, hfont);
        create_button(vp, "运行诊断", IDC_BTN_DIAG, 400, 912, 120, 24, hfont);

        // ===== 外观分区 =====
        create_static(vp, "外观", 20, 956, 200, 20, -1, hfont);
        create_static(vp, "主题：", 20, 986, 90, 20, -1, hfont);
        let _c_theme = create_combo(vp, IDC_COMBO_THEME, 110, 984, 120, hfont, THEMES);
        create_static(vp, "字号：", 250, 986, 90, 20, -1, hfont);
        let _c_fs = create_combo(vp, IDC_COMBO_FSCALE, 340, 984, 120, hfont, FSCALES);
        create_static(vp, "不透明度：", 20, 1022, 90, 20, -1, hfont);
        let _c_op = create_combo(vp, IDC_COMBO_OPACITY, 110, 1020, 120, hfont, OPACITIES);
        create_static(vp, "界面语言：", 20, 1058, 90, 20, -1, hfont);
        let _c_lang = create_combo(vp, IDC_COMBO_UI_LANG, 110, 1056, 120, hfont, UI_LANGS);
        create_checkbox(vp, "选区放大镜（§17.4）", IDC_CHK_MAGNIFIER, 250, 1056, 220, 24, hfont);
        create_checkbox(vp, "边缘吸附（§17.4）", IDC_CHK_SNAP, 250, 1086, 220, 24, hfont);
        create_checkbox(vp, "使用 DXGI 加速截图（关闭则 GDI，兼容性更好）", IDC_CHK_USE_DXGI, 20, 1118, 460, 24, hfont);

        // ===== 高级分区 =====
        create_static(vp, "高级", 20, 1150, 200, 20, -1, hfont);
        create_button(vp, "导出配置", IDC_BTN_EXPORT, 20, 1180, 120, 28, hfont);
        create_button(vp, "导入配置", IDC_BTN_IMPORT, 160, 1180, 120, 28, hfont);
        create_button(vp, "重置出厂", IDC_BTN_RESET, 300, 1180, 120, 28, hfont);
        create_static(vp, "配置位于 exe 同目录 winocr_config.json", 20, 1218, 420, 20, -1, hfont);

        // ===== 底部常驻（不随滚动）=====
        create_button(hwnd, "保存", IDC_BTN_SAVE, 580, 576, 100, 34, hfont);
        create_button(hwnd, "取消", IDC_BTN_CANCEL, 690, 576, 100, 34, hfont);

        // 初始化滚动范围并子类化视口
        let mut si: SCROLLINFO = mem::zeroed();
        si.cbSize = mem::size_of::<SCROLLINFO>() as u32;
        si.fMask = SIF_RANGE | SIF_PAGE;
        si.nMin = 0;
        si.nMax = CONTENT_TOTAL;
        si.nPage = VIEWPORT_H as u32;
        SetScrollInfo(vp, SB_VERT as i32, &si, 0);
        VP_POS.with(|p| *p.borrow_mut() = 0);
        COLLAPSED.with(|c| *c.borrow_mut() = false);
        SIDEBAR_HWND.with(|s| *s.borrow_mut() = sidebar);
        VIEWPORT_HWND.with(|v| *v.borrow_mut() = vp);
        SetWindowLongPtrW(vp, GWLP_WNDPROC, viewport_proc as isize);

        // 用当前配置初始化控件
        init_controls_from_config(hwnd);

        ShowWindow(hwnd, SW_SHOW);
        SetFocus(_e_model);

        let mut msg: MSG = mem::zeroed();
        loop {
            let result = GetMessageW(&mut msg, ptr::null_mut(), 0, 0);
            if result == 0 || result == -1 {
                break;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
            // saved/cancelled 通过 DestroyWindow 触发 WM_DESTROY -> break
            if msg.message == WM_DESTROY {
                break;
            }
        }
        // 不在此 DestroyWindow（可能已销毁）
    }
}

/// 用当前配置填充所有控件
unsafe fn init_controls_from_config(hwnd: HWND) {
    let arc = CONFIG_REF.with(|r| r.borrow().clone());
    let (arc_cfg, _path) = match arc {
        Some(v) => v,
        None => return,
    };
    let cfg = arc_cfg.lock().unwrap();
    set_combo_sel(hwnd, IDC_COMBO_SRC, SRC_LANGS, &cfg.src_lang);
    set_combo_sel(hwnd, IDC_COMBO_TGT, TGT_LANGS, &cfg.tgt_lang);
    set_combo_sel(hwnd, IDC_COMBO_OCR, OCR_ENGINES, ocr_to_str(&cfg.ocr_engine));
    set_combo_sel(hwnd, IDC_COMBO_TIER, OCR_TIERS, tier_to_str(&cfg.local_ocr_tier));
    set_combo_sel(hwnd, IDC_COMBO_TENGINE, TENGINES, &cfg.translate_engine);
    set_edit_text(hwnd, IDC_EDIT_SFMODEL, &cfg.sf_model);
    set_edit_text(hwnd, IDC_EDIT_SFKEY, &cfg.sf_key);
    set_edit_text(hwnd, IDC_TXT_CAP, &cfg.hotkey);
    set_edit_text(hwnd, IDC_TXT_SET, &cfg.settings_hotkey);
    set_edit_text(hwnd, IDC_TXT_QUIT, &cfg.quit_hotkey);
    set_edit_text(hwnd, IDC_TXT_PAUSE, &cfg.pause_hotkey);
    set_check(hwnd, IDC_CHK_TELEMETRY, cfg.telemetry);
    set_combo_sel(hwnd, IDC_COMBO_THEME, THEMES, &cfg.theme);
    set_combo_sel(hwnd, IDC_COMBO_FSCALE, FSCALES, &cfg.font_scale);
    let op_str = match cfg.float_opacity {
        230 => "90",
        242 => "95",
        _ => "100",
    };
    set_combo_sel(hwnd, IDC_COMBO_OPACITY, OPACITIES, op_str);
    set_combo_sel(hwnd, IDC_COMBO_UI_LANG, UI_LANGS, &cfg.ui_lang);
    set_check(hwnd, IDC_CHK_MAGNIFIER, cfg.magnifier);
    set_check(hwnd, IDC_CHK_SNAP, cfg.snap);
    set_check(hwnd, IDC_CHK_USE_DXGI, cfg.use_dxgi);
    // M12：OCR / 模型 / 隐私 分区回填
    set_check(hwnd, IDC_CHK_OCRCACHE, cfg.ocr_cache_enabled);
    set_edit_text(hwnd, IDC_EDIT_OCRMAX, &cfg.ocr_cache_max.to_string());
    set_edit_text(hwnd, IDC_EDIT_OLURL, &cfg.ollama_url);
    set_edit_text(hwnd, IDC_EDIT_OLMODEL, &cfg.ollama_model);
    set_edit_text(hwnd, IDC_EDIT_YDKEY, &cfg.youdao_app_key);
    set_edit_text(hwnd, IDC_EDIT_YDSEC, &cfg.youdao_app_secret);
    set_edit_text(hwnd, IDC_EDIT_MM, &cfg.mymemory_email);
    set_edit_text(hwnd, IDC_EDIT_PROXY, &cfg.proxy_url);
    set_edit_text(hwnd, IDC_EDIT_MONTHLY, &cfg.monthly_char_limit.to_string());
}

unsafe extern "system" fn settings_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: isize,
) -> isize {
    match msg {
        WM_COMMAND => {
            let id = LOWORD(wparam as u32) as i32;
            if lparam != 0 && ((wparam >> 16) & 0xFFFF) == 0 {
                // 按钮点击（BN_CLICKED）
                match id {
                    IDC_REC_CAP | IDC_REC_SET | IDC_REC_QUIT | IDC_REC_PAUSE => {
                        start_recording(hwnd, id);
                    }
                    IDC_BTN_TEST => {
                        test_hotkeys(hwnd);
                    }
                    IDC_BTN_DIAG => {
                        run_doctor_ui(hwnd);
                    }
                    IDC_BTN_EXPORT => {
                        export_config(hwnd);
                    }
                    IDC_BTN_IMPORT => {
                        import_config(hwnd);
                    }
                    IDC_BTN_RESET => {
                        reset_config(hwnd);
                    }
                    IDC_BTN_SAVE => {
                        if save_config(hwnd) {
                            DestroyWindow(hwnd);
                        }
                    }
                    IDC_BTN_CANCEL => {
                        DestroyWindow(hwnd);
                    }
                    IDC_NAV_TOGGLE => {
                        toggle_sidebar(hwnd);
                    }
                    x if (IDC_NAV_BASE..IDC_NAV_BASE + 8).contains(&x) => {
                        let idx = (x - IDC_NAV_BASE) as usize;
                        if idx < SECTION_Y.len() {
                            jump_to_section(VIEWPORT_HWND.with(|v| *v.borrow()), SECTION_Y[idx]);
                        }
                    }
                    _ => {}
                }
            }
            0
        }
        WM_KEYDOWN => {
            let rec = RECORDING.with(|r| *r.borrow());
            if rec != 0 {
                capture_hotkey(hwnd, rec, wparam as i32);
                0
            } else {
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
        }
        WM_CLOSE => {
            DestroyWindow(hwnd);
            0
        }
        WM_DESTROY => {
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// 视口窗口过程：仅接管垂直滚动，其余交由默认过程
unsafe extern "system" fn viewport_proc(
    hwnd: HWND,
    msg: UINT,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_VSCROLL => {
            vscroll(hwnd, wparam as u32);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// 处理视口垂直滚动命令
unsafe fn vscroll(hwnd: HWND, wparam: u32) {
    let cmd = (wparam & 0xFFFF) as i32;
    let pos = VP_POS.with(|p| *p.borrow());
    let max = (CONTENT_TOTAL - VIEWPORT_H).max(0);
    let newpos = if cmd == SB_LINEUP as i32 {
        (pos - 20).max(0)
    } else if cmd == SB_LINEDOWN as i32 {
        (pos + 20).min(max)
    } else if cmd == SB_PAGEUP as i32 {
        (pos - VIEWPORT_H).max(0)
    } else if cmd == SB_PAGEDOWN as i32 {
        (pos + VIEWPORT_H).min(max)
    } else if cmd == SB_TOP as i32 {
        0
    } else if cmd == SB_BOTTOM as i32 {
        max
    } else if cmd == SB_THUMBPOSITION as i32 || cmd == SB_THUMBTRACK as i32 {
        ((wparam >> 16) as i32).min(max).max(0)
    } else {
        pos
    };
    apply_scroll(hwnd, pos - newpos);
}

/// 将视口内容滚动 delta 像素（负=向下）
unsafe fn apply_scroll(hwnd: HWND, delta: i32) {
    if delta == 0 {
        return;
    }
    ScrollWindowEx(
        hwnd,
        0,
        delta,
        ptr::null(),
        ptr::null(),
        ptr::null_mut(),
        ptr::null_mut(),
        SW_INVALIDATE | SW_ERASE,
    );
    let newpos = (VP_POS.with(|p| *p.borrow()) - delta).max(0);
    VP_POS.with(|p| *p.borrow_mut() = newpos);
    let mut si: SCROLLINFO = mem::zeroed();
    si.cbSize = mem::size_of::<SCROLLINFO>() as u32;
    si.fMask = SIF_POS;
    si.nPos = newpos;
    SetScrollInfo(hwnd, SB_VERT as i32, &si, 1);
}

/// 跳转到指定分区 y 偏移
unsafe fn jump_to_section(hwnd: HWND, y: i32) {
    let max = (CONTENT_TOTAL - VIEWPORT_H).max(0);
    let newpos = y.min(max).max(0);
    let pos = VP_POS.with(|p| *p.borrow());
    apply_scroll(hwnd, pos - newpos);
}

/// 折叠 / 展开左侧导航侧栏
unsafe fn toggle_sidebar(hwnd: HWND) {
    let collapsed = COLLAPSED.with(|c| {
        let v = !*c.borrow();
        *c.borrow_mut() = v;
        v
    });
    let sb = SIDEBAR_HWND.with(|s| *s.borrow());
    let vp = VIEWPORT_HWND.with(|v| *v.borrow());
    if collapsed {
        ShowWindow(sb, SW_HIDE);
        SetWindowPos(
            vp,
            ptr::null_mut(),
            0,
            0,
            800,
            VIEWPORT_H,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
    } else {
        ShowWindow(sb, 1 /* SW_SHOW */);
        SetWindowPos(
            vp,
            ptr::null_mut(),
            168,
            0,
            632,
            VIEWPORT_H,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

/// 开始录制某个热键：记录组 ID，按钮变「请按键…」
unsafe fn start_recording(hwnd: HWND, rec_id: i32) {
    RECORDING.with(|r| *r.borrow_mut() = rec_id);
    set_button_text(hwnd, rec_id, "请按键…");
    set_static_text(hwnd, IDC_TXT_CONFLICT, "");
}

/// 捕获键盘组合，写入对应文本框
unsafe fn capture_hotkey(hwnd: HWND, rec_id: i32, vk: i32) {
    let ctrl = (GetAsyncKeyState(VK_CONTROL as i32) as u16 & 0x8000) != 0;
    let alt = (GetAsyncKeyState(VK_MENU as i32) as u16 & 0x8000) != 0;
    let shift = (GetAsyncKeyState(VK_SHIFT as i32) as u16 & 0x8000) != 0;
    let win = (GetAsyncKeyState(VK_LWIN as i32) as u16 & 0x8000) != 0
        || (GetAsyncKeyState(VK_RWIN as i32) as u16 & 0x8000) != 0;

    if let Some(s) = build_hotkey(vk, ctrl, alt, shift, win) {
        let txt_id = match rec_id {
            IDC_REC_CAP => IDC_TXT_CAP,
            IDC_REC_SET => IDC_TXT_SET,
            IDC_REC_QUIT => IDC_TXT_QUIT,
            _ => IDC_TXT_PAUSE,
        };
        set_edit_text(hwnd, txt_id, &s);
        set_button_text(hwnd, rec_id, "录制");
        RECORDING.with(|r| *r.borrow_mut() = 0);
        check_conflict(hwnd);
    }
    // 纯修饰键不写入，等待主键
}

/// 组合键字符串：支持 字母 / 数字 / F1-F12 + ctrl/alt/shift/win
fn build_hotkey(vk: i32, ctrl: bool, alt: bool, shift: bool, win: bool) -> Option<String> {
    // 过滤纯修饰键
    if matches!(vk, x if x == VK_CONTROL as i32 || x == VK_MENU as i32 || x == VK_SHIFT as i32 || x == VK_LWIN as i32 || x == VK_RWIN as i32) {
        return None;
    }
    let key = if (VK_F1..=VK_F12).contains(&vk) {
        format!("f{}", vk - VK_F1 + 1)
    } else if (0x41..=0x5A).contains(&vk) {
        // A-Z
        (vk as u8 as char).to_ascii_lowercase().to_string()
    } else if (0x30..=0x39).contains(&vk) {
        // 0-9
        (vk as u8 as char).to_string()
    } else {
        return None;
    };
    let mut parts: Vec<&str> = Vec::new();
    if ctrl {
        parts.push("ctrl");
    }
    if alt {
        parts.push("alt");
    }
    if shift {
        parts.push("shift");
    }
    if win {
        parts.push("win");
    }
    parts.push(&key);
    Some(parts.join("+"))
}

/// 读取四个热键文本框，检测重复
unsafe fn read_hotkeys(hwnd: HWND) -> (String, String, String, String) {
    (
        read_edit(hwnd, IDC_TXT_CAP),
        read_edit(hwnd, IDC_TXT_SET),
        read_edit(hwnd, IDC_TXT_QUIT),
        read_edit(hwnd, IDC_TXT_PAUSE),
    )
}

/// 冲突检测：重复则显示提示，返回是否有冲突
unsafe fn check_conflict(hwnd: HWND) -> bool {
    let (c, s, q, p) = read_hotkeys(hwnd);
    let list = [("截图", &c), ("设置", &s), ("退出", &q), ("暂停", &p)];
    for i in 0..4 {
        for j in (i + 1)..4 {
            if !list[i].1.is_empty()
                && !list[j].1.is_empty()
                && list[i].1.eq_ignore_ascii_case(list[j].1)
            {
                let msg = format!("热键冲突：{} 与 {} 重复（{}）", list[i].0, list[j].0, list[i].1);
                set_static_text(hwnd, IDC_TXT_CONFLICT, &msg);
                return true;
            }
        }
    }
    set_static_text(hwnd, IDC_TXT_CONFLICT, "");
    false
}

/// 「测试一下」：校验四个热键格式是否合法
unsafe fn test_hotkeys(hwnd: HWND) {
    let (c, s, q, p) = read_hotkeys(hwnd);
    let all = [c, s, q, p];
    let mut bad = Vec::new();
    for h in all.iter() {
        if h.is_empty() {
            bad.push("存在空热键");
            break;
        }
        // 简单校验：至少一个字母/数字主键
        let has_key = h
            .split('+')
            .last()
            .map(|k| k.chars().any(|ch| ch.is_ascii_alphanumeric()))
            .unwrap_or(false);
        if !has_key {
            bad.push("格式无效");
            break;
        }
    }
    let msg = if !bad.is_empty() {
        format!("校验未通过：{}", bad[0])
    } else if check_conflict(hwnd) {
        "校验未通过：存在热键冲突".to_string()
    } else {
        "校验通过：热键格式有效、无冲突".to_string()
    };
    MessageBoxW(hwnd, to_wide(&msg).as_ptr(), to_wide("热键测试").as_ptr(), 0x40);
}

/// P1-6：运行诊断并弹脱敏摘要
unsafe fn run_doctor_ui(hwnd: HWND) {
    let report = selfcheck::run_self_check();
    let summary = format_doctor_summary(&report);
    // 完整报告写入日志目录（复用逻辑）
    let dir = dirs::data_local_dir()
        .map(|p| p.join("WinOCR-Html").join("logs"))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let _ = std::fs::create_dir_all(&dir);
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = dir.join(format!("doctor-{}.json", ts));
    let _ = std::fs::write(&path, serde_json::to_string_pretty(&report).unwrap_or_default());
    let full = format!("{}\n\n完整报告已写入：\n{}", summary, path.display());
    MessageBoxW(
        hwnd,
        to_wide(&full).as_ptr(),
        to_wide("一键诊断").as_ptr(),
        0x40,
    );
}

/// 导出/导入的备份文件路径（exe 同目录，winocr_config.backup.json）
fn default_backup_path() -> String {
    match crate::config::default_config_path() {
        Ok(p) => {
            if p.ends_with(".json") {
                format!("{}.backup", &p[..p.len() - 5])
            } else {
                format!("{}.backup", p)
            }
        }
        Err(_) => "winocr_config.backup.json".to_string(),
    }
}

/// 导出当前配置到备份文件（含 DPAPI 加密后的密钥）
unsafe fn export_config(hwnd: HWND) {
    let arc = CONFIG_REF.with(|r| r.borrow().clone());
    if let Some((arc, _path)) = arc {
        let backup = default_backup_path();
        let res = {
            let c = arc.lock().unwrap();
            c.save(&backup)
        };
        match res {
            Ok(_) => MessageBoxW(
                hwnd,
                to_wide(&format!("配置已导出到：\n{}", backup)).as_ptr(),
                to_wide("导出成功").as_ptr(),
                0x40,
            ),
            Err(e) => MessageBoxW(
                hwnd,
                to_wide(&format!("导出失败：{}", e)).as_ptr(),
                to_wide("错误").as_ptr(),
                0x30,
            ),
        };
    }
}

/// 从备份文件导入配置并覆盖当前（需重启生效）
unsafe fn import_config(hwnd: HWND) {
    let arc = CONFIG_REF.with(|r| r.borrow().clone());
    if let Some((arc, path)) = arc {
        let backup = default_backup_path();
        if !Path::new(&backup).exists() {
            MessageBoxW(
                hwnd,
                to_wide("未找到导出文件，请先点击「导出配置」").as_ptr(),
                to_wide("提示").as_ptr(),
                0x40,
            );
            return;
        }
        match Config::load(Some(&backup)) {
            Ok(loaded) => match loaded.save(&path) {
                Ok(_) => {
                    *arc.lock().unwrap() = loaded;
                    MessageBoxW(
                        hwnd,
                        to_wide("导入成功，请重启程序使配置生效").as_ptr(),
                        to_wide("导入成功").as_ptr(),
                        0x40,
                    )
                }
                Err(e) => MessageBoxW(
                    hwnd,
                    to_wide(&format!("导入失败：{}", e)).as_ptr(),
                    to_wide("错误").as_ptr(),
                    0x30,
                ),
            },
            Err(e) => MessageBoxW(
                hwnd,
                to_wide(&format!("读取导出文件失败：{}", e)).as_ptr(),
                to_wide("错误").as_ptr(),
                0x30,
            ),
        };
    }
}

/// 重置为出厂设置（需重启生效）
unsafe fn reset_config(hwnd: HWND) {
    let arc = CONFIG_REF.with(|r| r.borrow().clone());
    if let Some((arc, path)) = arc {
        let d = Config::default();
        match d.save(&path) {
            Ok(_) => {
                *arc.lock().unwrap() = d;
                MessageBoxW(
                    hwnd,
                    to_wide("已重置为出厂设置，请重启程序").as_ptr(),
                    to_wide("已重置").as_ptr(),
                    0x40,
                )
            }
            Err(e) => MessageBoxW(
                hwnd,
                to_wide(&format!("重置失败：{}", e)).as_ptr(),
                to_wide("错误").as_ptr(),
                0x30,
            ),
        };
    }
}

/// 把 selfcheck 报告格式化为脱敏摘要文本
fn format_doctor_summary(report: &serde_json::Value) -> String {
    let mut lines = vec![
        format!("宿主：{}", report.get("host_name").unwrap_or(&serde_json::Value::Null)),
        format!("版本：{}", report.get("app_version").unwrap_or(&serde_json::Value::Null)),
        format!(
            "配置存在：{}",
            report.get("config_exists").unwrap_or(&serde_json::Value::Null)
        ),
        format!(
            "已注册任一边浏览器：{}",
            report.get("registered_any").unwrap_or(&serde_json::Value::Null)
        ),
    ];
    if let Some(browsers) = report.get("browsers").and_then(|v| v.as_array()) {
        for b in browsers {
            let name = b.get("name").and_then(|v| v.as_str()).unwrap_or("?");
            let reg = b.get("registered").and_then(|v| v.as_bool()).unwrap_or(false);
            let me = b.get("manifest_exists").and_then(|v| v.as_bool()).unwrap_or(false);
            let ex = b.get("exe_exists").and_then(|v| v.as_bool()).unwrap_or(false);
            let ok = b.get("allowed_origin_ok").and_then(|v| v.as_bool()).unwrap_or(false);
            lines.push(format!(
                "· {}：注册={} 清单={} 程序={} 扩展ID={}",
                name, reg, me, ex, ok
            ));
        }
    }
    // 脱敏：去掉可能含用户路径的字段
    lines.push("（已脱敏：不含密钥与完整用户路径）".to_string());
    lines.join("\n")
}

/// 保存：读取控件 -> 写回 Arc<Mutex<Config>> -> 落盘
unsafe fn save_config(hwnd: HWND) -> bool {
    if check_conflict(hwnd) {
        MessageBoxW(
            hwnd,
            to_wide("存在热键冲突，请先解决后再保存").as_ptr(),
            to_wide("无法保存").as_ptr(),
            0x30,
        );
        return false;
    }
    let (arc, path) = match CONFIG_REF.with(|r| r.borrow().clone()) {
        Some(v) => v,
        None => return false,
    };
    let mut cfg = arc.lock().unwrap();

    cfg.src_lang = combo_sel(hwnd, IDC_COMBO_SRC, SRC_LANGS).to_string();
    cfg.tgt_lang = combo_sel(hwnd, IDC_COMBO_TGT, TGT_LANGS).to_string();
    cfg.ocr_engine = match combo_sel(hwnd, IDC_COMBO_OCR, OCR_ENGINES) {
        "sf" => OcrEngine::Sf,
        "windows" => OcrEngine::Windows,
        _ => OcrEngine::Local,
    };
    cfg.local_ocr_tier = match combo_sel(hwnd, IDC_COMBO_TIER, OCR_TIERS) {
        "small" => OcrTier::Small,
        "medium" => OcrTier::Medium,
        _ => OcrTier::Tiny,
    };
    cfg.translate_engine = combo_sel(hwnd, IDC_COMBO_TENGINE, TENGINES).to_string();
    cfg.sf_model = read_edit(hwnd, IDC_EDIT_SFMODEL);
    cfg.sf_key = read_edit(hwnd, IDC_EDIT_SFKEY);
    let (c, s, q, p) = read_hotkeys(hwnd);
    cfg.hotkey = c.trim().to_lowercase();
    cfg.settings_hotkey = s.trim().to_lowercase();
    cfg.quit_hotkey = q.trim().to_lowercase();
    cfg.pause_hotkey = p.trim().to_lowercase();
    cfg.telemetry = get_check(hwnd, IDC_CHK_TELEMETRY);
    cfg.theme = combo_sel(hwnd, IDC_COMBO_THEME, THEMES).to_string();
    cfg.font_scale = combo_sel(hwnd, IDC_COMBO_FSCALE, FSCALES).to_string();
    cfg.float_opacity = match combo_sel(hwnd, IDC_COMBO_OPACITY, OPACITIES) {
        "90" => 230,
        "95" => 242,
        _ => 255,
    };
    cfg.ui_lang = combo_sel(hwnd, IDC_COMBO_UI_LANG, UI_LANGS).to_string();
    crate::i18n::set_locale(&cfg.ui_lang);
    cfg.magnifier = get_check(hwnd, IDC_CHK_MAGNIFIER);
    cfg.snap = get_check(hwnd, IDC_CHK_SNAP);
    cfg.use_dxgi = get_check(hwnd, IDC_CHK_USE_DXGI);
    // M12：OCR / 模型 / 隐私 分区读取
    cfg.ocr_cache_enabled = get_check(hwnd, IDC_CHK_OCRCACHE);
    cfg.ocr_cache_max = read_edit(hwnd, IDC_EDIT_OCRMAX).trim().parse().unwrap_or(500);
    cfg.ollama_url = read_edit(hwnd, IDC_EDIT_OLURL);
    cfg.ollama_model = read_edit(hwnd, IDC_EDIT_OLMODEL);
    cfg.youdao_app_key = read_edit(hwnd, IDC_EDIT_YDKEY);
    cfg.youdao_app_secret = read_edit(hwnd, IDC_EDIT_YDSEC);
    cfg.mymemory_email = read_edit(hwnd, IDC_EDIT_MM);
    cfg.proxy_url = read_edit(hwnd, IDC_EDIT_PROXY);
    cfg.monthly_char_limit = read_edit(hwnd, IDC_EDIT_MONTHLY).trim().parse().unwrap_or(0);

    if let Err(e) = cfg.save(&path) {
        MessageBoxW(
            hwnd,
            to_wide(&format!("保存失败：{}", e)).as_ptr(),
            to_wide("错误").as_ptr(),
            0x30,
        );
        return false;
    }
    MessageBoxW(
        hwnd,
        to_wide("设置已保存（热键/引擎变更重启程序后生效）").as_ptr(),
        to_wide("已保存").as_ptr(),
        0x40,
    );
    true
}

// ===================== 控件辅助 =====================

unsafe fn create_control(
    parent: HWND,
    class: &str,
    text: &str,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    id: i32,
    hfont: *mut std::ffi::c_void,
    style_extra: u32,
) -> HWND {
    let class_w = to_wide(class);
    let text_w = to_wide(text);
    let hwnd = CreateWindowExW(
        0,
        class_w.as_ptr(),
        text_w.as_ptr(),
        WS_CHILD | WS_VISIBLE | style_extra,
        x, y, w, h,
        parent,
        id as *mut _,
        ptr::null_mut(),
        ptr::null_mut(),
    );
    if !hfont.is_null() {
        SendMessageW(hwnd, WM_SETFONT, hfont as WPARAM, 1);
    }
    hwnd
}

unsafe fn create_static(parent: HWND, text: &str, x: i32, y: i32, w: i32, h: i32, id: i32, hfont: *mut std::ffi::c_void) -> HWND {
    create_control(parent, "STATIC", text, x, y, w, h, id, hfont, 0)
}

unsafe fn create_button(parent: HWND, text: &str, id: i32, x: i32, y: i32, w: i32, h: i32, hfont: *mut std::ffi::c_void) -> HWND {
    create_control(parent, "BUTTON", text, x, y, w, h, id, hfont, STYLE_BUTTON)
}

unsafe fn create_edit_ro(parent: HWND, id: i32, x: i32, y: i32, w: i32, h: i32, hfont: *mut std::ffi::c_void) -> HWND {
    create_control(parent, "EDIT", "", x, y, w, h, id, hfont, WS_EX_CLIENTEDGE | STYLE_EDIT | 0x0800 /* ES_READONLY */)
}

unsafe fn create_checkbox(parent: HWND, text: &str, id: i32, x: i32, y: i32, w: i32, h: i32, hfont: *mut std::ffi::c_void) -> HWND {
    create_control(parent, "BUTTON", text, x, y, w, h, id, hfont, BS_CHECKBOX)
}

unsafe fn create_combo(parent: HWND, id: i32, x: i32, y: i32, w: i32, hfont: *mut std::ffi::c_void, items: &[&str]) -> HWND {
    let hwnd = CreateWindowExW(
        0,
        to_wide("COMBOBOX").as_ptr(),
        ptr::null(),
        WS_CHILD | WS_VISIBLE | CBS_DROPDOWNLIST,
        x, y, w, 200,
        parent,
        id as *mut _,
        ptr::null_mut(),
        ptr::null_mut(),
    );
    if !hfont.is_null() {
        SendMessageW(hwnd, WM_SETFONT, hfont as WPARAM, 1);
    }
    for it in items {
        SendMessageW(hwnd, CB_ADDSTRING, 0, to_wide(it).as_ptr() as LPARAM);
    }
    hwnd
}

unsafe fn set_combo_sel(hwnd: HWND, id: i32, items: &[&str], value: &str) -> i32 {
    let ctrl = GetDlgItem(hwnd, id);
    let idx = items.iter().position(|v| v.eq_ignore_ascii_case(value)).unwrap_or(0);
    if !ctrl.is_null() {
        SendMessageW(ctrl, CB_SETCURSEL, idx as usize, 0);
    }
    idx as i32
}

unsafe fn combo_sel<'a>(hwnd: HWND, id: i32, items: &'a [&str]) -> &'a str {
    let ctrl = GetDlgItem(hwnd, id);
    if ctrl.is_null() {
        return items[0];
    }
    let idx = SendMessageW(ctrl, CB_GETCURSEL, 0, 0) as i32;
    items.get(idx as usize).copied().unwrap_or(items[0])
}

unsafe fn set_edit_text(hwnd: HWND, id: i32, text: &str) {
    let ctrl = GetDlgItem(hwnd, id);
    if !ctrl.is_null() {
        SetWindowTextW(ctrl, to_wide(text).as_ptr());
    }
}

unsafe fn set_static_text(hwnd: HWND, id: i32, text: &str) {
    let ctrl = GetDlgItem(hwnd, id);
    if !ctrl.is_null() {
        SetWindowTextW(ctrl, to_wide(text).as_ptr());
    }
}

unsafe fn set_button_text(hwnd: HWND, id: i32, text: &str) {
    let ctrl = GetDlgItem(hwnd, id);
    if !ctrl.is_null() {
        SetWindowTextW(ctrl, to_wide(text).as_ptr());
    }
}

unsafe fn set_check(hwnd: HWND, id: i32, checked: bool) {
    let ctrl = GetDlgItem(hwnd, id);
    if !ctrl.is_null() {
        SendMessageW(ctrl, BM_SETCHECK, if checked { BST_CHECKED } else { 0 } as usize, 0);
    }
}

unsafe fn get_check(hwnd: HWND, id: i32) -> bool {
    let ctrl = GetDlgItem(hwnd, id);
    if ctrl.is_null() {
        return false;
    }
    SendMessageW(ctrl, BM_GETCHECK, 0, 0) == BST_CHECKED as isize
}

unsafe fn read_edit(hwnd: HWND, id: i32) -> String {
    let ctrl = GetDlgItem(hwnd, id);
    if ctrl.is_null() {
        return String::new();
    }
    let len = GetWindowTextLengthW(ctrl);
    if len <= 0 {
        return String::new();
    }
    let mut buf = vec![0u16; (len + 1) as usize];
    let n = GetWindowTextW(ctrl, buf.as_mut_ptr(), len + 1);
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

/// 取对话框子控件句柄（与控件 ID 对应）
unsafe fn GetDlgItem(hwnd: HWND, id: i32) -> HWND {
    use winapi::um::winuser::GetDlgItem as GDI;
    GDI(hwnd, id)
}

fn to_wide(s: &str) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    std::ffi::OsString::from(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

fn ocr_to_str(e: &OcrEngine) -> &'static str {
    match e {
        OcrEngine::Local => "local",
        OcrEngine::Sf => "sf",
        OcrEngine::Windows => "windows",
    }
}

fn tier_to_str(t: &OcrTier) -> &'static str {
    match t {
        OcrTier::Tiny => "tiny",
        OcrTier::Small => "small",
        OcrTier::Medium => "medium",
    }
}
