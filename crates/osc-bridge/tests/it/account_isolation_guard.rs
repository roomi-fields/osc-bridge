//! Guard: no test reaches the machine's real account or settings file.
//!
//! The refused paths are the ones of `src/online.rs` that read the real `~/.config`.
//! A new config path in `online.rs` is added to `KEEPS_REAL_ACCOUNT` / `CONFIG_PATH_FNS`
//! together with its faulty example source below.

use std::path::{Path, PathBuf};

/// `Type::function` paths that keep the account in the real config folder.
const KEEPS_REAL_ACCOUNT: &[(&str, &str)] = &[("Online", "new")];
/// Functions that resolve the real config folder; a file calling them must set a `ConfigHome`.
const CONFIG_PATH_FNS: &[&str] = &["account_file", "settings_file"];
/// The type whose rename cannot be followed.
const GUARDED_TYPE: &str = "Online";
/// The helper that sets `XDG_CONFIG_HOME` and `APPDATA` on a temporary folder.
const ISOLATION: &str = "ConfigHome";

#[derive(Debug, PartialEq)]
enum Tok {
    Ident(String),
    Punct(char),
}

/// Code tokens with their line; comments, strings and chars are dropped.
fn lex(src: &str) -> Vec<(usize, Tok)> {
    let c: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let (mut i, mut line) = (0, 1);
    let at = |k: usize| c.get(k).copied().unwrap_or('\0');
    while i < c.len() {
        let ch = c[i];
        if ch == '\n' {
            line += 1;
            i += 1;
        } else if ch.is_whitespace() {
            i += 1;
        } else if ch == '/' && at(i + 1) == '/' {
            while i < c.len() && c[i] != '\n' {
                i += 1;
            }
        } else if ch == '/' && at(i + 1) == '*' {
            let mut depth = 1;
            i += 2;
            while i < c.len() && depth > 0 {
                if c[i] == '/' && at(i + 1) == '*' {
                    depth += 1;
                    i += 2;
                } else if c[i] == '*' && at(i + 1) == '/' {
                    depth -= 1;
                    i += 2;
                } else {
                    if c[i] == '\n' {
                        line += 1;
                    }
                    i += 1;
                }
            }
        } else if let Some(hashes) = raw_string_start(&c, i) {
            // r#"..."# : skip prefix, then to the closing quote followed by the same hashes
            while c[i] != '"' {
                i += 1;
            }
            i += 1;
            while i < c.len() {
                if c[i] == '"' && (1..=hashes).all(|k| at(i + k) == '#') {
                    i += 1 + hashes;
                    break;
                }
                if c[i] == '\n' {
                    line += 1;
                }
                i += 1;
            }
        } else if ch == '"' || (ch == 'b' && at(i + 1) == '"') {
            i += if ch == 'b' { 2 } else { 1 };
            while i < c.len() && c[i] != '"' {
                if c[i] == '\\' {
                    i += 1;
                }
                if at(i) == '\n' {
                    line += 1;
                }
                i += 1;
            }
            i += 1;
        } else if ch == '\'' || (ch == 'b' && at(i + 1) == '\'') {
            let q = if ch == 'b' { i + 1 } else { i };
            if at(q + 1) == '\\' {
                i = q + 2;
                while i < c.len() && c[i] != '\'' {
                    i += 1;
                }
                i += 1;
            } else if at(q + 2) == '\'' {
                i = q + 3;
            } else {
                i = q + 1; // lifetime or label
            }
        } else if ch.is_alphabetic() || ch == '_' {
            let start = i;
            while i < c.len() && (c[i].is_alphanumeric() || c[i] == '_') {
                i += 1;
            }
            out.push((line, Tok::Ident(c[start..i].iter().collect())));
        } else {
            out.push((line, Tok::Punct(ch)));
            i += 1;
        }
    }
    out
}

/// Number of `#` of a raw string starting at `i` (`r"`, `r#"`, `br#"`), if one starts there.
fn raw_string_start(c: &[char], i: usize) -> Option<usize> {
    let mut j = i;
    if c[j] == 'b' {
        j += 1;
    }
    if c.get(j) != Some(&'r') {
        return None;
    }
    // must not be the tail of a longer identifier
    if i > 0 && (c[i - 1].is_alphanumeric() || c[i - 1] == '_') {
        return None;
    }
    j += 1;
    let mut hashes = 0;
    while c.get(j) == Some(&'#') {
        hashes += 1;
        j += 1;
    }
    (c.get(j) == Some(&'"')).then_some(hashes)
}

