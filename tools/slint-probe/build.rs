fn main() {
    // PROBE_SLINT 相对仓库根；build script 的 CWD 是 crate 目录，故按 manifest 目录解析。
    // ⚠️ 必须同时 rerun-if-env-changed：cargo 默认不跟踪环境变量，
    //    切换 PROBE_SLINT 目标文件时若不声明，会静默复用旧的生成代码（探针渲染出旧布局）。
    println!("cargo:rerun-if-env-changed=PROBE_SLINT");
    let rel = std::env::var("PROBE_SLINT").unwrap_or_else(|_| {
        "../../app/src/ui/main_window.slint".to_string()
    });
    let p = if std::path::Path::new(&rel).is_absolute() {
        std::path::PathBuf::from(&rel)
    } else {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(&rel)
            .canonicalize()
            .unwrap_or_else(|_| std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(&rel))
    };
    slint_build::compile(&p).expect("slint compile");
    println!("cargo:rerun-if-changed={}", p.display());
}