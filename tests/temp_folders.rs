//! A test's temporary folder (`common::temp`) is removed when the test ends,
//! and when it fails: the system's temporary folder may be a memory disk
//! that every program of the machine shares.

mod common;

use std::path::{Path, PathBuf};

#[test]
fn a_failed_test_leaves_no_temporary_folder() {
    let mut made = PathBuf::new();
    let failed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let dir = common::temp("failing");
        let path: &Path = dir.as_ref();
        made = path.to_path_buf();
        std::fs::write(path.join("file"), "kept?").unwrap();
        panic!("the test fails");
    }));
    assert!(failed.is_err());
    assert!(made.starts_with(std::env::temp_dir()), "{}", made.display());
    assert!(!made.exists(), "{} is left behind", made.display());
}