/// The faults of one test source, each `(line, what)`.
fn faults(src: &str) -> Vec<(usize, String)> {
    let toks = lex(src);
    let ident = |k: usize, name: &str| matches!(toks.get(k), Some((_, Tok::Ident(s))) if s == name);
    let punct = |k: usize, p: char| matches!(toks.get(k), Some((_, Tok::Punct(q))) if *q == p);
    let isolated = (0..toks.len()).any(|k| ident(k, ISOLATION));
    let serves = (0..toks.len()).any(|k| ident(k, "serve") && punct(k + 1, '('));
    let mut found = Vec::new();
    for (k, (line, tok)) in toks.iter().enumerate() {
        let Tok::Ident(name) = tok else { continue };
        for (ty, func) in KEEPS_REAL_ACCOUNT {
            if name == ty && punct(k + 1, ':') && punct(k + 2, ':') && ident(k + 3, func) {
                found.push((*line, format!("{ty}::{func} keeps the account in the real config folder")));
            }
        }
        if name == GUARDED_TYPE && ident(k + 1, "as") {
            found.push((*line, format!("{GUARDED_TYPE} renamed by `as`: cannot be followed")));
        }
        if name == "online" && punct(k + 1, ':') && ident(k + 2, "true") && serves && !isolated {
            found.push((*line, format!("a linked `serve` session (online: true) without a {ISOLATION} in the same file")));
        }
        if CONFIG_PATH_FNS.contains(&name.as_str()) && punct(k + 1, '(') && !isolated {
            found.push((*line, format!("{name}() without a {ISOLATION} in the same file")));
        }
    }
    found
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

/// Every fault of every `.rs` file under `tests/`, as `path:line: what`.
fn tree_faults(root: &Path) -> Vec<String> {
    let mut files = Vec::new();
    rust_files(root, &mut files);
    let mut all = Vec::new();
    for file in files {
        let src = std::fs::read_to_string(&file).unwrap();
        for (line, what) in faults(&src) {
            all.push(format!("{}:{line}: {what}", file.strip_prefix(root).unwrap().display()));
        }
    }
    all
}

fn lines(src: &str) -> Vec<usize> {
    faults(src).into_iter().map(|(l, _)| l).collect()
}

#[test]
fn guard_refuses_online_new_which_keeps_the_real_account() {
    assert_eq!(lines("fn t() {\n    let o = Online::new(\"http://x\");\n}"), vec![2]);
    assert_eq!(lines("let o = osc_bridge::online::Online::new(b);"), vec![1]);
}

#[test]
fn guard_accepts_an_account_kept_in_a_folder_of_its_own() {
    assert!(faults("let o = Online::keeping_account_in(&base, Some(dir.join(\"user\")));").is_empty());
    assert!(faults("let o = online_in(&base, dir.path());").is_empty());
}

#[test]
fn guard_refuses_config_file_calls_without_a_config_home_in_the_file() {
    assert_eq!(lines("fn t() {\n\n    let p = account_file().unwrap();\n}"), vec![3]);
    assert_eq!(lines("let p = settings_file();"), vec![1]);
    assert_eq!(lines("let p = osc_bridge::online::settings_file();"), vec![1]);
}

#[test]
fn guard_accepts_config_file_calls_when_the_file_sets_a_config_home() {
    let src = "fn t() {\n    let home = ConfigHome::new();\n    assert_eq!(account_file().unwrap(), home.account());\n    let _ = settings_file();\n}";
    assert!(faults(src).is_empty());
}

#[test]
fn guard_refuses_a_linked_serve_session_without_a_config_home_in_the_file() {
    let src = "fn t() {\n    let opts = McpOptions {\n        online: true,\n    };\n    osc_bridge::mcp::serve(opts, b, None, i, &mut o);\n}";
    assert_eq!(lines(src), vec![3]);
    assert_eq!(lines("let o = McpOptions { online:true };\nserve (o, b);"), vec![1]);
}

#[test]
fn guard_accepts_a_linked_serve_session_under_a_config_home() {
    let src = "fn t() {\n    let _home = ConfigHome::new();\n    let o = McpOptions { online: true };\n    serve(o, b, None, i, &mut out);\n}";
    assert!(faults(src).is_empty());
}

#[test]
fn guard_accepts_an_unlinked_serve_session() {
    assert!(faults("let o = McpOptions { online: false };\nserve(o, b, None, i, &mut out);").is_empty());
}

#[test]
fn guard_sees_calls_spread_over_lines_spaced_or_inside_a_macro() {
    assert_eq!(lines("let o = Online\n    ::\n    new(b);"), vec![1]);
    assert_eq!(lines("let o = Online :: new(b);"), vec![1]);
    assert_eq!(lines("assert!(matches!(Online::new(b), _));"), vec![1]);
    assert_eq!(lines("dbg!(\n  account_file\n  (),\n);"), vec![2]);
}

#[test]
fn guard_refuses_a_rename_of_online_which_it_cannot_follow() {
    assert_eq!(lines("use osc_bridge::online::Online as Net;\n"), vec![1]);
    assert_eq!(lines("use osc_bridge::online::{Online as Net, Other};"), vec![1]);
}

#[test]
fn guard_reads_neither_comments_nor_strings() {
    let src = r##"
// Online::new(b) and account_file() in a line comment
/* Online::new(b)
   /* nested account_file() */ settings_file() */
let a = "Online::new(b) account_file()";
let b = r#"Online::new("x") settings_file() "quoted""#;
let c = br"Online::new";
let d = '"'; let e = '\''; let f: &'static str = "z";
"##;
    assert!(faults(src).is_empty(), "{:?}", faults(src));
    // code after a string and a char literal is still read
    assert_eq!(lines("let s = \"a\"; let c = '\"';\nOnline::new(b);"), vec![2]);
}

#[test]
fn guard_scans_every_rust_file_under_tests_recursively() {
    let dir = tempfile::tempdir().unwrap();
    let deep = dir.path().join("elsewhere").join("deeper");
    std::fs::create_dir_all(&deep).unwrap();
    std::fs::write(deep.join("stray.rs"), "fn t() {\n    Online::new(b);\n}\n").unwrap();
    std::fs::write(dir.path().join("top.rs"), "fn t() { let _ = account_file(); }\n").unwrap();
    std::fs::write(dir.path().join("notes.txt"), "Online::new(b)").unwrap();
    let found = tree_faults(dir.path());
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(found.iter().any(|f| f.starts_with(&format!("elsewhere{}deeper{}stray.rs:2:", std::path::MAIN_SEPARATOR, std::path::MAIN_SEPARATOR))));
    assert!(found.iter().any(|f| f.starts_with("top.rs:1:")));
}

#[test]
fn real_test_tree_never_reaches_the_real_account() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let found = tree_faults(&root);
    assert!(found.is_empty(), "tests reaching the real account:\n{}", found.join("\n"));
}
