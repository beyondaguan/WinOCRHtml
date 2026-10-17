// ============================================================
// Services · 剪贴板读取（公共模块）
// 为 UI 层提供 get_text / get_image / set_text。
// Windows：通过 Win32 API（CF_UNICODETEXT / CF_DIB）实现；
// 非 Windows：恒返回 None / false。
// ============================================================

/// 从剪贴板读取文本。无文本或失败返回 None。
#[cfg(windows)]
pub fn get_text() -> Option<String> {
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, GetClipboardData,
    };
    use windows_sys::Win32::System::Memory::{GlobalLock, GlobalUnlock};
    use windows_sys::Win32::System::Ole::CF_UNICODETEXT;

    if !clipboard_open_retry() {
        return None;
    }
    let mut out = None;
    unsafe {
        let h = GetClipboardData(CF_UNICODETEXT as u32);
        if !h.is_null() {
            let p = GlobalLock(h) as *const u16;
            if !p.is_null() {
                let mut len = 0usize;
                while *p.add(len) != 0 {
                    len += 1;
                }
                let slice = std::slice::from_raw_parts(p, len);
                out = Some(String::from_utf16_lossy(slice));
                let _ = GlobalUnlock(h);
            }
        }
        CloseClipboard();
    }
    out
}

#[cfg(not(windows))]
pub fn get_text() -> Option<String> {
    None
}

/// 从剪贴板读取图片（CF_DIB 位图），返回 (紧凑RGB, 宽, 高)。
/// 无图片或失败返回 None。
#[cfg(windows)]
pub fn get_image() -> Option<(Vec<u8>, u32, u32)> {
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, GetClipboardData,
    };
    use windows_sys::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
    use windows_sys::Win32::System::Ole::CF_DIB;

    if !clipboard_open_retry() {
        return None;
    }

    let result = unsafe {
        let h = GetClipboardData(CF_DIB as u32);
        if h.is_null() {
            CloseClipboard();
            return None;
        }
        let p = GlobalLock(h) as *const u8;
        if p.is_null() {
            CloseClipboard();
            return None;
        }
        let size = GlobalSize(h);

        // 复制 DIB 数据到 Vec（必须在 Unlock 前复制出来）
        let data = std::slice::from_raw_parts(p, size).to_vec();
        let _ = GlobalUnlock(h);
        CloseClipboard();

        parse_dib_to_rgb(&data)
    };

    Some(result)
}

#[cfg(windows)]
fn parse_dib_to_rgb(data: &[u8]) -> (Vec<u8>, u32, u32) {
    if data.len() < 40 {
        return (Vec::new(), 0, 0);
    }

    // 解析 BITINFOHEADER（40 字节）
    let header_size = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    if header_size < 40 {
        return (Vec::new(), 0, 0);
    }

    let width = i32::from_le_bytes([data[4], data[5], data[6], data[7]]);
    let height_raw = i32::from_le_bytes([data[8], data[9], data[10], data[11]]);
    let height = height_raw.abs() as u32;
    let planes = u16::from_le_bytes([data[12], data[13]]);
    let bpp = u16::from_le_bytes([data[14], data[15]]);
    let compression = u32::from_le_bytes([data[16], data[17], data[18], data[19]]);

    if planes != 1 || (bpp != 24 && bpp != 32) || compression != 0 {
        return (Vec::new(), 0, 0);
    }

    let width = width as u32;
    if width == 0 || height == 0 {
        return (Vec::new(), 0, 0);
    }

    // 计算调色板大小
    let clr_used = u32::from_le_bytes([data[32], data[33], data[34], data[35]]);
    let palette_entries = if clr_used == 0 { 0 } else { clr_used as usize };
    let pixel_offset = header_size as usize + palette_entries * 4;

    // 提取像素数据（DIB 格式：BGR 或 BGRA，自底向上行序）
    let bytes_per_pixel = bpp as usize / 8;
    let row_size = ((width as usize * bytes_per_pixel + 3) / 4) * 4; // 行对齐到 4 字节
    let mut rgb: Vec<u8> = Vec::with_capacity((width * height * 3) as usize);

    for y in 0..height as usize {
        // DIB 是底行在前的，所以倒序读取
        let row_idx = if height_raw > 0 {
            height as usize - 1 - y // bottom-up
        } else {
            y // top-down
        };
        let row_start = pixel_offset + row_idx * row_size;
        if row_start + width as usize * bytes_per_pixel > data.len() {
            break;
        }
        for x in 0..width as usize {
            let offset = row_start + x * bytes_per_pixel;
            let b = data[offset];
            let g = data[offset + 1];
            let r = data[offset + 2];
            rgb.push(r);
            rgb.push(g);
            rgb.push(b);
        }
    }

    (rgb, width, height)
}

#[cfg(not(windows))]
pub fn get_image() -> Option<(Vec<u8>, u32, u32)> {
    None
}

/// 写入文本到剪贴板。返回 false 表示失败。
#[cfg(windows)]
pub fn set_text(text: &str) -> bool {
    use windows_sys::Win32::Foundation::GlobalFree;
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, SetClipboardData,
    };
    use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
    use windows_sys::Win32::System::Ole::CF_UNICODETEXT;

    if !clipboard_open_retry() {
        return false;
    }
    unsafe {
        EmptyClipboard();
        let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        let bytes = wide.len() * 2;
        let h = GlobalAlloc(GMEM_MOVEABLE, bytes);
        if h.is_null() {
            CloseClipboard();
            return false;
        }
        let p = GlobalLock(h) as *mut u16;
        if p.is_null() {
            GlobalFree(h);
            CloseClipboard();
            return false;
        }
        std::ptr::copy_nonoverlapping(wide.as_ptr(), p, wide.len());
        let _ = GlobalUnlock(h);
        let ok = !SetClipboardData(CF_UNICODETEXT as u32, h).is_null();
        if !ok {
            GlobalFree(h);
        }
        CloseClipboard();
        ok
    }
}

#[cfg(not(windows))]
pub fn set_text(_text: &str) -> bool {
    false
}

#[cfg(windows)]
fn clipboard_open_retry() -> bool {
    use std::ptr;
    use std::time::Duration;
    use windows_sys::Win32::System::DataExchange::OpenClipboard;

    for _ in 0..5 {
        if unsafe { OpenClipboard(ptr::null_mut()) } != 0 {
            return true;
        }
        std::thread::sleep(Duration::from_millis(15));
    }
    log::warn!("[services::clipboard] OpenClipboard 重试 5 次仍失败");
    false
}
