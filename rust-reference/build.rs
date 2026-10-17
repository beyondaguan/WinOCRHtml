use std::env;
use std::fs;
use std::path::Path;

// 构建期把本地 OCR 模型与 onnxruntime.dll 拷到 profile 输出目录，
// 让 release 产物开箱即用（消除首启 "OCR 模型未配置"）。
fn copy_dir(src: &Path, dst: &Path) {
    if fs::create_dir_all(dst).is_err() {
        return;
    }
    if let Ok(entries) = fs::read_dir(src) {
        for e in entries.flatten() {
            let p = e.path();
            if p.is_file() {
                if let Some(name) = p.file_name() {
                    let _ = fs::copy(&p, dst.join(name));
                }
            }
        }
    }
}

fn main() {
    if let (Ok(manifest), Ok(out_dir)) = (env::var("CARGO_MANIFEST_DIR"), env::var("OUT_DIR")) {
        // OUT_DIR = target/<profile>/build/<pkg>/out  → 上两级即 target/<profile>
        let profile_dir = Path::new(&out_dir).join("..").join("..");
        let profile_dir = profile_dir.canonicalize().unwrap_or_else(|_| profile_dir.to_path_buf());
        let models_src = Path::new(&manifest).join("models");
        if models_src.exists() {
            let dll = models_src.join("onnxruntime.dll");
            if dll.exists() {
                let dst = profile_dir.join("onnxruntime.dll");
                if !dst.exists() {
                    let _ = fs::copy(&dll, &dst);
                }
            }
            let ocr_src = models_src.join("ocr");
            if ocr_src.exists() {
                copy_dir(&ocr_src, &profile_dir.join("models").join("ocr"));
            }
        }
        // 托盘图标：从仓库根 tray.ico 拷到 profile 输出目录，供 tray.rs LoadImageW 加载（彩色醒目图标）
        let tray_src = Path::new(&manifest).join("..").join("tray.ico");
        if tray_src.exists() {
            let _ = fs::copy(&tray_src, profile_dir.join("tray.ico"));
        }
    }
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=models");
    println!("cargo:rerun-if-changed=../tray.ico");
}
