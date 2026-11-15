//! The integration tests of the crate are one executable: every file of
//! `tests/it/` is a module that `tests/it/main.rs` declares, and nothing else
//! under `tests/` is an executable of its own.

use std::collections::BTreeSet;
use std::path::Path;

const MAIN: &str = include_str!("main.rs");

fn rs_names(dir: &Path) -> BTreeSet<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "rs"))
        .map(|p| p.file_stem().unwrap().to_string_lossy().into_owned())
        .collect()
}

fn declared(source: &str) -> BTreeSet<String> {
    source
        .lines()
        .filter_map(|l| l.trim().strip_prefix("mod "))
        .filter_map(|l| l.strip_suffix(';'))
        .map(|n| n.trim().to_string())
        .collect()
}

#[test]
fn every_file_of_the_executable_is_declared_as_a_module_in_main() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/it");
    let mut files = rs_names(&dir);
    files.remove("main");
    let mut mods = declared(MAIN);
    mods.remove("common");
    let undeclared: Vec<_> = files.difference(&mods).collect();
    let missing: Vec<_> = mods.difference(&files).collect();
    assert!(undeclared.is_empty(), "files not declared as `mod <name>;` in tests/it/main.rs: {undeclared:?}");
    assert!(missing.is_empty(), "modules declared in tests/it/main.rs with no file: {missing:?}");
}

#[test]
fn the_shared_helpers_are_declared_once_in_main_and_by_no_module() {
    assert_eq!(MAIN.matches("#[path = \"../common/mod.rs\"]").count(), 1, "one #[path] declaration of common in main.rs");
    assert_eq!(MAIN.lines().filter(|l| l.trim() == "mod common;").count(), 1);
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/it");
    for name in rs_names(&dir).into_iter().filter(|n| n != "main") {
        let src = std::fs::read_to_string(dir.join(format!("{name}.rs"))).unwrap();
        assert!(!src.lines().any(|l| l.trim() == "mod common;"), "{name} redeclares `mod common;`");
    }
}

#[test]
fn no_file_at_the_root_of_tests_is_an_executable_of_its_own() {
    let tests = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let strays = rs_names(&tests);
    assert!(strays.is_empty(), "integration test files outside tests/it/: {strays:?}");
}
