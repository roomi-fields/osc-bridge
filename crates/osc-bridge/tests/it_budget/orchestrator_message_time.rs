//! The time the orchestrator adds to one message, measured at rest by the night.
//!
//! The charter bounds the delay the core adds to 1 ms per message, MIDI excluded.
//! The measure runs the orchestrator's message path (`BenchPath`, the same path
//! the UDP and WebSocket loops call) on a loaded MIDI device that has no port:
//! the start is the message entering the path, the end is the frame reaching the
//! device's emission queue. It holds the owner lookup, the link applied and the
//! frame built; it leaves out the OSC wire encoding, the UDP socket, the MIDI
//! driver and the port. Each pass checks that the message reached the queue.

use crossbeam_channel::{bounded, Receiver};
use osc_bridge::orchestrator::{BenchPath, RouteRule};
use osc_bridge::runtime::SendOsc;
use rosc::{OscMessage, OscType};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The charter's bound: the delay the core adds per message, excluding MIDI.
const BOUND: Duration = Duration::from_millis(1);
/// Passes played before the measure starts.
const WARM_UP_PASSES: usize = 1_000;
/// Passes measured.
const PASSES: usize = 20_000;

const SOURCE: &str = "/test_synth/filter/cutoff";
const TARGET: &str = "/test_synth/filter/resonance";

fn rig(links: &[RouteRule]) -> (BenchPath, Receiver<Vec<u8>>) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/devices/examples/test_synth.json");
    let device = osc_bridge::device::load_with_docs(path).unwrap();
    let (tx, rx) = bounded::<Vec<u8>>(1024);
    (BenchPath::new(vec![(device, tx)], links), rx)
}

fn message() -> OscMessage {
    OscMessage { addr: SOURCE.into(), args: vec![OscType::Int(64)] }
}

/// Play the warm-up then `PASSES` messages; return the median and the worst
/// time, and fail a pass whose message did not reach the queue
/// (`expected_frames` frames).
fn measure(path: &BenchPath, rx: &Receiver<Vec<u8>>, expected_frames: usize) -> (Duration, Duration) {
    let send: SendOsc = Arc::new(|_| {});
    let mut times = Vec::with_capacity(PASSES);
    for pass in 0..WARM_UP_PASSES + PASSES {
        let msg = message();
        let start = Instant::now();
        path.dispatch(msg, &send);
        let elapsed = start.elapsed();
        let frames: Vec<Vec<u8>> = rx.try_iter().collect();
        assert_eq!(frames.len(), expected_frames, "pass {pass}: the message did not reach the emission queue");
        if pass >= WARM_UP_PASSES {
            times.push(elapsed);
        }
    }
    times.sort();
    (times[times.len() / 2], *times.last().unwrap())
}

fn assert_under_bound(what: &str, (median, worst): (Duration, Duration)) {
    eprintln!("{what}: median {median:?}, worst {worst:?}");
    assert!(median <= BOUND, "{what}: median {median:?} over {BOUND:?}");
    assert!(worst <= BOUND, "{what}: worst case {worst:?} over {BOUND:?} (replay at rest if the machine was loaded)");
}

#[test]
fn message_that_follows_a_link_adds_under_the_bound() {
    let link = RouteRule { from: SOURCE.into(), to: TARGET.into(), map: None };
    let (path, rx) = rig(&[link]);
    // The device's frame, then the linked one.
    assert_under_bound("message following a link", measure(&path, &rx, 2));
}

#[test]
fn message_that_follows_no_link_adds_under_the_bound() {
    let (path, rx) = rig(&[]);
    assert_under_bound("message following no link", measure(&path, &rx, 1));
}
