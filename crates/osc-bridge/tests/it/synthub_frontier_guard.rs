//! Guard: the link to the service lives in `src/synthub/`, and what still reads it is frozen.
//!
//! Every file of `src/` outside `synthub.rs`, `synthub/` and `main.rs` is read as tokens. Each
//! occurrence of the path `synthub::<submodule>` counts as one reading of that submodule by the
//! reader module (the first path component under `src/`). The counts must equal `INSCRIBED`:
//! a higher real count is a new reading, a lower one a stale entry to lower. The list only shrinks.

use crate::account_isolation_guard::{lex, Tok};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The submodules of the link.
const SUBMODULES: &[&str] = &["online", "connect", "relay_protocol", "catalogue", "take"];

/// (reader module, submodule of the link, number of readings).
const INSCRIBED: &[(&str, &str, usize)] = &[
    ("orchestrator", "catalogue", 1),
    ("install", "take", 2),
    ("detect", "take", 2),
    ("mcp", "online", 43),
    ("mcp", "catalogue", 5),
    ("mcp", "take", 11),
];

/// `(line, what)` pairs found in one source.
type Found = Vec<(usize, String)>;

type Counts =BTreeMap<(String, String), usize>;

/// Readings of one source: each submodule named after `synthub`, and each refused form, `(line, what)`.
fn readings(src: &str) -> (Found, Found) {
    let toks = lex(src);
    let ident = |k: usize| match toks.get(k) {
        Some((_, Tok::Ident(s))) => Some(s.as_str()),
        _ => None,
    };
    let punct = |k: usize, p: char| matches!(toks.get(k), Some((_, Tok::Punct(q))) if *q == p);
    let (mut subs, mut bad) = (Vec::new(), Vec::new());
    let named = |subs: &mut Found, bad: &mut Found, line: usize, name: Option<&str>| match name {
        Some(n) if SUBMODULES.contains(&n) => subs.push((line, n.to_string())),
        Some(n) => bad.push((line, format!("synthub::{n} is not a submodule of the link"))),
        None => bad.push((line, "synthub::<not a submodule name>: the submodule read cannot be determined".into())),
    };
    for (k, (line, tok)) in toks.iter().enumerate() {
        if !matches!(tok, Tok::Ident(s) if s == "synthub") {
            continue;
        }
        if k > 0 && ident(k - 1) == Some("mod") {
            continue;
        }
        if !(punct(k + 1, ':') && punct(k + 2, ':')) {
            bad.push((*line, "bare `synthub`: the submodule read cannot be determined".into()));
        } else if punct(k + 3, '{') {
            // one reading per member of the group, at depth 1
            let (mut depth, mut j, mut at_start) = (0usize, k + 3, true);
            while let Some((l, t)) = toks.get(j) {
                match t {
                    Tok::Punct('{') => {
                        depth += 1;
                        at_start = depth == 1;
                    }
                    Tok::Punct('}') => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    Tok::Punct(',') if depth == 1 => at_start = true,
                    Tok::Ident(s) if depth == 1 && at_start => {
                        named(&mut subs, &mut bad, *l, Some(s.as_str()));
                        at_start = false;
                    }
                    Tok::Punct(_) if depth == 1 && at_start => {
                        named(&mut subs, &mut bad, *l, None);
                        at_start = false;
                    }
                    _ => {}
                }
                j += 1;
            }
        } else if punct(k + 3, '*') {
            bad.push((*line, "`synthub::*`: the submodule read cannot be determined".into()));
        } else {
            named(&mut subs, &mut bad, *line, ident(k + 3));
        }
        if ident(k + 1) == Some("as") {
            bad.push((*line, "`synthub as …`: renamed, cannot be followed".into()));
        }
    }
    (subs, bad)
}

fn rust_files(dir: &Path, into: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            rust_files(&path, into);
        } else if path.extension().is_some_and(|e| e == "rs") {
            into.push(path);
        }
    }
}

