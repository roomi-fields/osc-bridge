//! Guard: the bridge is free of its first service. Outside the module of the link
//! (`src/synthub.rs`, `src/synthub/`) no file of `src/` reads that module, writes the name of the
//! service in any case, or writes its host name, comments and strings included.
//!
//! Two exemptions only, told apart by tokens: the declaration `pub mod synthub;` of `lib.rs`, and in
//! `main.rs` the call by full path `osc_bridge::synthub::<submodule>::…` in code position (never a
//! `use`, a `mod`, a path without submodule, a rename or a glob). A word in a comment or a string,
//! even written `synthub::x`, is refused.

use crate::account_isolation_guard::{lex, Tok};
use std::path::{Path, PathBuf};

/// The reason of every inscribed line.
const PUBLISHED: &str =
    "published text, leaves with the removal of the link (RENAME-SYNTHUB § 4, R0)";

/// The inscribed list, frozen both ways: a line of the sources that names the service and is not
/// here is refused; an entry here that matches no line is refused. Each entry is a file under
/// `src/`, the exact text of the line (indentation trimmed) and its reason. It holds published
/// texts only (`--help` of `main.rs`, the schema of the local tool `detect_hardware`).
const INSCRIBED: &[(&str, &str, &str)] = &[
    ("main.rs", "/// Show or change what this machine shares with the SynthHub server.", PUBLISHED),
    ("main.rs", "/// Match against the drivers of this folder instead of the SynthHub", PUBLISHED),
    (
        "main.rs",
        "/// With --learn: the port (a part of its name) whose fingerprint is sent to SynthHub, so that",
        PUBLISHED,
    ),
    ("main.rs", "/// the SynthHub catalogue (fetched one at a time and cached).", PUBLISHED),
    ("main.rs", "/// Do not contact the SynthHub server for this run (also", PUBLISHED),
    ("main.rs", "/// or slug (minilab3), fetched from the SynthHub server and kept in", PUBLISHED),
    (
        "mcp.rs",
        r#""share_fingerprint": { "type": "string", "description": "Learn mode only: the port (a part of its name) whose fingerprint is sent to SynthHub, so that everyone recognises this device. Giving it, with vendor, model and firmware, sends that device's fingerprint to SynthHub (POST /v1/fingerprints): the vendor, model and firmware, the USB vendor and product ids, the identity_reply, the ports (names without machine numbers), what the device sends and its controls; never a serial number, a host name, a measure or anything about the person. Nothing is sent for a private device or when offline. Ask the person first. Needs vendor, model and firmware, none empty." },"#,
        PUBLISHED,
    ),
];

/// The host name of the service, as `online::SERVER` writes it.
const HOST: &str = "mcp.synthhub.roomi-fields.com";

/// The names of the service, lower-cased: its spelling in the code and its spelling in prose.
const NAMES: &[&str] = &["synthub", "synthhub"];

fn occurrences(line: &str) -> usize {
    let low = line.to_lowercase();
    NAMES.iter().map(|n| low.matches(n).count()).sum()
}

/// Lines of the tokens of `src` that are the exempt forms, for the file called `file`.
fn exempt_lines(file: &str, src: &str) -> Vec<usize> {
    let toks = lex(src);
    let ident = |k: Option<usize>| match k.and_then(|k| toks.get(k)) {
        Some((_, Tok::Ident(s))) => Some(s.as_str()),
        _ => None,
    };
    let punct = |k: usize, p: char| matches!(toks.get(k), Some((_, Tok::Punct(q))) if *q == p);
    let mut out = Vec::new();
    for (k, (line, tok)) in toks.iter().enumerate() {
        if !matches!(tok, Tok::Ident(s) if s == "synthub") {
            continue;
        }
        let ok = match file {
            "lib.rs" => {
                ident(k.checked_sub(1)) == Some("mod")
                    && ident(k.checked_sub(2)) == Some("pub")
                    && punct(k + 1, ';')
            }
            "main.rs" => {
                k >= 3
                    && ident(Some(k - 3)) == Some("osc_bridge")
                    && punct(k - 2, ':')
                    && punct(k - 1, ':')
                    && punct(k + 1, ':')
                    && punct(k + 2, ':')
                    && ident(Some(k + 3)).is_some()
                    && punct(k + 4, ':')
                    && punct(k + 5, ':')
                    && !prefixed_by_use(&toks, k)
            }
            _ => false,
        };
        if ok {
            out.push(*line);
        }
    }
    out
}

