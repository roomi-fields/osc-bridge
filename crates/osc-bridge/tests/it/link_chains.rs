//! Links put end to end: a link follows the output of another link.
//!
//! The orchestrator is assembled on a loaded device with no port (`BenchPath`):
//! the seam under test is the link table, the hand-over to the owning device
//! and the emission queue. The depth lives in one counter on the thread that
//! runs the chain; the cut at four links is said by `/bridge/link/error`.

use crate::common::no_reply;
use crossbeam_channel::{bounded, Receiver};
use osc_bridge::orchestrator::{BenchPath, MapRule, RouteRule};
use osc_bridge::runtime::SendOsc;
use rosc::{OscMessage, OscType};
use std::sync::{Arc, Mutex};

const CUTOFF: &str = "/test_synth/filter/cutoff";
const RESONANCE: &str = "/test_synth/filter/resonance";
const ATTACK: &str = "/test_synth/amp/attack";
const RELEASE: &str = "/test_synth/amp/release";
const WAVEFORM: &str = "/test_synth/osc/waveform";
const LFO_RATE: &str = "/test_synth/lfo/rate";

// Control numbers of the addresses above in the test synth.
const CC_CUTOFF: u8 = 74;
const CC_RESONANCE: u8 = 71;
const CC_ATTACK: u8 = 73;
const CC_RELEASE: u8 = 72;
const CC_WAVEFORM: u8 = 70;

fn rig(links: &[RouteRule]) -> (BenchPath, Receiver<Vec<u8>>) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/devices/examples/test_synth.json");
    let device = osc_bridge::device::load_with_docs(path).unwrap();
    let (tx, rx) = bounded::<Vec<u8>>(64);
    (BenchPath::new(vec![(device, tx)], links), rx)
}

fn link(from: &str, to: &str) -> RouteRule {
    RouteRule { from: from.into(), to: to.into(), map: None }
}

fn msg(addr: &str, v: i32) -> OscMessage {
    OscMessage { addr: addr.into(), args: vec![OscType::Int(v)] }
}

/// A send function that keeps what it is given.
fn recording() -> (SendOsc, Arc<Mutex<Vec<OscMessage>>>) {
    let seen: Arc<Mutex<Vec<OscMessage>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    (Arc::new(move |m| sink.lock().unwrap().push(m)), seen)
}

fn frames(rx: &Receiver<Vec<u8>>) -> Vec<Vec<u8>> {
    rx.try_iter().collect()
}

fn controls(frames: &[Vec<u8>]) -> Vec<u8> {
    frames.iter().map(|f| f[1]).collect()
}

fn faults(seen: &Mutex<Vec<OscMessage>>) -> Vec<String> {
    seen.lock().unwrap().iter().filter(|m| m.addr == "/bridge/link/error").map(|m| {
        m.args.iter().filter_map(|a| if let OscType::String(s) = a { Some(s.clone()) } else { None })
            .collect::<Vec<_>>().join(" ")
    }).collect()
}

#[test]
fn a_message_received_at_a_reaches_c_through_the_links_a_to_b_and_b_to_c() {
    let (path, rx) = rig(&[link(CUTOFF, RESONANCE), link(RESONANCE, ATTACK)]);
    let (send, seen) = recording();
    path.dispatch(msg(CUTOFF, 64), &send, &no_reply());
    let got = frames(&rx);
    assert_eq!(
        got,
        vec![vec![0xB0, CC_CUTOFF, 64], vec![0xB0, CC_RESONANCE, 64], vec![0xB0, CC_ATTACK, 64]],
        "one frame per device address, no duplicate"
    );
    assert!(faults(&seen).is_empty(), "a two-link chain is no fault");
}

#[test]
fn the_chain_is_followed_whatever_the_order_of_the_links_in_the_table() {
    let (path, rx) = rig(&[link(RESONANCE, ATTACK), link(CUTOFF, RESONANCE)]);
    path.dispatch(msg(CUTOFF, 10), &recording().0, &no_reply());
    assert_eq!(controls(&frames(&rx)), vec![CC_CUTOFF, CC_RESONANCE, CC_ATTACK]);
}