/// The reader module of a file: the first component under `src/`, without `.rs`.
fn reader_of(rel: &Path) -> String {
    let first = rel.components().next().unwrap().as_os_str().to_string_lossy().into_owned();
    first.strip_suffix(".rs").unwrap_or(&first).to_string()
}

/// Counts per (reader, submodule) and the refused forms as `path:line: what`, over `src`.
fn tree_readings(src: &Path) -> (Counts, Vec<String>) {
    let mut files = Vec::new();
    rust_files(src, &mut files);
    let (mut counts, mut refused) = (Counts::new(), Vec::new());
    for file in files {
        let rel = file.strip_prefix(src).unwrap();
        let reader = reader_of(rel);
        if reader == "synthub" || reader == "main" {
            continue;
        }
        let (subs, bad) = readings(&std::fs::read_to_string(&file).unwrap());
        for (_, sub) in subs {
            *counts.entry((reader.clone(), sub)).or_default() += 1;
        }
        for (line, what) in bad {
            refused.push(format!("{}:{line}: {what}", rel.display()));
        }
    }
    (counts, refused)
}

/// Every problem of the readings against an inscribed list.
fn problems(src: &Path, inscribed: &[(&str, &str, usize)]) -> Vec<String> {
    let (counts, mut out) = tree_readings(src);
    for ((reader, sub), real) in &counts {
        let listed = inscribed.iter().find(|(r, s, _)| r == reader && s == sub).map_or(0, |e| e.2);
        if *real > listed {
            out.push(format!("new reading: {reader} reads synthub::{sub} {real} times, {listed} inscribed"));
        }
    }
    for (reader, sub, n) in inscribed {
        let real = counts.get(&(reader.to_string(), sub.to_string())).copied().unwrap_or(0);
        if real < *n {
            out.push(format!("stale entry: {reader} reads synthub::{sub} {real} times, {n} inscribed: lower it"));
        }
    }
    out
}

fn tree(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (path, body) in files {
        let full = dir.path().join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, body).unwrap();
    }
    dir
}

fn count(src: &str) -> Vec<String> {
    readings(src).0.into_iter().map(|(_, s)| s).collect()
}

#[test]
fn guard_refuses_a_reading_of_the_link_that_is_not_inscribed() {
    let dir = tree(&[("orchestrator.rs", "fn f() { crate::synthub::catalogue::locate(x); }\n")]);
    let found = problems(dir.path(), &[]);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("new reading") && found[0].contains("orchestrator") && found[0].contains("catalogue"));
}

#[test]
fn guard_accepts_an_inscribed_reading() {
    let dir = tree(&[("install.rs", "use crate::synthub::take::Midi;\nfn f(o: crate::synthub::take::Opener) {}\n")]);
    assert!(problems(dir.path(), &[("install", "take", 2)]).is_empty());
}

#[test]
fn guard_refuses_a_count_above_the_inscribed_one() {
    let dir = tree(&[("install.rs", "use crate::synthub::take::Midi;\nfn f(o: crate::synthub::take::Opener) {}\n")]);
    let found = problems(dir.path(), &[("install", "take", 1)]);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("new reading"));
}

#[test]
fn guard_refuses_a_stale_entry_whose_count_fell() {
    let dir = tree(&[("install.rs", "use crate::synthub::take::Midi;\n")]);
    let found = problems(dir.path(), &[("install", "take", 2)]);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("stale entry"));
}

#[test]
fn guard_refuses_an_entry_that_no_longer_exists_in_the_sources() {
    let dir = tree(&[("install.rs", "fn f() {}\n")]);
    let found = problems(dir.path(), &[("install", "take", 1)]);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("stale entry"));
}

