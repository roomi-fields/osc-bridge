use super::*;

#[test]
fn settings_default_to_sharing_and_round_trip() {
    let dir = tempfile::Builder::new().prefix("osc-bridge-settings-").tempdir().unwrap();
    let path = dir.path().join("osc-bridge/settings.toml");
    assert_eq!(Settings::load(Some(&path)).unwrap(), Settings { offline: false, private: vec![] });
    assert_eq!(Settings::load(None).unwrap(), Settings::default());
    let mine = Settings { offline: true, private: vec!["korg/minilogue".into()] };
    mine.save(&path).unwrap();
    assert_eq!(Settings::load(Some(&path)).unwrap(), mine);
    std::fs::write(&path, "private = [\"arturia/minilab3\"]\n").unwrap();
    assert_eq!(Settings::load(Some(&path)).unwrap(), Settings { offline: false, private: vec!["arturia/minilab3".into()] });
    let text = mine.render();
    assert!(text.contains("# true: do not contact the server") && text.contains("\"korg/minilogue\","), "{text}");
    assert_eq!(toml::from_str::<Settings>(&text).unwrap(), mine, "the commented file reads back");
    let fresh = dir.path().join("fresh/settings.toml");
    assert_eq!(Settings::load_or_create(&fresh).unwrap(), Settings::default());
    assert!(std::fs::read_to_string(&fresh).unwrap().contains("Edit this file freely"));
    let p = vec!["korg/*".to_string(), "minilab3".to_string(), "moog/sub37".to_string()];
    assert!(Settings::covers(&p, "korg/minilogue", "minilogue"));
    assert!(Settings::covers(&p, "arturia/minilab3.vendor.fw-1", "minilab3"));
    assert!(Settings::covers(&p, "moog/sub37", "sub37"));
    assert!(!Settings::covers(&p, "korgish/x", "x") && !Settings::covers(&p, "moog/sub37_v2", "sub37_v2"));
}

#[test]
fn driver_ids_stay_inside_the_cache() {
    assert!(is_driver_id("korg/minilogue") && is_driver_id("arturia/microfreak.vendor.fw-freakpatch-0.3.0"));
    for bad in ["../etc/passwd", "/abs", "korg/../x", "korg//x", "a\\b", "korg", "a/b/c", "korg/.hidden", "korg/x y", ""] {
        assert!(!is_driver_id(bad), "{bad}");
    }
    assert_eq!(driver_path(Path::new("/c"), "korg/minilogue"), Path::new("/c/korg/minilogue.json"));
    assert_eq!(driver_path(Path::new("/c"), "arturia/x.fw-1.0"), Path::new("/c/arturia/x.fw-1.0.json"));
    assert_eq!(sha256_hex(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    assert_eq!(etag_path(Path::new("/c/korg/minilogue.json")), Path::new("/c/korg/minilogue.json.etag"));
}

#[test]
fn the_deprecation_link_is_read_from_a_link_header() {
    assert_eq!(deprecation_link(r#"<https://x.example/new>; rel="deprecation""#).as_deref(), Some("https://x.example/new"));
    assert_eq!(deprecation_link(r#"<https://a.example/>; rel="next", <https://x.example/new>; type="text/html"; rel=deprecation"#).as_deref(),
        Some("https://x.example/new"));
    assert_eq!(deprecation_link(r#"<https://x.example/new>; rel="alternate deprecation""#).as_deref(), Some("https://x.example/new"));
    assert_eq!(deprecation_link(r#"<https://a.example/>; rel="next""#), None);
    assert_eq!(deprecation_link("not a link"), None);
}

fn kept_dir() -> tempfile::TempDir {
    tempfile::Builder::new().prefix("osc-bridge-keep-").tempdir().unwrap()
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    v.sort();
    v
}

/// The file the lock of `keep` is taken on.
fn lock_file(file: &Path) -> PathBuf {
    let mut name = file.as_os_str().to_owned();
    name.push(".lock");
    PathBuf::from(name)
}

#[test]
fn a_kept_driver_waits_for_the_lock_then_is_written_with_its_etag() {
    let dir = kept_dir();
    let file = dir.path().join("acme/box.json");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    let lock = std::fs::OpenOptions::new().create(true).write(true).truncate(false).open(lock_file(&file)).unwrap();
    lock.lock().unwrap();

    let (done, finished) = std::sync::mpsc::channel();
    let target = file.clone();
    let writer = std::thread::spawn(move || {
        keep(&target, "new text", Some("\"v2\"")).unwrap();
        done.send(()).unwrap();
    });
    assert!(finished.recv_timeout(Duration::from_millis(300)).is_err(), "keep finished while the lock was held");
    assert!(!file.exists() && !etag_path(&file).exists(), "keep wrote while the lock was held: {:?}", names(file.parent().unwrap()));

    lock.unlock().unwrap();
    finished.recv_timeout(Duration::from_secs(60)).expect("keep ends once the lock is released");
    writer.join().unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "new text");
    assert_eq!(std::fs::read_to_string(etag_path(&file)).unwrap(), "\"v2\"");
}

#[test]
fn a_kept_file_is_replaced_by_renaming_and_leaves_only_itself_its_etag_and_its_lock() {
    let dir = kept_dir();
    let file = dir.path().join("acme/box.json");
    keep(&file, "old text", Some("\"v1\"")).unwrap();
    let (old_link, old_etag_link) = (dir.path().join("acme/old-link"), dir.path().join("acme/old-etag-link"));
    std::fs::hard_link(&file, &old_link).unwrap();
    std::fs::hard_link(etag_path(&file), &old_etag_link).unwrap();

    keep(&file, "new text", Some("\"v2\"")).unwrap();

    assert_eq!(std::fs::read_to_string(&old_link).unwrap(), "old text", "the former file stays whole: it was not written in place");
    assert_eq!(std::fs::read_to_string(&old_etag_link).unwrap(), "\"v1\"", "the former ETag file stays whole");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "new text");
    assert_eq!(std::fs::read_to_string(etag_path(&file)).unwrap(), "\"v2\"");
    std::fs::remove_file(old_link).unwrap();
    std::fs::remove_file(old_etag_link).unwrap();
    assert_eq!(names(file.parent().unwrap()), ["box.json", "box.json.etag", "box.json.lock"]);
}

#[test]
fn a_file_kept_without_an_etag_drops_the_etag_of_the_former_one() {
    let dir = kept_dir();
    let file = dir.path().join("acme/box.md");
    keep(&file, "old", Some("\"v1\"")).unwrap();
    keep(&file, "new", None).unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "new");
    assert!(!etag_path(&file).exists(), "an old ETag never stands for a new file");
    assert_eq!(names(file.parent().unwrap()), ["box.md", "box.md.lock"]);
}

#[test]
fn a_deprecation_date_to_come_does_not_announce_a_withdrawal() {
    let now = 1_800_000_000;
    assert!(!deprecation_announces("@1900000000", now), "a date to come");
    assert!(!deprecation_announces("@1800000001", now), "one second to come");
    assert!(deprecation_announces("@1700000000", now), "a date past");
    assert!(deprecation_announces("@1800000000", now), "a date that is now");
    assert!(deprecation_announces("true", now), "not a date");
    assert!(deprecation_announces("", now), "no value");
    assert!(deprecation_announces("@soon", now), "an unreadable date");
}