#[test]
fn a_message_entering_in_the_middle_of_the_chain_follows_only_the_rest() {
    let (path, rx) = rig(&[link(CUTOFF, RESONANCE), link(RESONANCE, ATTACK)]);
    path.dispatch(msg(RESONANCE, 5), &recording().0, &no_reply());
    assert_eq!(controls(&frames(&rx)), vec![CC_RESONANCE, CC_ATTACK]);
}

#[test]
fn the_scaling_of_each_link_is_applied_in_turn_along_the_chain() {
    let scale = MapRule { from: [0.0, 127.0], to: [0.0, 1.0] };
    let first = RouteRule { from: CUTOFF.into(), to: RESONANCE.into(), map: Some(scale) };
    let (path, rx) = rig(&[first, link(RESONANCE, ATTACK)]);
    path.dispatch(msg(CUTOFF, 127), &recording().0, &no_reply());
    assert_eq!(
        frames(&rx),
        vec![vec![0xB0, CC_CUTOFF, 127], vec![0xB0, CC_RESONANCE, 1], vec![0xB0, CC_ATTACK, 1]],
        "the second link receives what the first one put out"
    );
}

#[test]
fn a_link_with_two_followers_and_a_chain_behind_one_emits_each_address_once_in_depth_first_order() {
    let (path, rx) = rig(&[
        link(CUTOFF, RESONANCE),
        link(CUTOFF, RELEASE),
        link(RESONANCE, ATTACK),
    ]);
    path.dispatch(msg(CUTOFF, 9), &recording().0, &no_reply());
    assert_eq!(controls(&frames(&rx)), vec![CC_CUTOFF, CC_RESONANCE, CC_ATTACK, CC_RELEASE]);
}

#[test]
fn a_chain_of_four_links_is_followed_to_its_end_without_fault() {
    let (path, rx) = rig(&[
        link(CUTOFF, RESONANCE),
        link(RESONANCE, ATTACK),
        link(ATTACK, RELEASE),
        link(RELEASE, WAVEFORM),
    ]);
    let (send, seen) = recording();
    path.dispatch(msg(CUTOFF, 1), &send, &no_reply());
    let got = frames(&rx);
    assert_eq!(got.len(), 5, "the device's frame and four links: {got:?}");
    assert_eq!(controls(&got), vec![CC_CUTOFF, CC_RESONANCE, CC_ATTACK, CC_RELEASE, CC_WAVEFORM]);
    assert!(faults(&seen).is_empty(), "no link matches at the depth limit: no fault, got {:?}", faults(&seen));
}

#[test]
fn a_fifth_link_is_not_followed_and_the_cut_is_said_once_by_the_link_error_address() {
    let (path, rx) = rig(&[
        link(CUTOFF, RESONANCE),
        link(RESONANCE, ATTACK),
        link(ATTACK, RELEASE),
        link(RELEASE, WAVEFORM),
        link(WAVEFORM, LFO_RATE),
    ]);
    let (send, seen) = recording();
    path.dispatch(msg(CUTOFF, 1), &send, &no_reply());
    let got = frames(&rx);
    assert_eq!(
        controls(&got),
        vec![CC_CUTOFF, CC_RESONANCE, CC_ATTACK, CC_RELEASE, CC_WAVEFORM],
        "four links are followed whole; the fifth delivers nothing"
    );
    let faults = faults(&seen);
    assert_eq!(faults.len(), 1, "one fault, through the emission path: {faults:?}");
    assert!(
        faults[0].contains(CUTOFF) && faults[0].contains(WAVEFORM) && faults[0].contains(LFO_RATE),
        "the fault names the chain followed and the link cut: {}", faults[0]
    );
    assert!(faults[0].to_lowercase().contains("cut"), "the fault says it is a cut: {}", faults[0]);
}

