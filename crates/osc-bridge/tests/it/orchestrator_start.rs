//! The shared start of an orchestrator in the tests: it is ready when its own
//! control forward arrives on the rig's private target, not when anyone answers;
//! it starts again on a fresh port when `Orchestrator::run` ends with an error
//! (3 starts at most), and its failure names the error, the message, the address
//! and the number of sends.

use crate::common::orchestrator_rig::{start_orchestrator, start_orchestrator_picking, MAX_STARTS, RIG_PING};
use std::net::UdpSocket;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// A stranger holding a UDP port, answering every datagram it gets (as
/// anyone listening on `/bridge/status` would).
struct Stranger {
    port: u16,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Stranger {
    fn hold() -> Self {
        let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        sock.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
        let port = sock.local_addr().unwrap().port();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = std::thread::spawn(move || {
            let mut buf = [0u8; 65536];
            while !flag.load(Ordering::Relaxed) {
                if let Ok((_, from)) = sock.recv_from(&mut buf) {
                    let _ = sock.send_to(b"/bridge/status\0\0,\0\0\0", from);
                }
            }
        });
        Self { port, stop, thread: Some(thread) }
    }
}

impl Drop for Stranger {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn empty_config(_port: u16, _dir: &std::path::Path) -> (String, String) {
    (String::new(), String::new())
}

#[test]
fn start_succeeds_on_the_first_try_when_the_port_is_free() {
    let started = start_orchestrator("start-free", empty_config);
    assert_eq!(started.starts, 1);
}

#[test]
fn start_falls_back_to_a_fresh_port_and_reaches_its_own_orchestrator_when_a_stranger_holds_the_chosen_one() {
    let stranger = Stranger::hold();
    let taken = stranger.port;
    let mut picks = 0;
    let started = start_orchestrator_picking("start-taken", || {
        picks += 1;
        if picks == 1 { taken } else { UdpSocket::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port() }
    }, empty_config);
    assert_eq!(started.starts, 2, "the first start fails on the taken port, the second holds");
    assert_ne!(started.orch.port(), taken, "the rig's orchestrator is not the stranger");
}

#[test]
fn start_fails_after_three_starts_naming_the_error_the_message_the_address_and_the_sends() {
    let stranger = Stranger::hold();
    let taken = stranger.port;
    let failure = std::panic::catch_unwind(|| {
        start_orchestrator_picking("start-never", || taken, empty_config);
    }).expect_err("every start finds the port taken");
    let message = failure.downcast_ref::<String>().cloned()
        .or_else(|| failure.downcast_ref::<&str>().map(|s| s.to_string()))
        .expect("a text message");
    let lower = message.to_lowercase();
    assert!(lower.contains("in use") || lower.contains("only one usage"),
        "names the error of the last Orchestrator::run: {message}");
    assert!(message.contains(RIG_PING), "names the message sent: {message}");
    assert!(message.contains(&format!("127.0.0.1:{taken}")), "names the address: {message}");
    assert!(message.contains(&MAX_STARTS.to_string()), "names the number of starts: {message}");
    assert!(message.contains("times"), "names the number of sends: {message}");
}
