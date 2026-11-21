//! An output is paired with the input of the same full name (ALSA: same client, same port, same
//! name); only an output with no such input falls back to the nearest name.

use osc_bridge::capture::{Asound, Capture};
use osc_bridge::detect::replay;
use std::path::Path;

fn capture_of(outputs: &[&str], inputs: &[&str]) -> Capture {
    let own = |l: &[&str]| l.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    Capture::new(own(outputs), own(inputs), Asound { cards: String::new(), usbids: vec![] })
}

/// The (output name, input name) pairs the replay gives, in the outputs' order.
fn pairs(outputs: &[&str], inputs: &[&str]) -> Vec<(String, Option<String>)> {
    replay(&capture_of(outputs, inputs)).into_iter().filter_map(|d| d.out_name.map(|o| (o, d.in_name))).collect()
}

fn pair(o: &str, i: &str) -> (String, Option<String>) {
    (o.to_string(), Some(i.to_string()))
}

const CTRL: &str = "Behringer BCR-32:Behringer BCR-32 BCR-32 CTRL 24:0";
const ZAQ: &str = "Behringer BCR-32:Behringer BCR-32 BCR-32 ZAQ 24:1";
const MIDI: &str = "Minilab3:Minilab3 MIDI 40:0";
const ALV: &str = "Minilab3:Minilab3 ALV 40:3";

#[test]
fn ports_of_one_device_that_differ_by_their_last_word_pair_with_their_own_name() {
    let p = pairs(&[CTRL, ZAQ, MIDI, ALV], &[CTRL, ZAQ, MIDI, ALV]);
    assert_eq!(p, vec![pair(CTRL, CTRL), pair(ZAQ, ZAQ), pair(MIDI, MIDI), pair(ALV, ALV)]);
}

#[test]
fn the_pairing_does_not_depend_on_the_order_of_the_inputs() {
    let p = pairs(&[CTRL, ZAQ, MIDI, ALV], &[ALV, MIDI, ZAQ, CTRL]);
    assert_eq!(p, vec![pair(CTRL, CTRL), pair(ZAQ, ZAQ), pair(MIDI, MIDI), pair(ALV, ALV)]);
}

#[test]
fn the_bench_capture_of_the_corpus_pairs_each_output_with_its_own_input() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/captures/2026-11-21-ports.json");
    let capture = Capture::parse(&std::fs::read_to_string(path).unwrap()).unwrap();
    let found: Vec<_> = replay(&capture).into_iter().filter(|d| d.out_name.is_some()).collect();
    assert_eq!(found.len(), 4);
    for d in found {
        assert_eq!(d.in_name, d.out_name, "output {:?}", d.out_name);
        assert_eq!(d.in_index, d.out_index);
    }
}

#[test]
fn an_output_without_an_input_of_its_name_falls_back_to_the_nearest_name() {
    let renamed = "Minilab3:Minilab3 MIDI 41:0";
    assert_eq!(pairs(&[renamed], &[MIDI]), vec![pair(renamed, MIDI)]);
}

#[test]
fn every_output_takes_its_same_name_input_before_any_output_falls_back() {
    // The first output has no input of its name; the second has one, which the first must not take.
    let renamed = "Minilab3:Minilab3 MIDI 41:0";
    let p = pairs(&[renamed, MIDI], &[MIDI, ALV]);
    assert_eq!(p[1], pair(MIDI, MIDI));
    assert_eq!(p[0], pair(renamed, ALV));
}

#[test]
fn the_full_name_includes_the_client_and_port_suffix() {
    let a = "Dev:Dev Port 20:0";
    let b = "Dev:Dev Port 20:1";
    assert_eq!(pairs(&[b, a], &[a, b]), vec![pair(b, b), pair(a, a)]);
}

#[test]
fn two_inputs_of_the_same_name_are_taken_in_list_order_each_once() {
    let mut found = replay(&capture_of(&[MIDI, MIDI], &[MIDI, ALV, MIDI]));
    found.retain(|d| d.out_name.is_some());
    assert_eq!(found[0].in_index, Some(0));
    assert_eq!(found[1].in_index, Some(2));
}

#[test]
fn the_fallback_takes_the_lowest_input_among_equal_common_words() {
    let out = "Dev:Dev Alpha 20:0";
    let p = pairs(&[out], &["Dev:Dev Beta 21:0", "Dev:Dev Gamma 22:0"]);
    assert_eq!(p, vec![pair(out, "Dev:Dev Beta 21:0")]);
}

#[test]
fn the_fallback_never_crosses_to_a_sibling_port_whose_input_exists() {
    let renamed = "Behringer BCR-32:Behringer BCR-32 BCR-32 CTRL 25:0";
    let p = pairs(&[renamed, ZAQ], &[ZAQ, CTRL]);
    assert_eq!(p[1], pair(ZAQ, ZAQ));
    assert_eq!(p[0], pair(renamed, CTRL));
}
