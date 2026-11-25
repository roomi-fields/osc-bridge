//! An installed osc-bridge reaches the SyntHub service at its address, with its identifier
//! header, and names the product `SyntHub` in what it shows (service contract,
//! `crates/service/docs/INTERFACE.md` of the service, « Le nom du produit et son ancienne
//! adresse »): one address, one header, no alias. The former address
//! `synthhub.roomi-fields.com` and the former header `X-Synthhub-Id` are gone from the crate and
//! from the published texts that describe the present.
//!
//! No network: the service is a fake one on a local port.

// @lit README.md CONTRIBUTING.md PRIVACY.md docs/MCP.md docs/getting-started.md
// @lit docs/TUTORIAL_FIRST_DEVICE.md docs/README.md docs/CONFIGURATION_COVERAGE.md
// @lit examples/bridge.toml npm/README.md npm/package.json server.json mcpb/manifest.json
// @lit .claude-plugin/plugin.json .github/**

use crate::common::{catalogue, initialize, mcp_session, temp, tool_call, FakeSynthHub};
use osc_bridge::synthub::online::{Online, Settings};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const SERVER: &str = "https://mcp.synthub.roomi-fields.com";
const RELAY: &str = "wss://mcp.synthub.roomi-fields.com/relay";
const FORMER_DOMAIN: &str = "synthhub.roomi-fields.com";
/// The former spelling of the product, its domain and its header, lower-cased.
const FORMER: &str = "synthhub";

fn krate() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn root() -> PathBuf {
    krate().join("../..")
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn files_under(dir: &Path, into: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            files_under(&path, into);
        } else {
            into.push(path);
        }
    }
}

/// The lines of `text` that contain `needle` (lower-cased), as `name:line: text`.
fn lines_with(name: &str, text: &str, needle: &str) -> Vec<String> {
    text.lines()
        .enumerate()
        .filter(|(_, l)| l.to_lowercase().contains(needle))
        .map(|(i, l)| format!("{name}:{}: {}", i + 1, l.trim()))
        .collect()
}

// ---- the address ----

#[test]
fn the_client_joins_the_service_at_its_address() {
    assert_eq!(osc_bridge::synthub::online::SERVER, SERVER);
}

#[test]
fn the_relay_door_joins_the_relay_at_its_address() {
    assert_eq!(osc_bridge::synthub::connect::DEFAULT_RELAY, RELAY);
}

#[test]
fn no_source_fixture_or_interface_text_of_the_crate_names_the_former_domain() {
    let mut files = Vec::new();
    files_under(&krate().join("src"), &mut files);
    files_under(&krate().join("tests/fixtures"), &mut files);
    files.push(krate().join("docs/INTERFACE.md"));
    let found: Vec<String> = files
        .iter()
        .flat_map(|f| {
            let text = String::from_utf8_lossy(&std::fs::read(f).unwrap()).into_owned();
            lines_with(&f.display().to_string(), &text, FORMER_DOMAIN)
        })
        .collect();
    assert!(found.is_empty(), "the former domain is named:\n{}", found.join("\n"));
}

#[test]
fn the_dated_rename_statement_keeps_its_quotes_of_the_former_address() {
    assert!(read(&krate().join("docs/RENAME-SYNTHUB.md")).contains(FORMER_DOMAIN));
}

// ---- the links to the pages of the site ----

#[test]
fn the_contribution_tool_links_the_format_page_of_the_site() {
    let tools = osc_bridge::synthub::mcp::linked_tool_schemas();
    let submit = tools["tools"].as_array().unwrap().iter().find(|t| t["name"] == "submit_driver").unwrap();
    let text = submit.to_string();
    assert!(text.contains("https://synthub.roomi-fields.com/osd/"), "{text}");
}

#[test]
fn the_virtual_synth_card_and_its_notes_link_the_connect_page_of_the_site() {
    let examples = krate().join("tests/fixtures/devices/examples");
    for file in ["test_synth.json", "test_synth.md"] {
        let text = read(&examples.join(file));
        assert!(text.contains("https://synthub.roomi-fields.com/connect/"), "{file}:\n{text}");
    }
}

