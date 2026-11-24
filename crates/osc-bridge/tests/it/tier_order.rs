//! Which card wins when two describe one device is an order the caller gives: the index takes it
//! as a value, and the catalogue's own order (`synthub::catalogue::tier_rank`) is what the
//! application gives.

use osc_bridge::mcp::{DeviceIndex, DeviceIndexEntry};
use osc_bridge::synthub::catalogue::tier_rank;
use std::collections::HashMap;
use std::path::Path;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/devices");

fn reversed(tier: &str) -> u8 { 255 - tier_rank(tier) }

#[test]
fn the_catalogue_order_ranks_a_tested_device_before_a_vendor_doc_before_the_unknown() {
    assert!(tier_rank("hardware-verified") < tier_rank("vendor-doc"));
    assert!(tier_rank("vendor-doc") < tier_rank("contributed"));
    assert_eq!(tier_rank("no-such-tier"), 7, "an unknown tier comes last");
}

#[test]
fn a_shared_slug_resolves_to_the_most_trustworthy_variant() {
    let idx = DeviceIndex::build(Path::new(FIXTURES), tier_rank);
    let mut by_slug: HashMap<&str, Vec<&DeviceIndexEntry>> = HashMap::new();
    for e in &idx.entries { by_slug.entry(e.slug.as_str()).or_default().push(e); }
    let mut checked = 0;
    for (slug, variants) in by_slug.iter().filter(|(s, v)| !s.is_empty() && v.len() > 1) {
        let best = variants.iter().map(|e| tier_rank(&e.tier)).min().unwrap();
        let chosen = idx.find_path(slug).unwrap();
        let chosen_tier = &variants.iter().find(|e| e.path == chosen).unwrap().tier;
        assert_eq!(tier_rank(chosen_tier), best, "{slug} resolved to a weaker variant");
        checked += 1;
    }
    assert!(checked > 0, "the test drivers hold variants of one device");
    assert_eq!(idx.find_path("box"), Some(Path::new(FIXTURES).join("acme/box.vendor.fw-2.json").as_path()),
        "the vendor's documentation wins over an unreviewed contribution");
}

#[test]
fn an_inverted_order_picks_the_other_card_for_the_same_prefix() {
    let today = DeviceIndex::build(Path::new(FIXTURES), tier_rank);
    let inverted = DeviceIndex::build(Path::new(FIXTURES), reversed);
    let (a, b) = (today.find_path("box").unwrap(), inverted.find_path("box").unwrap());
    assert_ne!(a, b, "the order given decides which card holds the slug");
    assert_eq!(b, Path::new(FIXTURES).join("acme/box.json"), "the least trustworthy card wins under the inverted order");
}

#[test]
fn cards_of_equal_rank_are_told_apart_by_path() {
    let neutral = DeviceIndex::build(Path::new(FIXTURES), |_| 0);
    assert_eq!(neutral.find_path("box"), Some(Path::new(FIXTURES).join("acme/box.json").as_path()),
        "the path that sorts first wins at equal rank");
}
