// 独立的 Slint 编译检查器：只做语法/类型检查，不生成代码、不链接。
// 目的：改完 .slint 能在秒级得到带行号的诊断，不必等 cargo run 的完整构建。
//
// 用法：
//   cargo run --quiet --manifest-path tools/slint-check/Cargo.toml -- app/src/ui/main_window.slint
//
// 退出码：0 = 通过，1 = 有编译错误
use std::path::PathBuf;
use std::process::exit;

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "app/src/ui/main_window.slint".to_string());
    // 必须转绝对路径：slint_build::compile 会以 CARGO_MANIFEST_DIR 为基准解析相对路径，
    // 而我们把 CARGO_MANIFEST_DIR 指向了 tools/slint-check。
    let p = std::env::current_dir()
        .map(|d| d.join(&path))
        .unwrap_or_else(|_| PathBuf::from(&path));
    if !p.exists() {
        eprintln!("[slint-check] 文件不存在: {}", p.display());
        exit(2);
    }

    // slint_build::compile 硬性要求这两个环境变量（它要写 OUT_DIR 下的生成文件）。
    // 本工具独立于 workspace 运行，没有 build script，所以自己补上。
    let manifest = std::env::current_dir()
        .map(|d| d.join("tools/slint-check"))
        .unwrap_or_else(|_| PathBuf::from("."));
    let out = manifest.join("target/slint-check-out");
    let _ = std::fs::create_dir_all(&out);
    std::env::set_var("CARGO_MANIFEST_DIR", &manifest);
    std::env::set_var("OUT_DIR", &out);

    match slint_build::compile(&p) {
        Ok(()) => {
            println!("[slint-check] OK  {}", p.display());
        }
        Err(e) => {
            match e {
                slint_build::CompileError::CompileError(diags) => {
                    eprintln!("[slint-check] FAIL  {}  ({} 条诊断)", p.display(), diags.len());
                    for d in &diags {
                        eprintln!("  {d}");
                    }
                }
                other => {
                    eprintln!("[slint-check] FAIL  {}: {other}", p.display());
                }
            }
            exit(1);
        }
    }
}
