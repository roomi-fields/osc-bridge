//! The client downloads a driver with the files `osd` says it is built from
//! (its shared MIDI and its maker profiles), asking the server through a
//! `Files` that keeps what it fetched in memory; the cache is written only
//! once `osd::resolve` has succeeded, and a driver `osd` refuses fails the
//! download with `osd`'s fault, the cached copy left as it is.
//!
//! No network: the server is a fake one on a local port (`common`).

use crate::common::{device05, files, online_in, temp, FakeSynthHub, D05, P05};
use std::path::Path;

fn profile(body: &str) -> String {
    format!(r#"{{"$schema":"{P05}",{body}}}"#)
}

fn server_with(list: &[(&str, &str)]) -> (tempfile::TempDir, FakeSynthHub) {
    let dir = temp("download");
    files(&dir.path().join("server"), list);
    let server = FakeSynthHub::start(&dir.path().join("server"), None);
    (dir, server)
}

fn urls(server: &FakeSynthHub) -> Vec<String> {
    let mut u: Vec<String> = server.driver_requests().into_iter().map(|s| s.url).collect();
    u.sort();
    u
}

fn kept(cache: &Path) -> Vec<String> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<String>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd {
            let p = e.unwrap().path();
            if p.is_dir() { walk(&p, root, out) } else { out.push(p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/")) }
        }
    }
    let mut out = Vec::new();
    walk(cache, cache, &mut out);
    out.sort();
    out
}

fn fault_of(err: &anyhow::Error) -> &osd::Fault {
    err.downcast_ref::<osd::Fault>().unwrap_or_else(|| panic!("the cause is not an osd fault: {err:#}"))
}

#[test]
fn a_driver_sharing_another_files_midi_is_downloaded_with_that_file_and_its_profiles() {
    let stock = device05("Box", "/box", r#","identity":{"profile":"acme","ports":["Box"]},"commands":[{"osc":"/init","frame":[240,247]}]"#);
    let fork = format!(r#"{{"$schema":"{D05}","device":{{"name":"Box","osc_prefix":"/fork"}},"midi_of":"acme/stock"}}"#);
    let (dir, server) = server_with(&[
        ("acme/fork.json", &fork), ("acme/fork.md", "# Fork"), ("acme/stock.json", &stock),
        ("acme/_profile.json", &profile(r#""identity":{"family":[1]}"#)),
    ]);
    let cache = dir.path().join("cache");
    let path = online_in(&server.base, dir.path()).fetch_driver("acme/fork", &cache).unwrap();
    assert_eq!(path, cache.join("acme/fork.json"));
    let have = kept(&cache);
    for f in ["acme/fork.json", "acme/fork.md", "acme/stock.json", "acme/_profile.json"] {
        assert!(have.iter().any(|h| h == f), "{f} is kept: {have:?}");
    }
    let dev = osc_bridge::device::load(&path).unwrap();
    assert_eq!(dev.device.osc_prefix, "/fork");
    assert_eq!(dev.commands.as_ref().map(Vec::len), Some(1), "the shared routes came with it");
    assert_eq!(dev.identity.unwrap().family, Some(vec![1]), "and the profile of the shared file");
}

#[test]
fn profiles_that_name_profiles_are_followed_as_osd_resolves_them() {
    let model = device05("Box", "/box", r#","identity":{"profile":"acme/groove","ports":["Box"]},"programs":{"profile":"acme","banks":[{"name":"A","size":8}]}"#);
    let (dir, server) = server_with(&[
        ("acme/box.json", &model),
        ("acme/_profile.groove.json", &profile(r#""identity":{"profile":"acme","family":[2]}"#)),
        ("acme/_profile.json", &profile(r#""identity":{"member":[3]},"programs":{"selects":"sound"}"#)),
    ]);
    let cache = dir.path().join("cache");
    online_in(&server.base, dir.path()).fetch_driver("acme/box", &cache).unwrap();
    let have = kept(&cache);
    for f in ["acme/_profile.groove.json", "acme/_profile.json", "acme/box.json"] {
        assert!(have.iter().any(|h| h == f), "{f} is kept: {have:?}");
    }
    let dev = osc_bridge::device::load(cache.join("acme/box.json")).unwrap();
    let id = dev.identity.unwrap();
    assert_eq!((id.family, id.member), (Some(vec![2]), Some(vec![3])));
}

#[test]
fn a_driver_osd_refuses_fails_the_download_with_osds_fault_and_the_cached_copy_stays() {
    let good = device05("Synth", "/synth", "");
    let (dir, server) = server_with(&[("acme/synth.json", &good)]);
    let cache = dir.path().join("cache");
    online_in(&server.base, dir.path()).fetch_driver("acme/synth", &cache).unwrap();
    let before = std::fs::read(cache.join("acme/synth.json")).unwrap();
    let etag_before = std::fs::read(cache.join("acme/synth.json.etag")).unwrap();

    files(&dir.path().join("server"), &[("acme/synth.json", &device05("Synth", "/synth", r#","wat":1"#))]);
    let err = online_in(&server.base, dir.path()).fetch_driver("acme/synth", &cache).unwrap_err();
    let fault = fault_of(&err);
    assert_eq!((fault.kind, fault.pointer.as_str()), (osd::FaultKind::UnknownField, "/wat"), "{err:#}");
    assert_eq!(std::fs::read(cache.join("acme/synth.json")).unwrap(), before, "the copy in the cache stays");
    assert_eq!(std::fs::read(cache.join("acme/synth.json.etag")).unwrap(), etag_before, "with its own ETag");
}

#[test]
fn a_driver_that_is_not_json_fails_the_download_with_osds_fault() {
    let (dir, server) = server_with(&[("acme/bad.json", "{ not json")]);
    let cache = dir.path().join("cache");
    let err = online_in(&server.base, dir.path()).fetch_driver("acme/bad", &cache).unwrap_err();
    assert_eq!(fault_of(&err).kind, osd::FaultKind::Syntax, "{err:#}");
    assert!(kept(&cache).is_empty(), "nothing is kept: {:?}", kept(&cache));
}

#[test]
fn when_the_resolution_fails_after_several_files_were_fetched_nothing_is_written() {
    // fork -> stock (fetched) -> a profile the server does not have.
    let stock = device05("Box", "/box", r#","identity":{"profile":"acme/none","ports":["Box"]}"#);
    let fork = format!(r#"{{"$schema":"{D05}","device":{{"name":"Box","osc_prefix":"/fork"}},"midi_of":"acme/stock"}}"#);
    let (dir, server) = server_with(&[("acme/fork.json", &fork), ("acme/fork.md", "# Fork"), ("acme/stock.json", &stock)]);
    let cache = dir.path().join("cache");
    let err = online_in(&server.base, dir.path()).fetch_driver("acme/fork", &cache).unwrap_err();
    assert_eq!(fault_of(&err).kind, osd::FaultKind::LinkMissing, "{err:#}");
    let asked = urls(&server);
    assert!(asked.iter().any(|u| u.ends_with("acme/stock.json")) && asked.iter().any(|u| u.ends_with("acme/fork.json")), "the files were fetched: {asked:?}");
    assert!(kept(&cache).is_empty(), "nothing in the cache: {:?}", kept(&cache));
}

#[test]
fn when_the_resolution_fails_the_copy_already_kept_stays_intact() {
    let fork = |note: &str| format!(r#"{{"$schema":"{D05}","device":{{"name":"Box","osc_prefix":"/fork"}},"midi_of":"acme/stock","_about":"{note}"}}"#);
    let stock = device05("Box", "/box", "");
    let (dir, server) = server_with(&[("acme/fork.json", &fork("one")), ("acme/stock.json", &stock)]);
    let cache = dir.path().join("cache");
    online_in(&server.base, dir.path()).fetch_driver("acme/fork", &cache).unwrap();
    let snapshot: Vec<(String, Vec<u8>)> = kept(&cache).into_iter().map(|f| { let b = std::fs::read(cache.join(&f)).unwrap(); (f, b) }).collect();

    // The server now has a new fork, whose shared file has gone.
    files(&dir.path().join("server"), &[("acme/fork.json", &fork("two"))]);
    std::fs::remove_file(dir.path().join("server/acme/stock.json")).unwrap();
    let err = online_in(&server.base, dir.path()).fetch_driver("acme/fork", &cache).unwrap_err();
    assert!(err.downcast_ref::<osd::Fault>().is_some() || format!("{err:#}").contains("stock"), "{err:#}");
    let after: Vec<(String, Vec<u8>)> = kept(&cache).into_iter().map(|f| { let b = std::fs::read(cache.join(&f)).unwrap(); (f, b) }).collect();
    assert_eq!(after, snapshot, "the cache is as it was");
}

#[test]
fn a_shared_file_the_server_does_not_have_is_a_link_missing_fault() {
    let fork = format!(r#"{{"$schema":"{D05}","device":{{"name":"Box","osc_prefix":"/fork"}},"midi_of":"acme/gone"}}"#);
    let (dir, server) = server_with(&[("acme/fork.json", &fork)]);
    let cache = dir.path().join("cache");
    let err = online_in(&server.base, dir.path()).fetch_driver("acme/fork", &cache).unwrap_err();
    assert_eq!(fault_of(&err).kind, osd::FaultKind::LinkMissing, "{err:#}");
    assert!(kept(&cache).is_empty());
}

#[test]
fn a_driver_the_server_does_not_have_is_an_error_naming_it() {
    let (dir, server) = server_with(&[("acme/other.json", &device05("O", "/o", ""))]);
    let err = online_in(&server.base, dir.path()).fetch_driver("acme/none", &dir.path().join("cache")).unwrap_err();
    assert!(format!("{err:#}").contains("acme/none"), "{err:#}");
}

#[test]
fn a_chain_of_shared_midi_deeper_than_osd_allows_fails_with_its_fault() {
    let mut list = vec![("acme/l0.json".to_string(), device05("Box", "/b", ""))];
    for n in 1..=9 {
        list.push((format!("acme/l{n}.json"), device05("Box", "/b", &format!(r#","midi_of":"acme/l{}""#, n - 1))));
    }
    let refs: Vec<(&str, &str)> = list.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    let (dir, server) = server_with(&refs);
    let cache = dir.path().join("cache");
    assert!(online_in(&server.base, dir.path()).fetch_driver("acme/l8", &cache).is_ok());
    let err = online_in(&server.base, dir.path()).fetch_driver("acme/l9", &dir.path().join("cache9")).unwrap_err();
    assert_eq!(fault_of(&err).kind, osd::FaultKind::LinkTooDeep, "{err:#}");
}

#[test]
fn files_already_kept_are_downloaded_again_only_when_the_servers_copy_changed() {
    let stock = device05("Box", "/box", r#","identity":{"profile":"acme","ports":["Box"]}"#);
    let fork = format!(r#"{{"$schema":"{D05}","device":{{"name":"Box","osc_prefix":"/fork"}},"midi_of":"acme/stock"}}"#);
    let (dir, server) = server_with(&[("acme/fork.json", &fork), ("acme/stock.json", &stock), ("acme/_profile.json", &profile(r#""identity":{"family":[1]}"#))]);
    let cache = dir.path().join("cache");
    online_in(&server.base, dir.path()).fetch_driver("acme/fork", &cache).unwrap();
    server.forget_requests();

    // The profile changes on the server; the others do not.
    files(&dir.path().join("server"), &[("acme/_profile.json", &profile(r#""identity":{"family":[9]}"#))]);
    online_in(&server.base, dir.path()).fetch_driver("acme/fork", &cache).unwrap();
    let json_requests: Vec<_> = server.driver_requests().into_iter().filter(|s| s.url.ends_with(".json")).collect();
    let status = |suffix: &str| json_requests.iter().find(|s| s.url.ends_with(suffix)).unwrap_or_else(|| panic!("{suffix}: {json_requests:?}")).status;
    assert_eq!(status("acme/fork.json"), 304);
    assert_eq!(status("acme/stock.json"), 304);
    assert_eq!(status("acme/_profile.json"), 200, "the changed copy is downloaded again");
    assert_eq!(json_requests.len(), 3, "each file once: {json_requests:?}");
    let dev = osc_bridge::device::load(cache.join("acme/fork.json")).unwrap();
    assert_eq!(dev.identity.unwrap().family, Some(vec![9]), "the cache holds the new profile");
}

#[test]
fn a_driver_that_names_nothing_else_is_downloaded_with_its_note_alone() {
    let (dir, server) = server_with(&[("acme/synth.json", &device05("Synth", "/synth", "")), ("acme/synth.md", "# Synth")]);
    let cache = dir.path().join("cache");
    online_in(&server.base, dir.path()).fetch_driver("acme/synth", &cache).unwrap();
    assert_eq!(urls(&server), ["/api/v1/devices/acme/synth.json", "/api/v1/devices/acme/synth.md"]);
    assert_eq!(kept(&cache), ["acme/synth.json", "acme/synth.json.etag", "acme/synth.json.lock", "acme/synth.md", "acme/synth.md.etag", "acme/synth.md.lock"]);
}

#[test]
fn a_driver_id_that_could_leave_the_cache_is_refused_before_any_request() {
    let (dir, server) = server_with(&[("acme/synth.json", &device05("Synth", "/synth", ""))]);
    for bad in ["../x", "acme/../x", "acme", "a/b/c", "acme/.hidden"] {
        assert!(online_in(&server.base, dir.path()).fetch_driver(bad, &dir.path().join("cache")).is_err(), "{bad}");
    }
    assert!(server.driver_requests().is_empty(), "{:?}", server.driver_requests());
}
