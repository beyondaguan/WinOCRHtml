// ============================================================
// 构建脚本
//   1) 编译 Slint 声明式 UI（src/ui/main_window.slint）为 Rust 代码。
//   2) P4-1 随包资产：release 构建时把运行时资产同步到 exe 同级目录，
//      使 target/release/ 直接成为可分发的绿色目录（离线 OCR + 离线翻译随包）。
//
// 运行时资产位于**仓库根** assets/（app/ 的上一级），约 190MB，故仅 release 同步，
// 且按「目标缺失或大小不同才复制」跳过已同步文件，避免每次构建重复拷贝 190MB。
// 目录约定（与 services 中的解析逻辑一致）：
//   <exe_dir>/assets/models/ocr/*.onnx + ppocr_dict.txt    （services/ocr.rs）
//   <exe_dir>/assets/onnxruntime.dll                       （services/ocr.rs, ORT_DYLIB_PATH）
//   <exe_dir>/assets/argos_packages/                       （services/translate.rs）
//   <exe_dir>/tray.ico                                     （services/tray.rs）
//   <exe_dir>/winocr-argos.exe                             （services/translate.rs 子进程）
// ============================================================
use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=src/ui/main_window.slint");
    println!("cargo:rerun-if-changed=src/ui/study_window.slint"); // 单词学习独立窗口（手工维护）
    println!("cargo:rerun-if-changed=src/ui/bubble.slint");
    // 不用 unwrap()：slint 语法/类型错误会 panic 成一条难以定位的 build-script 堆栈。
    // 这里显式打印编译器诊断（含行号）并退出，让 cargo 正常报错。
    if let Err(e) = slint_build::compile("src/ui/main_window.slint") {
        // CompileError 是 enum：解析诊断在 CompileError::CompileError(Vec<String>) 变体里，
        // 其余变体（NotRunViaCargo / SaveError）用 Display 打印即可。
        match e {
            slint_build::CompileError::CompileError(diags) => {
                for d in &diags {
                    println!("cargo:warning={d}");
                }
            }
            other => println!("cargo:warning=main_window.slint 编译失败：{other}"),
        }
        std::process::exit(1);
    }

    let manifest_dir =
        PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR 未设置"));
    let repo_root = manifest_dir.parent().map(Path::to_path_buf).unwrap_or_default();
    let assets_src = repo_root.join("assets");

    // 资产内容变更时重新触发本脚本（首次同步后很少变化）
    println!("cargo:rerun-if-changed={}", assets_src.display());

    // 仅 release 构建做资产同步；debug 构建保持轻量
    // （debug 下运行时靠 cwd/../assets 命中仓库根资产，无需拷贝）。
    if std::env::var("PROFILE").as_deref() != Ok("release") {
        return;
    }

    let Ok(out_dir) = std::env::var("OUT_DIR").map(PathBuf::from) else {
        return;
    };
    // OUT_DIR = <target>/<profile>/build/winocr-<hash>/out  →  <target>/<profile>
    let Some(exe_dir) = out_dir.ancestors().nth(3).map(Path::to_path_buf) else {
        println!("cargo:warning=[P4-1] 无法解析 exe 输出目录，跳过资产同步");
        return;
    };

    // 1) 资产树 → <exe_dir>/assets/**
    match sync_tree(&assets_src, &exe_dir.join("assets")) {
        Ok(0) => {}
        Ok(n) => println!(
            "cargo:warning=[P4-1] 随包 assets 已同步（新复制 {n} 个文件）→ {}",
            exe_dir.display()
        ),
        Err(e) => println!("cargo:warning=[P4-1] assets 同步失败: {e}"),
    }

    // 2) tray.ico 固定按 <exe_dir>/tray.ico 查找（services/tray.rs）
    let _ = copy_if_diff(&assets_src.join("tray.ico"), &exe_dir.join("tray.ico"));

    // 3) 离线翻译子进程 winocr-argos.exe（argos-bridge 独立构建，不在本 crate 依赖图内）
    let argos_dst = exe_dir.join("winocr-argos.exe");
    if !argos_dst.exists() {
        let found = ["target", "target2", "target3", "target4", "target5"]
            .iter()
            .map(|d| {
                repo_root
                    .join("argos-bridge")
                    .join(d)
                    .join("release")
                    .join("winocr-argos.exe")
            })
            .find(|p| p.exists());
        match found {
            Some(src) => {
                if copy_if_diff(&src, &argos_dst) {
                    println!("cargo:warning=[P4-1] 已随包 winocr-argos.exe（离线翻译子进程）");
                }
            }
            None => println!(
                "cargo:warning=[P4-1] 未找到 winocr-argos.exe，离线翻译将走在线回退链。\
                 请先构建：cd argos-bridge && cargo build --release"
            ),
        }
    }
}

/// 递归同步目录树；仅复制「目标缺失或大小不同」的文件，返回新复制的文件数。
fn sync_tree(src: &Path, dst: &Path) -> std::io::Result<usize> {
    if !src.is_dir() {
        return Ok(0);
    }
    std::fs::create_dir_all(dst)?;
    let mut copied = 0usize;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let ft = entry.file_type()?;
        if ft.is_dir() {
            copied += sync_tree(&from, &to)?;
        } else if ft.is_file() && copy_if_diff(&from, &to) {
            copied += 1;
        }
    }
    Ok(copied)
}

/// 复制文件（目标缺失或大小不同才复制）；返回是否真的发生了复制。
fn copy_if_diff(src: &Path, dst: &Path) -> bool {
    if !src.is_file() {
        return false;
    }
    let already_same = match (std::fs::metadata(src), std::fs::metadata(dst)) {
        (Ok(a), Ok(b)) => a.len() == b.len(),
        _ => false,
    };
    if already_same {
        return false;
    }
    if let Some(parent) = dst.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match std::fs::copy(src, dst) {
        Ok(_) => true,
        Err(e) => {
            println!(
                "cargo:warning=[P4-1] 复制失败 {} -> {}: {e}",
                src.display(),
                dst.display()
            );
            false
        }
    }
}
