//! PRIVACY.md says what the service keeps of a local copy's forwarded calls
//! (OB-vch.1.3): the call, its outcome, a clipped error message, its
//! duration, the search words with or without an account; the 6 months of
//! detail; the daily totals that stay, without visitor or account.

use std::path::Path;

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

#[test]
fn criterion_3_changelog_unreleased_names_the_change() {
    let log = read("CHANGELOG.md");
    let start = log.find("## [Unreleased]").unwrap();
    let rest = &log[start + 5..];
    let end = rest.find("\n## [").unwrap_or(rest.len());
    let unreleased = rest[..end].to_lowercase();
    assert!(unreleased.contains("privacy.md"), "{unreleased}");
    assert!(unreleased.contains("6 months") || unreleased.contains("six months"), "{unreleased}");
}
