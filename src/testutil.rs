//! 测试用临时目录。统一建在项目的 `target/test-tmp` 下，不落到系统临时目录，也不访问真实 `$HOME`。

use std::path::PathBuf;

/// 在 `target/test-tmp` 下创建一个临时目录，测试结束后自动删除。
pub(crate) fn tempdir() -> tempfile::TempDir {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("test-tmp");
    std::fs::create_dir_all(&root).expect("create target/test-tmp");
    tempfile::Builder::new()
        .prefix("case-")
        .tempdir_in(&root)
        .expect("create temp dir under target/test-tmp")
}
