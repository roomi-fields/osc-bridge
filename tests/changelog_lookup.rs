//! OB-iyd.1: the tests that read the release notes find their entry in any
//! section. The helper (`tests/common/changelog.rs`) is the only reader; this
//! file tests it on fake notes and guards that no other file of `tests/`
//! reads them.

mod common;

use common::changelog::section_of;
use std::path::{Path, PathBuf};

fn open_heading() -> String {
    format!("[{}]", ["Unre", "leased"].concat())
}

fn notes(open: &str, v19: &str, v18: &str) -> String {
    format!(
        "# Notes\n\n## {}\n\n{open}\n## [0.19.0] — 2026-10-31\n\n{v19}\n## [0.18.0] — 2026-09-01\n\n{v18}\n",
        open_heading()
    )
}

fn failure(words: &[&str], text: &str) -> String {
    let words = words.to_vec();
    let text = text.to_string();
    let err = std::panic::catch_unwind(move || section_of(&text, &words)).unwrap_err();
    err.downcast_ref::<String>().cloned().or_else(|| err.downcast_ref::<&str>().map(|s| s.to_string())).unwrap()
}

// ---- criteria 1 and 6: a section from a string, never the whole file ----

#[test]
fn criterion_6_finds_the_entry_under_the_open_section_and_returns_only_that_section() {
    let n = notes("- alpha beta entry\n", "- gamma\n", "- delta\n");
    let s = section_of(&n, &["alpha", "beta"]);
    assert!(s.contains("alpha beta") && s.starts_with("## "), "{s}");
    assert!(!s.contains("gamma") && !s.contains("delta"), "only its section:\n{s}");
}

#[test]
fn criterion_6_finds_the_entry_under_a_version() {
    let n = notes("", "- alpha beta entry\n", "- delta\n");
    let s = section_of(&n, &["alpha", "beta"]);
    assert!(s.contains("[0.19.0]") && !s.contains("delta"), "{s}");
}

#[test]
fn criterion_1_the_real_notes_have_sections_and_the_helper_reads_them() {
    let s = common::changelog::section_with_words(&["Fixed"]);
    assert!(s.starts_with("## "), "{s}");
}

#[test]
fn criterion_1_fails_naming_the_words_when_none_holds_them() {
    let m = failure(&["nowhere-word", "other-word"], &notes("- a\n", "- b\n", "- c\n"));
    assert!(m.contains("nowhere-word") && m.contains("other-word"), "{m}");
}

// ---- criterion 4: the first section in file order ----

#[test]
fn criterion_4_returns_the_first_section_in_file_order_when_several_hold_the_words() {
    let n = notes("- shared words here\n", "- shared words here\n", "- shared words here\n");
    let s = section_of(&n, &["shared", "words"]);
    assert!(s.contains(&open_heading()), "newest first:\n{s}");
}

#[test]
fn criterion_4_an_older_entry_with_the_same_generic_words_hides_the_missing_newer_one() {
    // generic words: the old version holds them, so the lookup passes on the old entry.
    let n = notes("", "", "- checksum loads\n");
    assert!(section_of(&n, &["checksum"]).contains("[0.18.0]"));
    // words specific to the new entry: the older version does not hold them, the lookup fails.
    let m = failure(&["checksum", "newentryword"], &n);
    assert!(m.contains("newentryword"), "{m}");
}

// ---- criterion 5: words spread over two sections do not pass ----

#[test]
fn criterion_5_words_spread_over_two_sections_fail_naming_all_and_the_missing() {
    let n = notes("- alpha\n", "- beta\n", "- gamma\n");
    let m = failure(&["alpha", "beta"], &n);
    assert!(m.contains("alpha") && m.contains("beta"), "every word:\n{m}");
    let m = failure(&["alpha", "beta", "gamma"], &notes("- alpha beta\n", "- gamma\n", "- x\n"));
    let (_, lacks) = m.split_once("lacks").expect("the message names what the best section lacks");
    assert!(lacks.contains("gamma"), "the word missing from the best section:\n{m}");
    assert!(
        !lacks.contains("alpha") && !lacks.contains("beta"),
        "the best section holds alpha and beta, so it does not lack them:\n{m}"
    );
}

// ---- criterion 9: the callers' lookup passes before and after a release ----

#[test]
fn criterion_9_the_lookup_passes_with_the_entry_under_the_open_section_and_under_a_version() {
    let words = ["PRIVACY.md says what the service keeps of a call"];
    for n in [
        notes("### Changed — PRIVACY.md says what the service keeps of a call\n", "- x\n", "- y\n"),
        notes("", "### Changed — PRIVACY.md says what the service keeps of a call\n", "- y\n"),
    ] {
        section_of(&n, &words);
    }
}

// ---- criteria 2, 3 and 8: the guard ----

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            rs_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// The text of the string literals of `src`, line by line: the literals of a
/// line are joined, so a name split in pieces and rebuilt (a concat, a join)
/// on one line is seen whole.
/// Limit of the guard: a path built by other means (an environment variable,
/// a directory walk), or split in pieces over several lines, is not seen.
fn literals_per_line(src: &str) -> Vec<String> {
    src.lines()
        .map(|line| {
            let mut out = String::new();
            let mut in_str = false;
            let mut esc = false;
            for c in line.chars() {
                if in_str {
                    if esc {
                        esc = false;
                    } else if c == '\\' {
                        esc = true;
                    } else if c == '"' {
                        in_str = false;
                    } else {
                        out.push(c);
                    }
                } else if c == '"' {
                    in_str = true;
                }
            }
            out
        })
        .collect()
}

fn reads_the_notes(src: &str) -> bool {
    literals_per_line(src).iter().any(|l| l.to_lowercase().contains("changelog"))
}

#[test]
fn criterion_8_the_guard_flags_the_literal_and_the_split_forms() {
    assert!(reads_the_notes(r#"std::fs::read_to_string("CHANGELOG.md")"#));
    assert!(reads_the_notes(r#"read("changelog.md")"#));
    assert!(reads_the_notes(r#"let p = ["CHANGE", "LOG.md"].concat();"#));
    assert!(reads_the_notes(r#"let p = ["CHANGE", "LOG"].join("") + ".md";"#));
    assert!(reads_the_notes(r#"let p = String::from("Change") + "log.md";"#));
    assert!(!reads_the_notes("fn the_changelog_test() { read(\"docs/X.md\") } // CHANGELOG"));
}

#[test]
fn criterion_3_no_file_of_tests_but_the_helper_reads_the_notes() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut files = Vec::new();
    rs_files(&root, &mut files);
    assert!(files.len() > 10, "the scan reaches the files: {}", files.len());
    let helper = root.join("common").join("changelog.rs");
    let me = root.join("changelog_lookup.rs");
    let offenders: Vec<_> = files
        .iter()
        .filter(|f| **f != helper && **f != me)
        .filter(|f| reads_the_notes(&std::fs::read_to_string(f).unwrap()))
        .map(|f| f.strip_prefix(&root).unwrap().display().to_string())
        .collect();
    assert!(offenders.is_empty(), "these files read the release notes without the helper: {offenders:?}");
}
