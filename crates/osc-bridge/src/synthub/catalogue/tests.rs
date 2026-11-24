use super::*;

fn cache(tag: &str) -> tempfile::TempDir {
    let dir = tempfile::Builder::new().prefix(&format!("osc-bridge-locate-{tag}-")).tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("acme")).unwrap();
    std::fs::write(dir.path().join("acme/box.json"), r#"{"device":{"name":"Box","vendor":"Acme","osc_prefix":"/box"}}"#).unwrap();
    dir
}

#[test]
fn a_file_on_this_machine_is_used_as_it_is() {
    let dir = cache("file");
    let root = dir.path();
    let file = root.join("acme/box.json");
    assert_eq!(locate(&file).unwrap(), file);
}

#[test]
fn offline_a_driver_is_found_in_the_cache_by_id_or_slug() {
    let dir = cache("offline");
    let root = dir.path();
    assert_eq!(from_cache_or_server("acme/box", root, None).unwrap(), root.join("acme/box.json"));
    assert_eq!(from_cache_or_server("box", root, None).unwrap(), root.join("acme/box.json"));
    let missing = from_cache_or_server("acme/other", root, None).unwrap_err().to_string();
    assert!(missing.contains("not in the drivers cached") && missing.contains("offline"), "{missing}");
    assert!(locate(Path::new("no/such/file.json")).unwrap_err().to_string().contains("no such file"));
}

#[test]
fn an_unreachable_server_falls_back_to_the_cache() {
    let dir = cache("unreachable");
    let root = dir.path();
    let server = Online::keeping_account_in("http://127.0.0.1:9", Some(root.join("user")));
    assert_eq!(from_cache_or_server("acme/box", root, Some(&server)).unwrap(), root.join("acme/box.json"));
    assert!(from_cache_or_server("acme/other", root, Some(&server)).is_err());
}

#[test]
fn each_source_type_of_the_format_has_its_rank() {
    let ranks = [
        ("hardware-verified", 0), ("software-verified", 0),
        ("hardware-observed", 1), ("vendor-doc", 1), ("vendor-self-described", 1),
        ("vendor-osc-api", 2), ("third-party-osc", 2),
        ("electra-preset", 3),
        ("pencilresearch", 4), ("knobkraft", 4), ("midnam", 4), ("community-re", 4),
        ("contributed", 5),
        ("test-fixture", 6),
    ];
    for (tier, rank) in ranks {
        assert_eq!(tier_rank(tier), rank, "{tier}");
    }
}

#[test]
fn an_extension_or_unknown_source_type_ranks_lowest() {
    for tier in ["x-my-source", "x-", "nonsense", ""] {
        assert_eq!(tier_rank(tier), 7, "{tier:?}");
    }
}

#[test]
fn terms_the_format_does_not_name_rank_lowest() {
    for tier in ["vendor", "agent-contributed"] {
        assert_eq!(tier_rank(tier), 7, "{tier}");
    }
}