#[test]
fn the_rule_at_adding_and_the_rule_at_running_judge_the_same_chain_of_five_links() {
    // The table already holds five links (the file is not checked at the door):
    // running cuts at the fifth. The same chain, added live one link at a time,
    // is refused at the fifth hop by the rule at adding.
    let chain = [
        (CUTOFF, RESONANCE),
        (RESONANCE, ATTACK),
        (ATTACK, RELEASE),
        (RELEASE, WAVEFORM),
        (WAVEFORM, LFO_RATE),
    ];
    let rules: Vec<RouteRule> = chain.iter().map(|(a, b)| link(a, b)).collect();

    let (path, _rx) = rig(&[]);
    let (send, seen) = recording();
    let mut refused = Vec::new();
    for (from, to) in chain {
        let args = vec![OscType::String(from.into()), OscType::String(to.into())];
        path.dispatch(OscMessage { addr: "/bridge/link/add".into(), args }, &send, &no_reply());
        refused.push(seen.lock().unwrap().last().unwrap().addr.clone());
    }
    assert_eq!(
        refused,
        vec!["/bridge/link/added"; 4].into_iter().chain(["/bridge/link/error"]).collect::<Vec<_>>(),
        "four links accepted, the fifth refused"
    );

    let (path, rx) = rig(&rules);
    let (send, seen) = recording();
    path.dispatch(msg(CUTOFF, 1), &send, &no_reply());
    assert_eq!(frames(&rx).len(), 5, "run: four links followed");
    assert_eq!(faults(&seen).len(), 1, "run: the fifth is cut");
}

#[test]
fn a_loop_of_concrete_links_is_refused_at_adding_and_never_enters_the_table() {
    let (path, rx) = rig(&[]);
    let (send, seen) = recording();
    let add = |from: &str, to: &str| {
        let args = vec![OscType::String(from.into()), OscType::String(to.into())];
        path.dispatch(OscMessage { addr: "/bridge/link/add".into(), args }, &send, &no_reply());
        seen.lock().unwrap().last().unwrap().addr.clone()
    };
    assert_eq!(add(CUTOFF, RESONANCE), "/bridge/link/added");
    assert_eq!(add(RESONANCE, CUTOFF), "/bridge/link/error");
    path.dispatch(msg(CUTOFF, 3), &send, &no_reply());
    assert_eq!(controls(&frames(&rx)), vec![CC_CUTOFF, CC_RESONANCE], "the loop never turns");
}

#[test]
fn a_loop_of_links_with_captures_emits_four_hops_then_is_cut_with_the_fault() {
    let looping = link("/test_synth/filter/{p}", "/test_synth/filter/{p}");
    let (path, rx) = rig(&[looping]);
    let (send, seen) = recording();
    path.dispatch(msg(CUTOFF, 7), &send, &no_reply());
    let got = frames(&rx);
    assert_eq!(got.len(), 5, "the device's frame and at most four hops: {got:?}");
    let faults = faults(&seen);
    assert_eq!(faults.len(), 1, "the loop is cut once, with the fault: {faults:?}");
}

#[test]
fn a_second_message_after_a_cut_starts_again_from_depth_zero() {
    let (path, rx) = rig(&[link(CUTOFF, RESONANCE), link(RESONANCE, ATTACK)]);
    let looping = rig(&[link("/test_synth/filter/{p}", "/test_synth/filter/{p}")]);
    let (send, seen) = recording();
    looping.0.dispatch(msg(CUTOFF, 1), &send, &no_reply());
    assert_eq!(faults(&seen).len(), 1);
    // The counter of this thread went back to zero: a chain is followed whole.
    path.dispatch(msg(CUTOFF, 2), &send, &no_reply());
    assert_eq!(controls(&frames(&rx)), vec![CC_CUTOFF, CC_RESONANCE, CC_ATTACK]);
    assert_eq!(faults(&seen).len(), 1, "no new fault");
}
