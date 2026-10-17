//! 历史 / 生词本 / 复习窗口（§17.6 窗口 UI）
//!
//! 三个独立 Win32 窗口（纯 winapi 自绘，沿用设置窗口风格）：
//! - `show_history`：历史列表（搜索 + 导出 JSON/CSV/MD + 清空）
//! - `show_vocab`：生词本列表（复习 + 导出 Anki/CSV + 删除）
//! - `show_review`：复习闪卡（空格翻面 / 1-3 打分 / Esc 退出，SM-2 推进）
//!
//! 均在独立线程或独立消息循环中运行（由调用方 spawn 或主循环直接打开）。

use crate::history::{self, HistoryItem, VocabItem};
use crate::tr;
use std::cell::RefCell;
use std::mem;
use std::ptr;
use winapi::shared::minwindef::{LPARAM, UINT, WPARAM};
use winapi::shared::windef::HWND;
use winapi::um::winuser::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetDlgItem, GetMessageW,
    GetWindowTextLengthW, GetWindowTextW, LoadCursorW, MessageBoxW, RegisterClassW, SendMessageW,
    SetWindowTextW, ShowWindow, TranslateMessage, BS_PUSHBUTTON, CS_HREDRAW, CS_VREDRAW, IDC_ARROW,
    LBS_HASSTRINGS, LBS_NOTIFY, LB_ADDSTRING, LB_DELETESTRING, LB_GETCURSEL, LB_GETTEXT,
    LB_RESETCONTENT, VK_ESCAPE, VK_SPACE, WM_CLOSE, WM_COMMAND, WM_DESTROY,
    WM_KEYDOWN, WS_CHILD, WS_OVERLAPPEDWINDOW, WS_VISIBLE, WS_VSCROLL, CW_USEDEFAULT, SW_SHOW,
};

thread_local! {
    static HIST_ITEMS: RefCell<Vec<HistoryItem>> = RefCell::new(Vec::new());
    static VOCAB_ITEMS: RefCell<Vec<VocabItem>> = RefCell::new(Vec::new());
    static REVIEW_QUEUE: RefCell<Vec<VocabItem>> = RefCell::new(Vec::new());
    static REVIEW_SHOW_FRONT: RefCell<bool> = RefCell::new(true);
}

// 历史窗口控件 ID
const H_LIST: i32 = 101;
const H_SEARCH: i32 = 102;
const H_EXPORT_JSON: i32 = 110;
const H_EXPORT_CSV: i32 = 111;
const H_EXPORT_MD: i32 = 112;
const H_CLEAR: i32 = 113;

// 生词本控件 ID
const V_LIST: i32 = 201;
const V_REVIEW: i32 = 210;
const V_EXPORT_ANKI: i32 = 211;
const V_EXPORT_CSV: i32 = 212;
const V_DELETE: i32 = 213;

// 复习控件 ID
const R_WORD: i32 = 301;
const R_MEANING: i32 = 302;
const R_HINT: i32 = 303;
// 虚拟键（部分 winapi 版本未导出 VK_1/2/3）
const VK_1: i32 = 0x31;
const VK_2: i32 = 0x32;
const VK_3: i32 = 0x33;
const R_KNOW: i32 = 310;
const R_FUZZY: i32 = 311;
const R_FORGET: i32 = 312;

