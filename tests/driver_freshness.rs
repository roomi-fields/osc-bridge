//! A driver fetched from the SynthHub server is kept with the ETag the server
//! gave, and asked for again with `If-None-Match`: on `304 Not Modified` the
//! kept file is used without being downloaded, on `200` it is replaced, and
//! when the server cannot be reached it is used as it is. osc-bridge asks
//! only for the drivers a session uses, never for the whole catalogue.
//!
//! No network: the server is a fake one on a local port (`common`).

mod common;

use common::{catalogue, mcp_session, temp, texts, tool_call, FakeSynthHub};
use osc_bridge::online::Online;
use serde_json::json;
use std::path::Path;

fn etag_of(file: &Path) -> String {
    format!("\"{}\"", common::sha256_hex(&std::fs::read(file).unwrap()))
}

#[test]
fn a_kept_driver_is_asked_again_with_its_etag_and_used_on_304() {
    let dir = temp("fresh-304");
    let (root, cache) = (dir.path().join("server"), dir.path().join("cache"));
    catalogue(&root);
    let server = FakeSynthHub::start(&root, None);

    let path = Online::new(&server.base).fetch_driver("acme/box", &cache).unwrap();
    assert_eq!(path, cache.join("acme/box.json"));
    let first = server.driver_requests();
    assert_eq!(first.iter().map(|s| s.url.as_str()).collect::<Vec<_>>(),
        ["/api/v1/devices/acme/box.json", "/api/v1/devices/acme/box.md", "/api/v1/devices/acme/_profile.json"]);
    assert!(first.iter().all(|s| s.if_none_match.is_none() && s.status == 200), "{first:?}");
    server.forget_requests();

    // A later run (a new client): each kept file is asked for with its ETag.
    let again = Online::new(&server.base).fetch_driver("acme/box", &cache).unwrap();
    assert_eq!(again, path);
    let second = server.driver_requests();
    assert_eq!(second.len(), 3, "{second:?}");
    for (seen, file) in second.iter().zip(["acme/box.json", "acme/box.md", "acme/_profile.json"]) {
        assert_eq!(seen.if_none_match.as_deref(), Some(etag_of(&root.join(file)).as_str()), "{seen:?}");
        assert_eq!(seen.status, 304, "{seen:?}");
        assert_eq!(std::fs::read(cache.join(file)).unwrap(), std::fs::read(root.join(file)).unwrap(),
            "{file}: the kept copy is used as it is");
    }
}

#[test]
fn a_changed_driver_replaces_the_kept_one_and_its_etag() {
    let dir = temp("fresh-200");
    let (root, cache) = (dir.path().join("server"), dir.path().join("cache"));
    catalogue(&root);
    let server = FakeSynthHub::start(&root, None);
    Online::new(&server.base).fetch_driver("acme/synth", &cache).unwrap();

    let changed = r#"{"device":{"name":"Synth II","vendor":"Acme","osc_prefix":"/synth"}}"#;
    std::fs::write(root.join("acme/synth.json"), changed).unwrap();
    server.forget_requests();
    Online::new(&server.base).fetch_driver("acme/synth", &cache).unwrap();
    let asked = server.driver_requests();
    assert_eq!((asked[0].url.as_str(), asked[0].status), ("/api/v1/devices/acme/synth.json", 200), "{asked:?}");
    assert!(asked[0].if_none_match.is_some(), "the old ETag was sent: {asked:?}");
    assert_eq!(std::fs::read_to_string(cache.join("acme/synth.json")).unwrap(), changed);

    // The new ETag is the one kept: the next fetch is a 304.
    server.forget_requests();
    Online::new(&server.base).fetch_driver("acme/synth", &cache).unwrap();
    let asked = server.driver_requests();
    assert_eq!(asked[0].if_none_match.as_deref(), Some(etag_of(&root.join("acme/synth.json")).as_str()));
    assert_eq!(asked[0].status, 304);
}

