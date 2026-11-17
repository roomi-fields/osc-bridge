use super::*;
use rosc::OscType;

#[test]
fn interp_substitutes_captures() {
    let mut caps = HashMap::new();
    caps.insert("n".to_string(), "3".to_string());
    assert_eq!(
        interp_to_template("/ableton/track/{n}/volume", &caps),
        "/ableton/track/3/volume"
    );
}

#[test]
fn interp_unknown_placeholder_becomes_empty() {
    let caps = HashMap::new();
    assert_eq!(interp_to_template("/x/{missing}/y", &caps), "/x//y");
}

#[test]
fn interp_no_placeholder_passes_through() {
    let caps = HashMap::new();
    assert_eq!(interp_to_template("/static/path", &caps), "/static/path");
}

#[test]
fn apply_map_linear_endpoints_and_midpoint() {
    let m = MapRule { from: [0.0, 127.0], to: [0.0, 1.0] };
    match apply_map(Some(&OscType::Int(127)), &m).unwrap() {
        OscType::Float(f) => assert!((f - 1.0).abs() < 0.001),
        o => panic!("expected float, got {o:?}"),
    }
    match apply_map(Some(&OscType::Int(0)), &m).unwrap() {
        OscType::Float(f) => assert!(f.abs() < 0.001),
        o => panic!("expected float, got {o:?}"),
    }
    match apply_map(Some(&OscType::Int(64)), &m).unwrap() {
        OscType::Float(f) => assert!((f - 0.504).abs() < 0.01),
        o => panic!("expected float, got {o:?}"),
    }
}

#[test]
fn apply_map_clamps_out_of_range_input() {
    let m = MapRule { from: [0.0, 100.0], to: [0.0, 1.0] };
    match apply_map(Some(&OscType::Int(250)), &m).unwrap() {
        OscType::Float(f) => assert!((f - 1.0).abs() < 0.001),
        o => panic!("expected float, got {o:?}"),
    }
    match apply_map(Some(&OscType::Int(-50)), &m).unwrap() {
        OscType::Float(f) => assert!(f.abs() < 0.001),
        o => panic!("expected float, got {o:?}"),
    }
}

#[test]
fn apply_map_inverted_output_range() {
    // map.to can be descending — e.g. CC → an inverted parameter.
    let m = MapRule { from: [0.0, 127.0], to: [1.0, 0.0] };
    match apply_map(Some(&OscType::Int(0)), &m).unwrap() {
        OscType::Float(f) => assert!((f - 1.0).abs() < 0.001),
        o => panic!("expected float, got {o:?}"),
    }
    match apply_map(Some(&OscType::Int(127)), &m).unwrap() {
        OscType::Float(f) => assert!(f.abs() < 0.001),
        o => panic!("expected float, got {o:?}"),
    }
}

#[test]
fn apply_map_rejects_non_numeric_and_missing() {
    let m = MapRule { from: [0.0, 1.0], to: [0.0, 1.0] };
    assert!(apply_map(Some(&OscType::String("x".into())), &m).is_none());
    assert!(apply_map(None, &m).is_none());
}

#[test]
fn apply_map_zero_input_range_is_rejected() {
    let m = MapRule { from: [5.0, 5.0], to: [0.0, 1.0] };
    assert!(apply_map(Some(&OscType::Int(5)), &m).is_none());
}

// The orchestrator's table of links: held once, changed while it runs, read
// by a message through a shared snapshot.

fn rule(from: &str, to: &str) -> RouteRule {
    RouteRule { from: from.into(), to: to.into(), map: None }
}

#[test]
fn a_snapshot_taken_before_a_change_keeps_the_whole_table_it_saw() {
    let table = LinkTable::new(&[rule("/a", "/b")]);
    let before = table.snapshot();
    table.insert(rule("/c", "/d"));
    assert_eq!(before.len(), 1, "the snapshot in hand is not changed under the reader");
    assert_eq!(table.snapshot().len(), 2, "the next snapshot has the new link");
    assert!(table.remove("/a", "/b"));
    assert_eq!(before.len(), 1);
    assert_eq!(table.snapshot().len(), 1);
}

#[test]
fn a_change_does_not_wait_for_a_reader_holding_a_snapshot() {
    let table = std::sync::Arc::new(LinkTable::new(&[rule("/a", "/b")]));
    let held = table.snapshot();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let t = table.clone();
    std::thread::spawn(move || {
        t.insert(rule("/c", "/d"));
        t.remove("/a", "/b");
        let _ = done_tx.send(());
    });
    // The guard is the generous deadline: a writer blocked by `held` never answers.
    done_rx.recv_timeout(Duration::from_secs(30)).expect("the writer finished while a snapshot was held");
    assert_eq!(held.len(), 1);
    assert_eq!(table.snapshot().len(), 1);
}

#[test]
fn removing_an_absent_link_says_so_and_changes_nothing() {
    let table = LinkTable::new(&[rule("/a", "/b")]);
    assert!(!table.remove("/x", "/y"));
    assert_eq!(table.snapshot().len(), 1);
}