fn to_wide(s: &str) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    std::ffi::OsString::from(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

fn data_dir_file(name: &str) -> String {
    let dir = dirs::data_local_dir()
        .map(|p| p.join("WinOCR-Html"))
        .unwrap_or_else(|| std::path::PathBuf::from("WinOCR-Html"));
    let _ = std::fs::create_dir_all(&dir);
    dir.join(name).to_string_lossy().into_owned()
}

// ---------------------------------------------------------------------------
// 历史窗口
// ---------------------------------------------------------------------------

pub fn show_history() {
    unsafe { history_window_run() };
}

unsafe fn history_window_run() {
    let items = history::load_history();
    HIST_ITEMS.with(|i| *i.borrow_mut() = items);

    let cls = to_wide("WinOCRHistoryWnd");
    let wc = WNDCLASS_STUB(cls.clone());
    RegisterClassW(&wc);

    let hwnd = CreateWindowExW(
        0,
        cls.as_ptr(),
        to_wide(tr!("history_title")).as_ptr(),
        WS_OVERLAPPEDWINDOW,
        CW_USEDEFAULT, CW_USEDEFAULT, 620, 520,
        ptr::null_mut(),
        ptr::null_mut(),
        ptr::null_mut(),
        ptr::null_mut(),
    );
    if hwnd.is_null() {
        return;
    }

    let hfont = winapi::um::wingdi::GetStockObject(winapi::um::wingdi::DEFAULT_GUI_FONT as i32);

    create_static(hwnd, tr!("search"), 16, 12, 60, 22, -1, hfont);
    create_edit(hwnd, H_SEARCH, 80, 10, 200, 22, hfont);
    create_button(hwnd, "JSON", H_EXPORT_JSON, 300, 10, 70, 24, hfont);
    create_button(hwnd, "CSV", H_EXPORT_CSV, 376, 10, 70, 24, hfont);
    create_button(hwnd, "MD", H_EXPORT_MD, 452, 10, 70, 24, hfont);
    create_button(hwnd, tr!("clear_all"), H_CLEAR, 528, 10, 70, 24, hfont);

    let list = CreateWindowExW(
        0,
        to_wide("LISTBOX").as_ptr(),
        ptr::null(),
        WS_CHILD | WS_VISIBLE | LBS_HASSTRINGS | LBS_NOTIFY | WS_VSCROLL,
        16, 44, 588, 440,
        hwnd,
        H_LIST as *mut _,
        ptr::null_mut(),
        ptr::null_mut(),
    );
    set_font(list, hfont);
    refresh_history_list(hwnd);

    ShowWindow(hwnd, SW_SHOW);
    let mut msg: winapi::um::winuser::MSG = mem::zeroed();
    while GetMessageW(&mut msg, ptr::null_mut(), 0, 0) > 0 {
        TranslateMessage(&msg);
        DispatchMessageW(&msg);
        if msg.message == WM_DESTROY {
            break;
        }
    }
}

unsafe fn refresh_history_list(hwnd: HWND) {
    let items = HIST_ITEMS.with(|i| i.borrow().clone());
    let list = GetDlgItem(hwnd, H_LIST);
    if list.is_null() {
        return;
    }
    SendMessageW(list, LB_RESETCONTENT, 0, 0);
    // 搜索过滤
    let search = read_edit(hwnd, H_SEARCH);
    let q = search.to_lowercase();
    for it in &items {
        if !q.is_empty()
            && !it.src.to_lowercase().contains(&q)
            && !it.tgt.to_lowercase().contains(&q)
        {
            continue;
        }
        let line = format!(
            "{} | {} → {}",
            history::format_time(it.ts),
            it.src.chars().take(40).collect::<String>(),
            it.tgt.chars().take(40).collect::<String>()
        );
        SendMessageW(list, LB_ADDSTRING, 0, to_wide(&line).as_ptr() as LPARAM);
    }
}

unsafe extern "system" fn history_proc(
    hwnd: HWND,
    msg: UINT,
    wparam: WPARAM,
    lparam: LPARAM,
) -> isize {
    match msg {
        WM_COMMAND => {
            let id = (wparam as u16) as i32;
            match id {
                H_SEARCH => {
                    // EN_CHANGE（高字 0x0300）时刷新
                    if ((wparam >> 16) & 0xFFFF) as u32 == 0x0300 {
                        refresh_history_list(hwnd);
                    }
                }
                H_EXPORT_JSON => export_history(hwnd, "json"),
                H_EXPORT_CSV => export_history(hwnd, "csv"),
                H_EXPORT_MD => export_history(hwnd, "md"),
                H_CLEAR => {
                    if crate::window::show_confirm(hwnd, tr!("confirm_clear")) {
                        history::clear_history();
                        HIST_ITEMS.with(|i| i.borrow_mut().clear());
                        refresh_history_list(hwnd);
                    }
                }
                _ => {}
            }
            0
        }
        WM_CLOSE => {
            DestroyWindow(hwnd);
            0
        }
        WM_DESTROY => 0,
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn export_history(hwnd: HWND, fmt: &str) {
    let items = HIST_ITEMS.with(|i| i.borrow().clone());
    let ext = if fmt == "csv" { "csv" } else { "json" };
    let fname = format!("history.{}.{}", chrono_like_secs(), ext);
    let path = data_dir_file(&fname);
    let res = if fmt == "md" {
        history::export_history(&items, "md", &path)
    } else if fmt == "csv" {
        history::export_history(&items, "csv", &path)
    } else {
        history::export_history(&items, "json", &path)
    };
    match res {
        Ok(_) => {
            let m = format!("已导出到：\n{}", path);
            MessageBoxW(hwnd, to_wide(&m).as_ptr(), to_wide("导出成功").as_ptr(), 0x40);
        }
        Err(e) => {
            let m = format!("导出失败：{}", e);
            MessageBoxW(hwnd, to_wide(&m).as_ptr(), to_wide("错误").as_ptr(), 0x30);
        }
    }
}

// ---------------------------------------------------------------------------
// 生词本窗口
// ---------------------------------------------------------------------------

pub fn show_vocab() {
    unsafe { vocab_window_run() };
}

unsafe fn vocab_window_run() {
    let items = history::load_vocab();
    VOCAB_ITEMS.with(|i| *i.borrow_mut() = items);

    let cls = to_wide("WinOCRVocabWnd");
    let wc = WNDCLASS_STUB(cls.clone());
    RegisterClassW(&wc);

    let hwnd = CreateWindowExW(
        0,
        cls.as_ptr(),
        to_wide(tr!("vocab_title")).as_ptr(),
        WS_OVERLAPPEDWINDOW,
        CW_USEDEFAULT, CW_USEDEFAULT, 560, 520,
        ptr::null_mut(),
        ptr::null_mut(),
        ptr::null_mut(),
        ptr::null_mut(),
    );
    if hwnd.is_null() {
        return;
    }
    let hfont = winapi::um::wingdi::GetStockObject(winapi::um::wingdi::DEFAULT_GUI_FONT as i32);

        create_button(hwnd, tr!("review"), V_REVIEW, 16, 10, 90, 26, hfont);
        create_button(hwnd, tr!("export_anki"), V_EXPORT_ANKI, 116, 10, 100, 26, hfont);
        create_button(hwnd, &format!("{} CSV", tr!("export")), V_EXPORT_CSV, 224, 10, 100, 26, hfont);
        create_button(hwnd, tr!("clear_all"), V_DELETE, 332, 10, 90, 26, hfont);

    let list = CreateWindowExW(
        0,
        to_wide("LISTBOX").as_ptr(),
        ptr::null(),
        WS_CHILD | WS_VISIBLE | LBS_HASSTRINGS | LBS_NOTIFY | WS_VSCROLL,
        16, 48, 528, 440,
        hwnd,
        V_LIST as *mut _,
        ptr::null_mut(),
        ptr::null_mut(),
    );
    set_font(list, hfont);
    refresh_vocab_list(hwnd);

    ShowWindow(hwnd, SW_SHOW);
    let mut msg: winapi::um::winuser::MSG = mem::zeroed();
    while GetMessageW(&mut msg, ptr::null_mut(), 0, 0) > 0 {
        TranslateMessage(&msg);
        DispatchMessageW(&msg);
        if msg.message == WM_DESTROY {
            break;
        }
    }
}

unsafe fn refresh_vocab_list(hwnd: HWND) {
    let items = VOCAB_ITEMS.with(|i| i.borrow().clone());
    let list = GetDlgItem(hwnd, V_LIST);
    if list.is_null() {
        return;
    }
    SendMessageW(list, LB_RESETCONTENT, 0, 0);
    for it in &items {
        let line = format!("{}  —  {}", it.word, it.meaning);
        SendMessageW(list, LB_ADDSTRING, 0, to_wide(&line).as_ptr() as LPARAM);
    }
}

unsafe extern "system" fn vocab_proc(
    hwnd: HWND,
    msg: UINT,
    wparam: WPARAM,
    lparam: LPARAM,
) -> isize {
    match msg {
        WM_COMMAND => {
            let id = (wparam as u16) as i32;
            match id {
                V_REVIEW => {
                    show_review(hwnd);
                    // 复习后刷新待复习标记（这里简单刷新列表）
                    let items = history::load_vocab();
                    VOCAB_ITEMS.with(|i| *i.borrow_mut() = items);
                    refresh_vocab_list(hwnd);
                }
                V_EXPORT_ANKI => export_vocab(hwnd, "anki"),
                V_EXPORT_CSV => export_vocab(hwnd, "csv"),
                V_DELETE => {
                    let list = GetDlgItem(hwnd, V_LIST);
                    let sel = SendMessageW(list, LB_GETCURSEL, 0, 0) as i32;
                    if sel >= 0 {
                        let items = VOCAB_ITEMS.with(|i| i.borrow().clone());
                        if let Some(it) = items.get(sel as usize) {
                            history::remove_vocab(&it.id);
                            VOCAB_ITEMS.with(|i| i.borrow_mut().remove(sel as usize));
                            SendMessageW(list, LB_DELETESTRING, sel as usize, 0);
                        }
                    }
                }
                _ => {}
            }
            0
        }
        WM_CLOSE => {
            DestroyWindow(hwnd);
            0
        }
        WM_DESTROY => 0,
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn export_vocab(hwnd: HWND, fmt: &str) {
    let items = VOCAB_ITEMS.with(|i| i.borrow().clone());
    let fname = format!("vocab.{}.{}", chrono_like_secs(), fmt);
    let path = data_dir_file(&fname);
    let res = if fmt == "anki" {
        history::export_vocab_anki(&items, &path)
    } else {
        history::export_vocab_csv(&items, &path)
    };
    match res {
        Ok(_) => {
            let m = format!("已导出到：\n{}", path);
            MessageBoxW(hwnd, to_wide(&m).as_ptr(), to_wide("导出成功").as_ptr(), 0x40);
        }
        Err(e) => {
            let m = format!("导出失败：{}", e);
            MessageBoxW(hwnd, to_wide(&m).as_ptr(), to_wide("错误").as_ptr(), 0x30);
        }
    }
}

// ---------------------------------------------------------------------------
// 复习闪卡窗口（SM-2）
// ---------------------------------------------------------------------------

pub fn show_review(parent: HWND) {
    unsafe {
        let items = history::load_vocab();
        let due = history::due_vocab(&items);
        if due.is_empty() {
            MessageBoxW(
                parent,
                to_wide(tr!("no_due")).as_ptr(),
                to_wide(tr!("review_title")).as_ptr(),
                0x40,
            );
            return;
        }
        REVIEW_QUEUE.with(|q| *q.borrow_mut() = due);
        REVIEW_SHOW_FRONT.with(|f| *f.borrow_mut() = true);

        let cls = to_wide("WinOCRReviewWnd");
        let wc = WNDCLASS_STUB(cls.clone());
        RegisterClassW(&wc);

        let hwnd = CreateWindowExW(
            0,
            cls.as_ptr(),
            to_wide(tr!("review_title")).as_ptr(),
            WS_OVERLAPPEDWINDOW & !0x00080000 & !0x00040000,
            CW_USEDEFAULT, CW_USEDEFAULT, 420, 300,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
        );
        if hwnd.is_null() {
            return;
        }
        let hfont = winapi::um::wingdi::GetStockObject(winapi::um::wingdi::DEFAULT_GUI_FONT as i32);

        let word = CreateWindowExW(
            0,
            to_wide("STATIC").as_ptr(),
            ptr::null(),
            WS_CHILD | WS_VISIBLE,
            20, 30, 380, 40,
            hwnd, R_WORD as *mut _,
            ptr::null_mut(), ptr::null_mut(),
        );
        let meaning = CreateWindowExW(
            0,
            to_wide("STATIC").as_ptr(),
            ptr::null(),
            WS_CHILD | WS_VISIBLE,
            20, 90, 380, 80,
            hwnd, R_MEANING as *mut _,
            ptr::null_mut(), ptr::null_mut(),
        );
        let hint = create_static(hwnd, tr!("flip_hint"), 20, 230, 380, 24, R_HINT, hfont);
        set_font(word, hfont);
        set_font(meaning, hfont);
        set_font(hint, hfont);
        create_button(hwnd, tr!("know"), R_KNOW, 40, 250, 90, 28, hfont);
        create_button(hwnd, tr!("fuzzy"), R_FUZZY, 165, 250, 90, 28, hfont);
        create_button(hwnd, tr!("forget"), R_FORGET, 290, 250, 90, 28, hfont);

        render_review_card(hwnd);

        ShowWindow(hwnd, SW_SHOW);
        let mut msg: winapi::um::winuser::MSG = mem::zeroed();
        while GetMessageW(&mut msg, ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
            if msg.message == WM_DESTROY {
                break;
            }
        }
    }
}

/// 渲染当前闪卡（正面显示 word，反面显示 meaning）
unsafe fn render_review_card(hwnd: HWND) {
    let queue = REVIEW_QUEUE.with(|q| q.borrow().clone());
    if queue.is_empty() {
        // 全部复习完，关闭
        DestroyWindow(hwnd);
        return;
    }
    let cur = &queue[0];
    let show_front = REVIEW_SHOW_FRONT.with(|f| *f.borrow());
    let word_w = GetDlgItem(hwnd, R_WORD);
    let mean_w = GetDlgItem(hwnd, R_MEANING);
    if show_front {
        SetWindowTextW(word_w, to_wide(&cur.word).as_ptr());
        SetWindowTextW(mean_w, to_wide("? ? ?").as_ptr());
    } else {
        SetWindowTextW(mean_w, to_wide(&cur.meaning).as_ptr());
    }
}

unsafe extern "system" fn review_proc(
    hwnd: HWND,
    msg: UINT,
    wparam: WPARAM,
    lparam: LPARAM,
) -> isize {
    match msg {
        WM_COMMAND => {
            let id = (wparam as u16) as i32;
            // 三个评分按钮：认识=4 / 模糊=2 / 忘记=0（SM-2 quality）
            let quality: i32 = match id {
                R_KNOW => 4,
                R_FUZZY => 2,
                R_FORGET => 0,
                _ => -1,
            };
            if quality >= 0 {
                grade_current(hwnd, quality as u8);
            }
            0
        }
        WM_KEYDOWN => {
            match wparam as i32 {
                VK_SPACE => {
                    let f = !REVIEW_SHOW_FRONT.with(|x| *x.borrow());
                    REVIEW_SHOW_FRONT.with(|x| *x.borrow_mut() = f);
                    render_review_card(hwnd);
                }
                VK_1 => grade_current(hwnd, 4),
                VK_2 => grade_current(hwnd, 2),
                VK_3 => grade_current(hwnd, 0),
                VK_ESCAPE => {
                    DestroyWindow(hwnd);
                }
                _ => {}
            }
            0
        }
        WM_CLOSE => {
            DestroyWindow(hwnd);
            0
        }
        WM_DESTROY => 0,
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// 给当前闪卡打分并推进
unsafe fn grade_current(hwnd: HWND, quality: u8) {
    let mut queue = REVIEW_QUEUE.with(|q| q.borrow().clone());
    if let Some(cur) = queue.first().cloned() {
        history::review_grade(&cur, quality);
        queue.remove(0);
        REVIEW_QUEUE.with(|q| *q.borrow_mut() = queue);
        REVIEW_SHOW_FRONT.with(|f| *f.borrow_mut() = true);
        render_review_card(hwnd);
    }
}

// ---------------------------------------------------------------------------
// 控件辅助
// ---------------------------------------------------------------------------

unsafe fn WNDCLASS_STUB(cls: Vec<u16>) -> winapi::um::winuser::WNDCLASSW {
    // 根据类名选择窗口过程
    let proc = if cls == to_wide("WinOCRHistoryWnd") {
        history_proc
    } else if cls == to_wide("WinOCRVocabWnd") {
        vocab_proc
    } else {
        review_proc
    };
    winapi::um::winuser::WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(proc),
        hInstance: ptr::null_mut(),
        hCursor: LoadCursorW(ptr::null_mut(), IDC_ARROW),
        hbrBackground: winapi::um::wingdi::GetStockObject(6 /* COLOR_WINDOW + 1 */) as *mut _,
        lpszClassName: cls.as_ptr(),
        lpszMenuName: ptr::null_mut(),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hIcon: ptr::null_mut(),
    }
}

unsafe fn create_static(
    parent: HWND,
    text: &str,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    id: i32,
    hfont: *mut std::ffi::c_void,
) -> HWND {
    let hwnd = CreateWindowExW(
        0,
        to_wide("STATIC").as_ptr(),
        to_wide(text).as_ptr(),
        WS_CHILD | WS_VISIBLE,
        x, y, w, h,
        parent, id as *mut _,
        ptr::null_mut(), ptr::null_mut(),
    );
    set_font(hwnd, hfont);
    hwnd
}

unsafe fn create_edit(
    parent: HWND,
    id: i32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    hfont: *mut std::ffi::c_void,
) -> HWND {
    let hwnd = CreateWindowExW(
        0,
        to_wide("EDIT").as_ptr(),
        ptr::null(),
        WS_CHILD | WS_VISIBLE | winapi::um::winuser::WS_BORDER,
        x, y, w, h,
        parent, id as *mut _,
        ptr::null_mut(), ptr::null_mut(),
    );
    set_font(hwnd, hfont);
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
    hfont: *mut std::ffi::c_void,
) -> HWND {
    let hwnd = CreateWindowExW(
        0,
        to_wide("BUTTON").as_ptr(),
        to_wide(text).as_ptr(),
        WS_CHILD | WS_VISIBLE | BS_PUSHBUTTON,
        x, y, w, h,
        parent, id as *mut _,
        ptr::null_mut(), ptr::null_mut(),
    );
    set_font(hwnd, hfont);
    hwnd
}

unsafe fn set_font(hwnd: HWND, hfont: *mut std::ffi::c_void) {
    if !hfont.is_null() {
        SendMessageW(hwnd, winapi::um::winuser::WM_SETFONT, hfont as WPARAM, 1);
    }
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

fn chrono_like_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
