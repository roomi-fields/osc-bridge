use super::*;

const SCHEMA: &str = "https://w3id.org/osd/0.5/device.schema.json";

/// A 0.5 driver from the keys after `device` (leading comma included).
fn device(body: &str) -> osd::Device {
    let text = format!(r#"{{"$schema":"{SCHEMA}","device":{{"name":"n","vendor":"v","osc_prefix":"/n"}}{body}}}"#);
    match osd::read(&text, osd::Kind::Device).unwrap_or_else(|e| panic!("{e}\n{text}")) {
        osd::Descriptor::Device(d) => *d,
        _ => unreachable!(),
    }
}

/// The `forward` of the one command `/c` of a driver.
fn forward(forward: &str) -> osd::Forward {
    device(&format!(r#","commands":[{{"osc":"/c","forward":{forward}}}]"#)).commands.unwrap().remove(0).forward.unwrap()
}

/// The one entry of a `cc_params` table.
fn entry(json: &str) -> osd::CcEntry {
    device(&format!(r#","cc_params":{{"entries":[{json}]}}"#)).cc_params.unwrap().entries.remove(0)
}

// ---- control messages ----

#[test]
fn rpn_sends_101_100_then_data_entry() {
    let e = entry(r#"{"osc":"/pb_sens","rpn_msb":0,"rpn_lsb":0,"range":[0,24]}"#);
    assert_eq!(cc_messages(&e, 2, 12, &[OscType::Int(12)]).unwrap(),
        vec![vec![0xB2, 101, 0], vec![0xB2, 100, 0], vec![0xB2, 6, 12]]);
}

#[test]
fn nrpn_lsb_comes_from_the_call_then_the_channel() {
    let e = entry(r#"{"osc":"/drum/cutoff","nrpn_msb":20,"nrpn_lsb_range":[12,108],"range":[0,127]}"#);
    assert_eq!(cc_messages(&e, 0, 90, &[OscType::Int(90), OscType::Int(36), OscType::Int(9)]).unwrap(),
        vec![vec![0xB9, 99, 20], vec![0xB9, 98, 36], vec![0xB9, 6, 90]]);
    assert_eq!(cc_messages(&e, 0, 90, &[OscType::Int(90), OscType::Int(5)]).unwrap()[1], vec![0xB0, 98, 12]);
    assert!(cc_messages(&e, 0, 90, &[OscType::Int(90)]).is_err());
}

#[test]
fn fourteen_bit_nrpn_and_cc_pairs_split_the_value() {
    let n = entry(r#"{"osc":"/x","nrpn_msb":3,"nrpn_lsb":75,"range":[0,16383]}"#);
    assert_eq!(cc_messages(&n, 0, 300, &[]).unwrap().last().unwrap(), &vec![0xB0, 38, 44]);
    let c = entry(r#"{"osc":"/y","cc":9,"cc_lsb":41,"range":[0,16383]}"#);
    assert_eq!(cc_messages(&c, 0, 300, &[]).unwrap(), vec![vec![0xB0, 9, 2], vec![0xB0, 41, 44]]);
}

// ---- expressions and paths ----

#[test]
fn eval_simple_add() {
    let mut b = Bindings::new(); b.set_u7("pad", 10);
    assert_eq!(eval_expr("pad + 4", &b).unwrap(), 14);
}
#[test]
fn eval_ternary() {
    let mut b = Bindings::new(); b.set_u7("pad", 10);
    assert_eq!(eval_expr("pad > 7 ? 12 : 0", &b).unwrap(), 12);
    b.set_u7("pad", 3);
    assert_eq!(eval_expr("pad > 7 ? 12 : 0", &b).unwrap(), 0);
}
#[test]
fn path_match_with_capture() {
    let parts = parse_osc_tpl("/minilab3/pad/{pad}/color");
    let caps = match_osc_path(&parts, "/minilab3/pad/3/color").unwrap();
    assert_eq!(caps.get("pad").unwrap(), "3");
}

#[test]
fn an_exact_address_is_matched_before_a_capture() {
    let dev = device(r#","commands":[{"osc":["/param/",{"capture":"id"}]},{"osc":"/param/cutoff"}]"#);
    let cmds = build_command_map(&dev);
    let first = cmds.iter().find(|h| match_osc_path(&h.pattern_parts, "/n/param/cutoff").is_some()).unwrap();
    assert_eq!(first.cmd.osc, osd::Address::Text("/param/cutoff".into()));
    assert!(cmds.iter().any(|h| match_osc_path(&h.pattern_parts, "/n/param/level").is_some()));
}

// ---- OSC transport (software drivers) ----

#[test]
fn forward_msg_literal_int_arg() {
    let f = forward(r#"{"path":"/live/song/start_playing","args":[120]}"#);
    let m = build_forward_msg(&f, &HashMap::new()).unwrap();
    assert_eq!(m.addr, "/live/song/start_playing");
    assert!(matches!(m.args[0], OscType::Int(120)));
}

#[test]
fn forward_msg_placeholder_args_resolved_from_bindings() {
    let f = forward(r#"{"path":"/live/track/set/volume","args":[{"arg":"n"},{"arg":"v"}]}"#);
    let mut b = HashMap::new();
    b.insert("n".into(), OscType::Int(3));
    b.insert("v".into(), OscType::Float(0.75));
    let m = build_forward_msg(&f, &b).unwrap();
    assert!(matches!(m.args[0], OscType::Int(3)));
    assert!(matches!(m.args[1], OscType::Float(_)));
}

#[test]
fn forward_msg_path_with_placeholder() {
    let f = forward(r#"{"path":["/track/",{"capture":"n"},"/state"],"args":[]}"#);
    let mut b = HashMap::new();
    b.insert("n".into(), OscType::Int(5));
    let m = build_forward_msg(&f, &b).unwrap();
    assert_eq!(m.addr, "/track/5/state");
}

#[test]
fn forward_msg_literal_string_pass_through() {
    let f = forward(r#"{"path":"/x","args":["literal"]}"#);
    let m = build_forward_msg(&f, &HashMap::new()).unwrap();
    assert!(matches!(&m.args[0], OscType::String(s) if s == "literal"));
}

#[test]
fn coerce_str_to_int_and_bool() {
    assert!(matches!(coerce_str_to_osc("42", osd::ArgType::Int), Some(OscType::Int(42))));
    assert!(matches!(coerce_str_to_osc("true", osd::ArgType::Bool), Some(OscType::Bool(true))));
    assert!(matches!(coerce_str_to_osc("0", osd::ArgType::Bool), Some(OscType::Bool(false))));
}

#[test]
fn osc_type_match_accepts_long_for_int_and_double_for_float() {
    assert!(osc_type_matches(&OscType::Int(1), osd::ArgType::Int));
    assert!(osc_type_matches(&OscType::Long(1), osd::ArgType::Int));
    assert!(osc_type_matches(&OscType::Float(0.5), osd::ArgType::Float));
    assert!(osc_type_matches(&OscType::Double(0.5), osd::ArgType::Float));
    assert!(!osc_type_matches(&OscType::Int(1), osd::ArgType::String));
}

#[test]
fn full_osc_driver_json_reads_through_osd() {
    let dev = device(r#","commands":[
            { "osc": "/transport/play", "forward": { "path": "/live/song/start_playing", "args": [] } },
            { "osc": "/transport/tempo", "args": [{"name":"bpm","type":"float"}],
              "forward": { "path": "/live/song/set/tempo", "args": [{"arg":"bpm"}] } } ],
        "subscriptions": [ { "on": "startup", "forward": { "path": "/live/song/start_listen/tempo", "args": [] } } ],
        "replies": [ { "match_osc": "/live/song/get/tempo", "match_args": [{"name":"bpm","type":"float"}],
            "emit_osc": "/transport/tempo", "emit_args": [{"capture":"bpm"}] } ]"#);
    let commands = dev.commands.as_ref().unwrap();
    assert_eq!(commands.len(), 2);
    assert!(commands[0].forward.is_some());
    assert!(commands[0].frame.is_none());
    assert_eq!(dev.subscriptions.as_ref().map(Vec::len), Some(1));
    assert_eq!(dev.replies.as_ref().unwrap()[0].match_args.as_ref().map(Vec::len), Some(1));
}

#[test]
fn emit_osc_reply_substitutes_bindings_and_prefixes() {
    let mut b = HashMap::new();
    b.insert("bpm".into(), OscType::Float(124.5));
    let reply = device(r#","replies":[{"match_osc":"/live/song/get/tempo","match_args":[{"name":"bpm","type":"float"}],
        "emit_osc":"/transport/tempo","emit_args":[{"capture":"bpm"}]}]"#).replies.unwrap().remove(0);
    let m = emit_osc_reply_typed(&reply.emit_osc, reply.emit_args.as_deref(), "/ableton", &b);
    assert_eq!(m.addr, "/ableton/transport/tempo");
    assert_eq!(m.args.len(), 1);
    match &m.args[0] {
        OscType::Float(f) => assert!((*f - 124.5).abs() < 0.01),
        other => panic!("wrong arg type: {other:?}"),
    }
}

#[test]
fn passthrough_strips_prefix_when_empty_target() {
    // device.osc_prefix = "/sonicpi", passthrough_prefix = ""
    // /sonicpi/cue/foo → /cue/foo
    let addr = "/sonicpi/cue/foo";
    let osc_prefix = "/sonicpi";
    let pt = "";
    let rest = addr.strip_prefix(osc_prefix).unwrap();
    let new_addr = if pt.is_empty() { rest.to_string() } else { format!("{pt}{rest}") };
    assert_eq!(new_addr, "/cue/foo");
}

#[test]
fn passthrough_replaces_prefix_when_set() {
    // device.osc_prefix = "/sc", passthrough_prefix = "/notify"
    // /sc/sync → /notify/sync
    let addr = "/sc/sync";
    let osc_prefix = "/sc";
    let pt = "/notify";
    let rest = addr.strip_prefix(osc_prefix).unwrap();
    let new_addr = if pt.is_empty() { rest.to_string() } else { format!("{pt}{rest}") };
    assert_eq!(new_addr, "/notify/sync");
}

#[test]
fn build_bindings_combines_path_caps_and_positional_args() {
    let cmd = device(r#","commands":[{"osc":["/track/",{"capture":"n"},"/volume"],
        "args":[{"name":"n","type":"int"},{"name":"v","type":"float"}]}]"#).commands.unwrap().remove(0);
    let mut caps = HashMap::new();
    caps.insert("n".into(), "7".to_string());
    let b = build_forward_bindings(&cmd, &caps, &[OscType::Float(0.5)]);
    assert!(matches!(b.get("n").unwrap(), OscType::Int(7)));
    assert!(matches!(b.get("v").unwrap(), OscType::Float(_)));
}
