fn main() {
    #[cfg(windows)]
    build_sqlite();
    tauri_build::build();
}

#[cfg(windows)]
fn build_sqlite() {
    println!("cargo:rerun-if-env-changed=SEMANTIC_READER_SQLITE_SOURCE");
    let source = std::env::var_os("SEMANTIC_READER_SQLITE_SOURCE")
        .expect("Run scripts/package-windows.ps1 to prepare SQLite");
    let source = std::path::PathBuf::from(source);
    println!("cargo:rerun-if-changed={}", source.display());
    cc::Build::new()
        .file(source)
        .define("SQLITE_ENABLE_FTS5", None)
        .define("SQLITE_OMIT_LOAD_EXTENSION", None)
        .compile("sqlite3");
}