/// Whether the statement holding token `k` starts with `use` (or `pub use`).
fn prefixed_by_use(toks: &[(usize, Tok)], k: usize) -> bool {
    for j in (0..k).rev() {
        match &toks[j].1 {
            Tok::Punct(';') | Tok::Punct('}') | Tok::Punct('{') => return false,
            Tok::Ident(s) if s == "use" => return true,
            _ => {}
        }
    }
    false
}

/// Refused lines of one source, `(line, what)`; `file` is its path under `src/`.
fn refused_in(
    file: &str,
    src: &str,
    inscribed: &[(&str, &str, &str)],
) -> Vec<(usize, String)> {
    let exempt = exempt_lines(file, src);
    let mut out = Vec::new();
    for (i, text) in src.lines().enumerate() {
        let line = i + 1;
        if inscribed.iter().any(|(f, t, _)| *f == file && *t == text.trim()) {
            continue;
        }
        let exempted = exempt.iter().filter(|l| **l == line).count();
        if text.contains(HOST) {
            out.push((line, "the host name of the service".to_string()));
        }
        if occurrences(text) > exempted {
            out.push((line, "the name of the service, or a reading of the link".to_string()));
        }
    }
    out
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

/// The name of a path relative to `src`, written with `/` on every platform.
fn slash_name(rel: &Path) -> String {
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

#[test]
fn a_file_is_named_with_a_slash_whatever_the_platform_separator() {
    let rel: PathBuf = ["a", "b.rs"].iter().collect();
    assert_eq!(slash_name(&rel), "a/b.rs");
}

/// Every refusal over the sources of `src`, as `path:line: what`, outside the module of the link.
fn problems(src: &Path, inscribed: &[(&str, &str, &str)]) -> Vec<String> {
    let mut files = Vec::new();
    rust_files(src, &mut files);
    let mut out = Vec::new();
    for (f, t, _) in inscribed {
        let held = std::fs::read_to_string(src.join(f))
            .is_ok_and(|s| s.lines().any(|l| l.trim() == *t));
        if !held {
            out.push(format!("{f}: stale inscribed entry, no line matches: {t}"));
        }
    }
    for file in files {
        let rel = file.strip_prefix(src).unwrap();
        let first = rel.components().next().unwrap().as_os_str().to_string_lossy().into_owned();
        if first == "synthub.rs" || first == "synthub" {
            continue;
        }
        let rel_s = slash_name(rel);
        for (line, what) in
            refused_in(&rel_s, &std::fs::read_to_string(&file).unwrap(), inscribed)
        {
            out.push(format!("{rel_s}:{line}: {what}"));
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

fn refused(files: &[(&str, &str)]) -> Vec<String> {
    problems(tree(files).path(), &[])
}

fn refused_with(files: &[(&str, &str)], inscribed: &[(&str, &str, &str)]) -> Vec<String> {
    problems(tree(files).path(), inscribed)
}

#[test]
fn guard_list_is_frozen_both_ways() {
    let body = "/// Talks to the SynthHub server.\nfn f() {}\n";
    let entry = [("main.rs", "/// Talks to the SynthHub server.", "reason")];
    // an inscribed line passes
    assert!(refused_with(&[("main.rs", body)], &entry).is_empty());
    // the same text in another file is refused
    assert!(!refused_with(&[("orchestrator.rs", body)], &entry).is_empty());
    // a line that is not inscribed is refused
    let other = "/// Talks to the SynthHub server.\n/// Another SynthHub line.\n";
    assert!(!refused_with(&[("main.rs", other)], &entry).is_empty());
    // an entry that matches no line is refused
    let stale = refused_with(&[("main.rs", "fn f() {}\n")], &entry);
    assert!(stale.iter().any(|f| f.contains("stale")), "{stale:?}");
}

#[test]
fn the_sources_of_the_bridge_name_neither_the_service_nor_its_host_nor_read_the_link() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    assert!(src.join("synthub").join("online.rs").is_file(), "the link is not under src/synthub/");
    let found = problems(&src, INSCRIBED);
    assert!(found.is_empty(), "the bridge is not free of the service:\n{}", found.join("\n"));
}

#[test]
fn guard_refuses_a_reading_of_the_link_from_a_module_of_the_bridge() {
    for (file, body) in [
        ("orchestrator.rs", "fn f() { crate::synthub::catalogue::locate(x); }\n"),
        ("mcp.rs", "use crate::synthub::online::linked;\n"),
        ("mcp/tests.rs", "fn t() { super::super::synthub::take::find(); }\n"),
        ("midi_port.rs", "use crate::synthub::{online, take};\n"),
        ("install.rs", "use crate::synthub as s;\n"),
        ("detect.rs", "use crate::synthub::*;\n"),
        ("lib.rs", "fn f() { crate::synthub::online::linked(); }\n"),
    ] {
        let found = refused(&[(file, body)]);
        assert!(found.iter().any(|f| f.starts_with(&format!("{file}:"))), "{file}: {found:?}");
    }
}

#[test]
fn guard_refuses_a_name_of_the_service_in_any_case_in_code_comments_and_strings() {
    for body in [
        "// the SynthHub catalogue\nfn f() {}\n",
        "/* synthub */ fn f() {}\n",
        "//! Link to synthub.\n",
        "fn f() -> &'static str { \"the SYNTHHUB service\" }\n",
        "// crate::synthub::online::linked()\n",
        "let s = \"crate::synthub::take::open\";\n",
        "fn synthub() {}\n",
        "fn f() { SynthHub::x(); }\n",
    ] {
        let found = refused(&[("orchestrator.rs", body)]);
        assert!(!found.is_empty(), "not refused: {body}");
    }
}

#[test]
fn guard_refuses_the_host_name_of_the_service() {
    for (file, body) in [
        ("orchestrator.rs", "const S: &str = \"https://mcp.synthhub.roomi-fields.com\";\n"),
        ("main.rs", "const S: &str = \"https://mcp.synthhub.roomi-fields.com\";\n"),
        ("mcp.rs", "// see mcp.synthhub.roomi-fields.com\n"),
    ] {
        let found = refused(&[(file, body)]);
        assert!(found.iter().any(|f| f.contains("host name")), "{file}: {found:?}");
    }
}

#[test]
fn guard_leaves_the_link_itself_unread() {
    let found = refused(&[
        ("synthub.rs", "//! The link to SynthHub.\npub mod online;\nuse crate::synthub::online::X;\n"),
        ("synthub/online.rs", "pub const SERVER: &str = \"https://mcp.synthhub.roomi-fields.com\";\n"),
        ("synthub/take.rs", "use crate::synthub::catalogue::y;\n"),
    ]);
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn guard_allows_the_declaration_of_the_module_in_lib_only() {
    assert!(refused(&[("lib.rs", "pub mod synthub;\n")]).is_empty());
    assert!(!refused(&[("lib.rs", "pub mod synthub;\n// synthub\n")]).is_empty());
    assert!(!refused(&[("orchestrator.rs", "pub mod synthub;\n")]).is_empty());
    assert!(!refused(&[("main.rs", "mod synthub;\n")]).is_empty());
}

#[test]
fn guard_allows_main_to_call_the_link_by_full_path_only() {
    let ok = "fn main() { let d = osc_bridge::synthub::catalogue::locate(&p)?; osc_bridge::synthub::connect::run(a); }\n";
    assert!(refused(&[("main.rs", ok)]).is_empty());
    let attr = "#[arg(default_value = osc_bridge::synthub::connect::DEFAULT_RELAY)]\nstruct A;\n";
    assert!(refused(&[("main.rs", attr)]).is_empty());
    // the same call elsewhere is refused
    assert!(!refused(&[("orchestrator.rs", ok)]).is_empty());
}

#[test]
fn guard_refuses_in_main_a_use_a_bare_path_a_rename_or_a_glob() {
    for body in [
        "use osc_bridge::synthub::catalogue::locate;\n",
        "pub use osc_bridge::synthub::catalogue;\n",
        "use osc_bridge::synthub;\n",
        "use osc_bridge::synthub as s;\n",
        "use osc_bridge::synthub::*;\n",
        "use osc_bridge::{synthub::online, device};\n",
        "fn main() { osc_bridge::synthub::run(); }\n",
        "fn main() { let s = osc_bridge::synthub; }\n",
        "fn main() { osc_bridge::synthub::*; }\n",
        "fn main() { use osc_bridge::synthub::online::linked; }\n",
        "fn main() { crate::synthub::online::linked(); }\n",
    ] {
        assert!(!refused(&[("main.rs", body)]).is_empty(), "not refused: {body}");
    }
}

#[test]
fn guard_refuses_in_main_a_word_in_a_comment_or_a_string_even_written_as_a_path() {
    for body in [
        "// osc_bridge::synthub::online::linked()\nfn main() {}\n",
        "fn main() { println!(\"osc_bridge::synthub::online::linked\"); }\n",
        "/// Connect to SynthHub.\nstruct A;\n",
        "fn main() { osc_bridge::synthub::online::linked(); } // synthub\n",
    ] {
        assert!(!refused(&[("main.rs", body)]).is_empty(), "not refused: {body}");
    }
}