#[test]
fn the_relay_protocol_and_the_connect_door_cite_the_connect_page_of_the_site() {
    for file in ["src/synthub/relay_protocol.rs", "src/synthub/connect.rs"] {
        let text = read(&krate().join(file));
        assert!(text.contains("https://synthub.roomi-fields.com/connect"), "{file}");
    }
}

// ---- the identifier header ----

/// One request the service received: its path, and the identifier in each of the two headers.
#[derive(Clone, Debug)]
struct Seen {
    url: String,
    id: Option<String>,
    former: Option<String>,
}

const ACCOUNT: &str = "0123456789abcdef0123456789abcdef";

/// The service after its rename: `426` to a request that carries the former header, `200` to one
/// that carries the account it delivered in `X-Synthub-Id` (or in the path `/u/<id>/`), `401`
/// with a challenge otherwise.
struct Renamed {
    base: String,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Renamed {
    fn start() -> Self {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let base = format!("http://{}", server.server_addr());
        let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
        let log = seen.clone();
        std::thread::spawn(move || {
            for req in server.incoming_requests() {
                let url = req.url().to_string();
                let header = |name: &'static str| {
                    req.headers().iter().find(|h| h.field.equiv(name)).map(|h| h.value.to_string())
                };
                let (id, former) = (header("X-Synthub-Id"), header("X-Synthhub-Id"));
                log.lock().unwrap().push(Seen { url: url.clone(), id: id.clone(), former: former.clone() });
                let challenge = json!({ "challenge": "c", "algorithm": "sha256", "bits": 4, "deliver": "/v1/access/deliver" });
                let in_path = url.starts_with(&format!("/u/{ACCOUNT}/"));
                let (status, body) = if former.is_some() {
                    (426, json!({ "error": "install the current version" }))
                } else if url == "/v1/access/challenge" {
                    (200, challenge)
                } else if url == "/v1/access/deliver" {
                    (200, json!({ "id": ACCOUNT, "user_id": ACCOUNT, "mcp_path": format!("/u/{ACCOUNT}/mcp"), "tier": "free" }))
                } else if !in_path && id.as_deref() != Some(ACCOUNT) {
                    (401, challenge)
                } else if url == "/v1/trust" {
                    (200, json!({ "devices": { "acme/box": { "status": "confirmed" } } }))
                } else if in_path {
                    (200, json!({ "jsonrpc": "2.0", "id": 1, "result": { "content": [] } }))
                } else {
                    (200, json!({}))
                };
                let _ = req.respond(tiny_http::Response::from_string(body.to_string()).with_status_code(status));
            }
        });
        Renamed { base, seen }
    }

    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

fn client(service: &Renamed, dir: &Path) -> Online {
    Online::keeping_account_in(&service.base, Some(dir.join("user")))
}

#[test]
fn a_machine_with_an_account_is_served_by_the_renamed_service() {
    let (service, dir) = (Renamed::start(), temp("renamed-kept"));
    std::fs::write(dir.path().join("user"), ACCOUNT).unwrap();
    let trust: Value = client(&service, dir.path()).trust().unwrap();
    assert_eq!(trust["devices"]["acme/box"]["status"], "confirmed", "{trust}");
    let seen = service.seen();
    assert_eq!(seen.len(), 1, "served at once: {seen:?}");
    assert_eq!(seen[0].id.as_deref(), Some(ACCOUNT), "{seen:?}");
}

#[test]
fn a_new_machine_obtains_an_account_and_is_served_by_the_renamed_service() {
    let (service, dir) = (Renamed::start(), temp("renamed-new"));
    let trust = client(&service, dir.path()).trust().unwrap();
    assert_eq!(trust["devices"]["acme/box"]["status"], "confirmed", "{trust}");
    let seen = service.seen();
    let served = seen.iter().find(|r| r.url == "/v1/trust").unwrap();
    assert_eq!(served.id.as_deref(), Some(ACCOUNT), "{seen:?}");
}

#[test]
fn no_request_of_any_family_sends_the_former_header() {
    let (service, dir) = (Renamed::start(), temp("renamed-families"));
    let online = client(&service, dir.path());
    online.trust().unwrap();
    online.librarian_synths().unwrap();
    online.librarian("recognize", &json!({})).unwrap();
    online.preset_messages("abc", None).unwrap();
    online.call_tool(ACCOUNT, "get_trust", &json!({})).unwrap();
    online.upload(ACCOUNT, "abc", "image", b"x").unwrap();
    let seen = service.seen();
    assert!(seen.iter().any(|r| r.url.starts_with("/u/")), "{seen:?}");
    assert!(seen.iter().all(|r| r.former.is_none()), "the former header was sent: {seen:?}");
}

#[test]
fn a_linked_session_is_served_by_the_renamed_service() {
    let (service, dir) = (Renamed::start(), temp("renamed-session"));
    let replies = mcp_session(&service.base, &dir.path().join("cache"), &[initialize(), tool_call(1, "get_trust", json!({}))]);
    assert_ne!(replies[1]["result"]["isError"], true, "{}", replies[1]);
    let seen = service.seen();
    assert!(!seen.is_empty() && seen.iter().all(|r| r.former.is_none()), "{seen:?}");
}

// ---- the name of the product ----

#[test]
fn no_text_the_sources_show_writes_the_former_name() {
    let src = krate().join("src");
    let mut files = Vec::new();
    files_under(&src, &mut files);
    let mut found = Vec::new();
    for file in files.iter().filter(|f| f.extension().is_some_and(|e| e == "rs")) {
        // In `main.rs` the doc comments are the help of the command line; elsewhere a comment is not shown.
        let help = file == &src.join("main.rs");
        let text = read(file);
        for (i, line) in text.lines().enumerate() {
            if (help || !line.trim_start().starts_with("//")) && line.to_lowercase().contains(FORMER) {
                found.push(format!("{}:{}: {}", file.display(), i + 1, line.trim()));
            }
        }
    }
    assert!(found.is_empty(), "the former name is shown:\n{}", found.join("\n"));
}

#[test]
fn every_tool_and_parameter_description_writes_synthub() {
    for schemas in [osc_bridge::mcp::local_tool_schemas(), osc_bridge::synthub::mcp::linked_tool_schemas()] {
        let text = schemas.to_string();
        assert!(!text.to_lowercase().contains(FORMER), "{text}");
        assert!(text.contains("SyntHub"), "{text}");
    }
}

#[test]
fn the_instructions_of_the_linked_server_write_synthub() {
    let dir = temp("renamed-instructions");
    catalogue(&dir.path().join("server"));
    let server = FakeSynthHub::start(&dir.path().join("server"), None);
    let replies = mcp_session(&server.base, &dir.path().join("cache"), &[initialize()]);
    let text = replies[0]["result"]["instructions"].as_str().unwrap();
    assert!(text.contains("SyntHub catalogue") && text.contains("SyntHub server"), "{text}");
    assert!(!text.to_lowercase().contains(FORMER), "{text}");
}

#[test]
fn the_generated_settings_file_writes_synthub() {
    let text = Settings::default().render();
    assert!(text.contains("the SyntHub server"), "{text}");
    assert!(!text.to_lowercase().contains(FORMER), "{text}");
}

#[test]
fn the_privacy_messages_write_synthub() {
    let dir = temp("renamed-privacy");
    let path = dir.path().join("settings.toml");
    let index = osc_bridge::mcp::DeviceIndex::build(dir.path(), osc_bridge::synthub::catalogue::tier_rank);
    for offline in [false, true] {
        let text = osc_bridge::synthub::mcp::privacy(&path, &index, None, Some(offline), &[], &[]).unwrap();
        assert!(text.contains("SyntHub server"), "{text}");
        assert!(!text.to_lowercase().contains(FORMER), "{text}");
    }
}

#[test]
fn the_interface_of_the_crate_writes_synthub() {
    let text = read(&krate().join("docs/INTERFACE.md"));
    assert!(text.contains("SyntHub"), "no SyntHub in docs/INTERFACE.md");
    // `synthhub-protocol` is the name of a crate, not of the product.
    let found = lines_with("docs/INTERFACE.md", &text.replace("synthhub-protocol", ""), FORMER);
    assert!(found.is_empty(), "{}", found.join("\n"));
}

// ---- the published texts of the root ----

/// The published texts that describe the present; the dated records (the past entries of the
/// CHANGELOG, `BACKLOG.md`, `docs/MCP_DIRECTORIES.md`) are left as they were.
const PUBLISHED: &[&str] = &[
    "README.md",
    "CONTRIBUTING.md",
    "PRIVACY.md",
    "docs/MCP.md",
    "docs/getting-started.md",
    "docs/TUTORIAL_FIRST_DEVICE.md",
    "docs/README.md",
    "docs/CONFIGURATION_COVERAGE.md",
    "examples/bridge.toml",
    "npm/README.md",
    "npm/package.json",
    "server.json",
    "mcpb/manifest.json",
    ".claude-plugin/plugin.json",
];

#[test]
fn no_published_text_of_the_root_writes_the_former_name_or_address() {
    let mut files: Vec<PathBuf> = PUBLISHED.iter().map(|f| root().join(f)).collect();
    files_under(&root().join(".github"), &mut files);
    let found: Vec<String> =
        files.iter().flat_map(|f| lines_with(&f.display().to_string(), &read(f), FORMER)).collect();
    assert!(found.is_empty(), "the former name or address is written:\n{}", found.join("\n"));
}

#[test]
fn the_published_texts_give_the_service_address_and_the_site_pages_at_the_same_paths() {
    const MCP: &str = "https://mcp.synthub.roomi-fields.com/mcp";
    const SITE: &str = "https://synthub.roomi-fields.com/";
    let expected: &[(&str, &[&str])] = &[
        ("README.md", &[MCP, SITE, "https://synthub.roomi-fields.com/api/v1/index.json", "https://synthub.roomi-fields.com/connect/"]),
        ("CONTRIBUTING.md", &[SITE]),
        ("PRIVACY.md", &["mcp.synthub.roomi-fields.com", "https://synthub.roomi-fields.com/api/"]),
        ("docs/MCP.md", &[MCP, SITE, "https://synthub.roomi-fields.com/connect/",
            "https://synthub.roomi-fields.com/api/#reports-and-trust", "https://synthub.roomi-fields.com/api/#shared-presets"]),
        ("docs/getting-started.md", &["https://synthub.roomi-fields.com/catalogue/",
            "https://synthub.roomi-fields.com/connect/", "https://synthub.roomi-fields.com/blog/"]),
        ("docs/TUTORIAL_FIRST_DEVICE.md", &[SITE]),
        ("npm/README.md", &[MCP, SITE]),
        (".github/workflows/docs.yml", &[SITE]),
    ];
    for (file, links) in expected {
        let text = read(&root().join(file));
        for link in *links {
            assert!(text.contains(link), "{file} does not give {link}");
        }
    }
}

#[test]
fn the_published_manifests_point_to_the_site_and_the_server() {
    let json_of = |file: &str| -> Value { serde_json::from_str(&read(&root().join(file))).unwrap() };
    let server = json_of("server.json");
    assert_eq!(server["websiteUrl"], "https://synthub.roomi-fields.com/");
    let remotes = server["remotes"].to_string();
    assert!(remotes.contains("https://mcp.synthub.roomi-fields.com/mcp"), "{remotes}");
    for file in ["npm/package.json", "mcpb/manifest.json"] {
        assert_eq!(json_of(file)["homepage"], "https://synthub.roomi-fields.com/", "{file}");
    }
    for file in ["server.json", "npm/package.json", "mcpb/manifest.json", ".claude-plugin/plugin.json"] {
        assert!(read(&root().join(file)).contains("SyntHub"), "{file} writes SyntHub");
    }
}

// ---- the record of the decision ----

#[test]
fn the_decision_on_a_copy_too_old_carries_its_dated_amendment() {
    let text = read(&krate().join("docs/RENAME-SYNTHUB.md"));
    let d7 = text.lines().find(|l| l.starts_with("| D7 |")).expect("the row D7");
    assert!(d7.contains("2026-11-25"), "{d7}");
}