#[test]
fn guard_counts_readings_across_the_files_of_one_reader_module() {
    let dir = tree(&[
        ("mcp.rs", "fn f() { crate::synthub::online::linked(); }\n"),
        ("mcp/tests.rs", "fn t() { crate::synthub::online::SERVER; crate::synthub::take::find(); }\n"),
    ]);
    assert!(problems(dir.path(), &[("mcp", "online", 2), ("mcp", "take", 1)]).is_empty());
}

#[test]
fn guard_reads_super_self_and_osc_bridge_paths_the_same() {
    assert_eq!(count("use super::synthub::online::Online;"), ["online"]);
    assert_eq!(count("use self::synthub::take::Midi;"), ["take"]);
    assert_eq!(count("let c = osc_bridge::synthub::catalogue::locate(p);"), ["catalogue"]);
    assert_eq!(count("synthub\n    ::\n    connect::run();"), ["connect"]);
}

#[test]
fn guard_counts_one_reading_per_member_of_a_group() {
    assert_eq!(count("use crate::synthub::{online::Online, take::{find, Midi}, catalogue};"), ["online", "take", "catalogue"]);
    assert!(readings("use crate::synthub::{online, take};").1.is_empty());
}

#[test]
fn guard_refuses_a_bare_synthub_import() {
    let (subs, bad) = readings("use crate::synthub;\n");
    assert!(subs.is_empty());
    assert_eq!(bad.len(), 1, "{bad:?}");
}

#[test]
fn guard_refuses_a_renamed_synthub_import() {
    let (_, bad) = readings("use crate::synthub as s;\n");
    assert!(!bad.is_empty());
}

#[test]
fn guard_refuses_a_glob_import_of_synthub() {
    let (subs, bad) = readings("use crate::synthub::*;\n");
    assert!(subs.is_empty());
    assert_eq!(bad.len(), 1, "{bad:?}");
}

#[test]
fn guard_refuses_a_bare_synthub_that_no_inscribed_entry_can_list() {
    let dir = tree(&[("mcp.rs", "use crate::synthub;\n")]);
    let found = problems(dir.path(), INSCRIBED);
    assert!(found.iter().any(|f| f.starts_with("mcp.rs:1:")), "{found:?}");
}

#[test]
fn guard_refuses_a_path_through_synthub_that_names_no_submodule() {
    let (_, bad) = readings("use crate::synthub::Online;\n");
    assert_eq!(bad.len(), 1, "{bad:?}");
}

#[test]
fn guard_does_not_read_the_declaration_of_the_module_nor_comments_nor_strings() {
    assert_eq!(readings("pub mod synthub;\n"), (vec![], vec![]));
    let src = "// crate::synthub::online::linked()\n/* synthub */\nlet s = \"crate::synthub::take::open\";\n";
    assert_eq!(readings(src), (vec![], vec![]));
}

#[test]
fn guard_leaves_the_link_itself_and_main_unread() {
    let dir = tree(&[
        ("synthub.rs", "pub mod online;\nuse crate::synthub::online::X;\n"),
        ("synthub/take.rs", "use crate::synthub::catalogue::y;\n"),
        ("main.rs", "fn main() { osc_bridge::synthub::online::linked(); }\n"),
        ("lib.rs", "pub mod synthub;\n"),
    ]);
    assert!(problems(dir.path(), &[]).is_empty());
}

#[test]
fn the_sources_read_the_link_only_as_inscribed() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    assert!(src.join("synthub").join("online.rs").is_file(), "the link is not under src/synthub/");
    let found = problems(&src, INSCRIBED);
    assert!(found.is_empty(), "readings of the link outside the inscribed list:\n{}", found.join("\n"));
}

#[test]
fn the_first_list_is_the_readings_of_the_bridge_at_the_split() {
    let mut list: Vec<_> = INSCRIBED.iter().map(|(r, s, _)| (*r, *s)).collect();
    list.sort();
    assert_eq!(
        list,
        [("detect", "take"), ("install", "take"), ("mcp", "catalogue"), ("mcp", "online"), ("mcp", "take"), ("orchestrator", "catalogue")]
    );
}