#[test]
fn notes_the_server_no_longer_has_are_dropped_with_their_etag() {
    let dir = temp("fresh-notes");
    let (root, cache) = (dir.path().join("server"), dir.path().join("cache"));
    catalogue(&root);
    let server = FakeSynthHub::start(&root, None);
    Online::new(&server.base).fetch_driver("acme/synth", &cache).unwrap();
    assert!(cache.join("acme/synth.md").is_file());

    std::fs::remove_file(root.join("acme/synth.md")).unwrap();
    Online::new(&server.base).fetch_driver("acme/synth", &cache).unwrap();
    assert!(!cache.join("acme/synth.md").exists());
    std::fs::write(root.join("acme/synth.md"), "# Synth\n\nNew notes.\n").unwrap();
    server.forget_requests();
    Online::new(&server.base).fetch_driver("acme/synth", &cache).unwrap();
    let notes = server.driver_requests().into_iter().find(|s| s.url.ends_with("synth.md")).unwrap();
    assert_eq!((notes.if_none_match, notes.status), (None, 200), "no ETag kept for notes that went away");
    assert_eq!(std::fs::read_to_string(cache.join("acme/synth.md")).unwrap(), "# Synth\n\nNew notes.\n");
}

#[test]
fn an_unreachable_server_leaves_the_kept_driver_in_use() {
    let dir = temp("fresh-offline");
    let (root, cache) = (dir.path().join("server"), dir.path().join("cache"));
    catalogue(&root);
    let server = FakeSynthHub::start(&root, None);
    Online::new(&server.base).fetch_driver("acme/box", &cache).unwrap();
    let kept = std::fs::read(cache.join("acme/box.json")).unwrap();

    let unreachable = Online::new("http://127.0.0.1:9");
    let path = osc_bridge::catalogue::from_cache_or_server("acme/box", &cache, Some(&unreachable)).unwrap();
    assert_eq!(path, cache.join("acme/box.json"));
    assert_eq!(std::fs::read(&path).unwrap(), kept);
}

#[test]
fn a_session_with_two_devices_asks_only_for_those_two_drivers() {
    let dir = temp("fresh-session");
    let (root, cache) = (dir.path().join("server"), dir.path().join("cache"));
    catalogue(&root);
    // Drivers the session does not use, which must stay on the server.
    std::fs::create_dir_all(root.join("other")).unwrap();
    std::fs::write(root.join("other/thing.json"), r#"{"device":{"name":"Thing","vendor":"Other","osc_prefix":"/thing"}}"#).unwrap();
    let server = FakeSynthHub::start(&root, None);
    let session = [
        common::initialize(),
        tool_call(1, "list_routes", json!({ "slug": "box" })),
        tool_call(2, "get_device_docs", json!({ "slug": "synth" })),
        tool_call(3, "list_routes", json!({ "slug": "box" })),
    ];

    let replies = mcp_session(&server.base, &cache, &session);
    assert_eq!(replies.len(), 4, "{replies:?}");
    for reply in &replies[1..] {
        assert_ne!(reply["result"]["isError"], true, "{reply}");
        assert!(texts(reply).iter().all(|t| !t.contains("newer osc-bridge")), "no notice without Deprecation: {reply}");
    }
    let urls = |seen: &[common::Seen]| seen.iter().map(|s| s.url.clone()).collect::<Vec<_>>();
    assert_eq!(urls(&server.driver_requests()), ["/api/v1/devices/acme/box.json", "/api/v1/devices/acme/box.md",
        "/api/v1/devices/acme/_profile.json", "/api/v1/devices/acme/synth.json", "/api/v1/devices/acme/synth.md"],
        "each driver once, with its notes and profile, and nothing else of the catalogue");
    let all = server.requests.lock().unwrap().clone();
    let others: Vec<&str> = all.iter().map(|s| s.url.as_str()).filter(|u| !u.starts_with("/api/v1/devices/acme/")).collect();
    assert!(others.iter().all(|u| ["/v1/devices/box", "/v1/devices/synth", "/v1/trust"].contains(u)), "{all:?}");
    assert!(!cache.join("other").exists(), "the drivers not used are not on this machine");

    // The next session asks for the same files with their ETags, and uses the kept ones.
    server.forget_requests();
    let replies = mcp_session(&server.base, &cache, &session);
    assert_ne!(replies[2]["result"]["isError"], true, "{:?}", replies[2]);
    let again = server.driver_requests();
    assert_eq!(again.len(), 5, "{again:?}");
    assert!(again.iter().all(|s| s.if_none_match.is_some() && s.status == 304), "{again:?}");
}
