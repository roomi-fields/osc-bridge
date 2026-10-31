//! The one place of `tests/` that reads the release notes. A test finds its
//! entry by its words in whichever section holds it (the open one or a
//! version): at a release the entry moves, what the test checks stays true.

#![allow(dead_code)]

/// The release notes of the repository, read from disk. The only read.
pub fn read() -> String {
    std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/", "CHANGE", "LOG.md")).unwrap()
}

/// The section (heading line to the next `## ` heading) of the repository's
/// release notes that holds all `words`. Panics naming the words otherwise.
pub fn section_with_words(words: &[&str]) -> String {
    section_of(&read(), words)
}

/// The section of `text` that holds all `words`: the first in file order
/// (newest first) when several do. Panics naming every word, and the words
/// missing from the best section, when no single section holds them all.
pub fn section_of(text: &str, words: &[&str]) -> String {
    let sections = sections(text);
    if let Some(s) = sections.iter().find(|s| words.iter().all(|w| s.contains(w))) {
        return s.clone();
    }
    let best_missing = sections
        .iter()
        .map(|s| words.iter().filter(|w| !s.contains(**w)).copied().collect::<Vec<_>>())
        .min_by_key(|m| m.len())
        .unwrap_or_else(|| words.to_vec());
    panic!(
        "no section holds all the words {words:?}; the best section lacks {best_missing:?}"
    );
}

fn sections(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        if line.starts_with("## ") {
            out.push(String::new());
        }
        if let Some(cur) = out.last_mut() {
            cur.push_str(line);
            cur.push('\n');
        }
    }
    out
}
