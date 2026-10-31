//! PRIVACY.md says what the service keeps of a local copy's forwarded calls
//! (OB-vch.1.3): the call, its outcome, a clipped error message, its
//! duration, the search words with or without an account; the 6 months of
//! detail; the daily totals that stay, without visitor or account.

use std::path::Path;

mod common;

fn read(name: &str) -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(name)).unwrap()
}

/// The text under the heading `## <title>`, up to the next `## `.
fn section(doc: &str, title: &str) -> String {
    let start = doc
        .find(&format!("## {title}"))
        .unwrap_or_else(|| panic!("PRIVACY.md has no section `{title}`"));
    let rest = &doc[start + 3..];
    let end = rest.find("\n## ").unwrap_or(rest.len());
    rest[..end].to_lowercase().split_whitespace().collect::<Vec<_>>().join(" ")
}

fn has_all(text: &str, words: &[&str]) {
    for w in words {
        assert!(text.contains(w), "missing `{w}` in:\n{text}");
    }
}

#[test]
fn criterion_1_forwarded_calls_keep_outcome_clipped_error_and_duration() {
    let s = section(&read("PRIVACY.md"), "Visits and activity");
    has_all(&s, &["tool call", "outcome", "error", "duration"]);
    assert!(
        s.contains("shortened") || s.contains("clipped") || s.contains("truncated"),
        "the error message is said to be clipped:\n{s}"
    );
}

#[test]
fn criterion_1_says_with_what_the_calls_are_tied() {
    let s = section(&read("PRIVACY.md"), "Visits and activity");
    has_all(&s, &["fingerprint", "visitor"]);
}

#[test]
fn criterion_1_search_words_are_kept_with_or_without_an_account() {
    let s = section(&read("PRIVACY.md"), "Visits and activity");
    assert!(
        s.contains("search") && (s.contains("words") || s.contains("terms")),
        "the words of a search are named:\n{s}"
    );
    assert!(
        s.contains("with or without an account"),
        "the search words are said kept with or without an account:\n{s}"
    );
    assert!(
        !s.contains("only with an account") && !s.contains("only when an account"),
        "the words are not said kept only with an account:\n{s}"
    );
}

#[test]
fn criterion_1_the_local_copy_section_no_longer_promises_search_words_are_never_kept() {
    let s = section(&read("PRIVACY.md"), "A copy installed on your machine");
    let mut from = 0;
    while let Some(i) = s[from..].find("search words") {
        let at = from + i;
        let before = &s[at.saturating_sub(80)..at];
        assert!(
            !before.contains("never") && !before.contains("no "),
            "the section promises the search words are not kept:\n{s}"
        );
        from = at + 1;
    }
    assert!(
        s.contains("visits and activity"),
        "the section points to what is kept of forwarded calls:\n{s}"
    );
}

#[test]
fn criterion_2_detail_is_kept_six_months_then_deleted() {
    let s = section(&read("PRIVACY.md"), "Visits and activity");
    assert!(s.contains("6 months") || s.contains("six months"), "{s}");
    assert!(s.contains("delete"), "the detail is said to be deleted:\n{s}");
}

#[test]
fn criterion_2_only_daily_totals_remain_without_visitor_or_account() {
    let s = section(&read("PRIVACY.md"), "Visits and activity");
    assert!(
        s.contains("totals") || s.contains("counts"),
        "each day's totals are named:\n{s}"
    );
    assert!(s.contains("each day") || s.contains("daily") || s.contains("per day"), "{s}");
    assert!(s.contains("no visitor") || s.contains("without visitor") || s.contains("without any visitor"), "{s}");
    assert!(s.contains("no account") || s.contains("without account") || s.contains("without any account"), "{s}");
}

const PRIVACY_ENTRY_TITLE: &str = "PRIVACY.md says what the service keeps of a call";

/// The entry of `log` titled `PRIVACY_ENTRY_TITLE`, wherever it sits: from its
/// heading to the next heading, lowercased.
fn privacy_entry(log: &str) -> String {
    let section = common::changelog::section_of(log, &[PRIVACY_ENTRY_TITLE]);
    let rest = &section[section.find(PRIVACY_ENTRY_TITLE).unwrap()..];
    let end = rest.find("\n#").unwrap_or(rest.len());
    rest[..end].to_lowercase().split_whitespace().collect::<Vec<_>>().join(" ")
}

fn entry_names_the_change(log: &str) {
    let e = privacy_entry(log);
    assert!(e.contains("privacy.md"), "{e}");
    assert!(e.contains("6 months") || e.contains("six months"), "{e}");
}

fn notes_with(heading: &str, title: &str, body: &str) -> String {
    format!(
        "# Notes\n\n## {heading}\n\n### Changed — {title}\n- {body}\n\n## [0.17.0] — 2026-01-01\n\n### Fixed — other\n- something about 6 months and PRIVACY.md\n"
    )
}

const WITH_BOTH: &str = "PRIVACY.md says the detail is kept 6 months.";

/// The heading of the open section, assembled so no file spells it whole.
fn open_heading() -> String {
    format!("[{}]", ["Unre", "leased"].concat())
}

#[test]
fn criterion_3_changelog_entry_names_the_change() {
    entry_names_the_change(&common::changelog::read());
}

#[test]
fn criterion_5_entry_under_the_open_section_passes() {
    let log = notes_with(&open_heading(), PRIVACY_ENTRY_TITLE, WITH_BOTH);
    entry_names_the_change(&log);
}

#[test]
fn criterion_5_entry_under_a_version_passes() {
    let log = notes_with("[0.18.0] — 2026-10-31", PRIVACY_ENTRY_TITLE, WITH_BOTH);
    entry_names_the_change(&log);
}

#[test]
fn criterion_4_changelog_without_the_entry_fails() {
    let log = notes_with("[0.18.0] — 2026-10-31", "something else", "PRIVACY.md, 6 months.");
    assert!(std::panic::catch_unwind(|| entry_names_the_change(&log)).is_err());
}

#[test]
fn criterion_4_entry_without_the_six_months_fails() {
    let log = notes_with("[0.18.0] — 2026-10-31", PRIVACY_ENTRY_TITLE, "PRIVACY.md says what is kept.");
    assert!(std::panic::catch_unwind(|| entry_names_the_change(&log)).is_err());
}

#[test]
fn criterion_4_entry_stops_at_the_next_heading() {
    // a 6 months mention under another heading does not count for the entry.
    let log = format!("## [0.18.0]\n\n### Changed — {PRIVACY_ENTRY_TITLE}\n\n### Next\n- PRIVACY.md 6 months\n");
    let e = privacy_entry(&log);
    assert!(!e.contains("6 months"), "the entry stops at the next heading:\n{e}");
}

fn files_under(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            let n = e.file_name().to_string_lossy().to_string();
            if n != "target" && n != "node_modules" {
                files_under(&p, out);
            }
        } else {
            out.push(p);
        }
    }
}

#[test]
fn criterion_6_no_test_script_or_guard_is_bound_to_the_open_section_heading() {
    let needle = open_heading();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    for d in ["tests", "crates", "scripts"] {
        files_under(&root.join(d), &mut files);
    }
    assert!(files.len() > 10, "the search reaches the files: {}", files.len());
    let bound: Vec<_> = files
        .iter()
        .filter(|f| std::fs::read_to_string(f).is_ok_and(|t| t.contains(&needle)))
        .collect();
    assert!(bound.is_empty(), "bound to the open-section heading: {bound:?}");
}
