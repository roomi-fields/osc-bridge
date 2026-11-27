//! A control of a 0.7 driver reaches its messages by ways: the engine sends the value by the
//! first way written that it can send (a CC, an NRPN, an RPN, or the SysEx of a parameter),
//! whose `recognize` is not `none` and which holds in every context (no `in`, no `slice`); the
//! value is bounded to the reference scale of the control, then scaled to the way. Without such a
//! way nothing is sent and the sender gets `/bridge/fault <address> <reason>`. No socket, no
//! MIDI port.

use std::sync::{Arc, Mutex};

use crate::common::{files, temp};
use crossbeam_channel::bounded;
use osc_bridge::runtime::{dispatch_to_device, DeviceDispatch, ReplyTo, SendOsc};
use rosc::{OscMessage, OscType};
use serde_json::{json, Value};

/// The driver of `document`, loaded from a catalogue folder as the engine loads it.
fn loaded(document: Value) -> osc_bridge::Device {
    let root = temp("ways");
    files(root.path(), &[("acme/box.json", &document.to_string())]);
    osc_bridge::device::load(root.path().join("acme/box.json")).unwrap_or_else(|e| panic!("{e:#}\n{document}"))
}

/// A 0.7 driver at `/box`, whose `cc_params` entries are `entries`, in a device with two modes.
fn controls(entries: Value) -> osc_bridge::Device {
    loaded(json!({
        "$schema": "https://w3id.org/osd/0.7/device.schema.json",
        "device": { "name": "Box", "vendor": "Acme", "osc_prefix": "/box" },
        "contexts": { "mode": { "values": [{ "id": "single" }, { "id": "multi" }] } },
        "cc_params": { "channel": 0, "entries": entries }
    }))
}

/// A 0.5 driver at `/box` whose `cc_params` entries are `entries`.
fn controls05(entries: Value) -> osc_bridge::Device {
    loaded(json!({
        "$schema": crate::common::D05,
        "device": { "name": "Box", "vendor": "Acme", "osc_prefix": "/box" },
        "cc_params": { "channel": 0, "entries": entries }
    }))
}

/// What one call queued, and the `(address, reason)` of each fault its sender got.
fn call(device: &osc_bridge::Device, address: &str, args: &[i32]) -> (Vec<Vec<u8>>, Vec<(String, String)>) {
    let (tx, rx) = bounded::<Vec<u8>>(64);
    let state = DeviceDispatch::build(device.clone(), tx);
    let faults: Arc<Mutex<Vec<(String, String)>>> = Default::default();
    let sink = faults.clone();
    let reply: ReplyTo = Arc::new(move |_, m| {
        if let (true, [OscType::String(a), OscType::String(r)]) = (m.addr == "/bridge/fault", m.args.as_slice()) {
            sink.lock().unwrap().push((a.clone(), r.clone()));
        }
    });
    let send: SendOsc = Arc::new(|_| {});
    let args = args.iter().map(|v| OscType::Int(*v)).collect();
    dispatch_to_device(&state, OscMessage { addr: address.into(), args }, &send, &reply, "127.0.0.1:9999".parse().unwrap());
    let frames = rx.try_iter().collect();
    let faults = faults.lock().unwrap().clone();
    (frames, faults)
}

/// The frames of a call that is answered with no fault.
fn sent(device: &osc_bridge::Device, address: &str, args: &[i32]) -> Vec<Vec<u8>> {
    let (frames, faults) = call(device, address, args);
    assert!(faults.is_empty(), "{address} {args:?}: {faults:?}");
    frames
}

/// The reason of the one fault of a call that queues nothing, answered at its address.
fn refused(device: &osc_bridge::Device, address: &str, args: &[i32]) -> String {
    let (frames, faults) = call(device, address, args);
    assert!(frames.is_empty(), "{address} {args:?} sends nothing: {frames:?}");
    assert_eq!(faults.len(), 1, "{address} {args:?}: {faults:?}");
    assert_eq!(faults[0].0, address, "{faults:?}");
    faults[0].1.clone()
}

fn cc(numbers: Value) -> Value {
    json!({ "ctrlType": "cc", "ctrlIndex": numbers })
}

fn nrpn(msb: u8, lsb: u8, bits: u8, range: [i64; 2]) -> Value {
    json!({ "ctrlType": "nrpn", "ctrlIndex": [msb, lsb], "value_bits": bits, "range": range })
}

// ---- the way chosen ---------------------------------------------------------

#[test]
fn the_first_way_written_is_the_one_sent() {
    let d = controls(json!([
        { "osc": "/cc_first", "ways": [cc(json!([74])), nrpn(1, 2, 7, [0, 127])] },
        { "osc": "/nrpn_first", "ways": [nrpn(1, 3, 7, [0, 127]), cc(json!([75]))] }
    ]));
    assert_eq!(sent(&d, "/box/cc_first", &[9]), [vec![0xB0, 74, 9]]);
    assert_eq!(sent(&d, "/box/nrpn_first", &[9]), [vec![0xB0, 99, 1], vec![0xB0, 98, 3], vec![0xB0, 6, 9]]);
}

#[test]
fn a_0_5_control_that_mixes_a_cc_and_an_nrpn_sends_the_cc_once_converted() {
    let d = controls05(json!([{ "osc": "/wave", "cc": 9, "cc_lsb": 41, "nrpn_msb": 3, "nrpn_lsb": 96, "range": [0, 16383] }]));
    assert_eq!(sent(&d, "/box/wave", &[8192]), [vec![0xB0, 9, 64], vec![0xB0, 41, 0]]);
}

#[test]
fn a_way_the_device_does_not_recognise_is_passed_over_whatever_it_transmits() {
    let mut deaf = cc(json!([13]));
    deaf["recognize"] = json!("none");
    deaf["transmit"] = json!("absolute");
    let mut mute = cc(json!([14]));
    mute["transmit"] = json!("none");
    let d = controls(json!([{ "osc": "/a", "ways": [deaf, mute] }]));
    assert_eq!(sent(&d, "/box/a", &[5]), [vec![0xB0, 14, 5]]);
}

#[test]
fn a_way_that_holds_in_a_context_or_a_slice_is_passed_over() {
    let mut in_mode = cc(json!([11]));
    in_mode["in"] = json!({ "mode": "single" });
    let mut sliced = cc(json!([12]));
    sliced["slice"] = json!([0, 63]);
    let d = controls(json!([{ "osc": "/a", "ways": [in_mode, sliced, cc(json!([20]))] }]));
    assert_eq!(sent(&d, "/box/a", &[5]), [vec![0xB0, 20, 5]]);
}

#[test]
fn a_way_of_a_kind_the_engine_cannot_send_is_passed_over() {
    let d = controls(json!([{ "osc": "/a", "scale": [0, 127], "ways": [
        { "ctrlType": "pBend" }, { "ctrlType": "chPress" }, { "ctrlType": "pPress", "ctrlIndex": [3] },
        { "ctrlType": "note", "ctrlIndex": [4] }, nrpn(1, 2, 7, [0, 127]) ] }]));
    assert_eq!(sent(&d, "/box/a", &[5]), [vec![0xB0, 99, 1], vec![0xB0, 98, 2], vec![0xB0, 6, 5]]);
}

// ---- the value, bounded to the reference scale then scaled to the way -------

#[test]
fn a_value_is_bounded_to_the_scale_of_the_control_then_scaled_to_the_way_sent() {
    let mut way = cc(json!([10]));
    way["range"] = json!([0, 127]);
    let d = controls(json!([{ "osc": "/pan", "scale": [-64, 63], "ways": [way] }]));
    for (value, byte) in [(-64, 0), (0, 64), (63, 127), (-100, 0), (100, 127)] {
        assert_eq!(sent(&d, "/box/pan", &[value]), [vec![0xB0, 10, byte]], "{value}");
    }
}

#[test]
fn without_a_written_scale_the_most_precise_way_gives_it_and_the_half_rounds_up() {
    let mut coarse = cc(json!([7]));
    coarse["range"] = json!([0, 127]);
    let mut small = cc(json!([8]));
    small["range"] = json!([0, 100]);
    let d = controls(json!([
        { "osc": "/level", "ways": [coarse, nrpn(1, 2, 14, [0, 16383])] },
        { "osc": "/half", "ways": [small, nrpn(1, 3, 14, [0, 200])] }
    ]));
    for (value, byte) in [(0, 0), (8192, 64), (16383, 127), (20000, 127), (-1, 0)] {
        assert_eq!(sent(&d, "/box/level", &[value]), [vec![0xB0, 7, byte]], "{value}");
    }
    assert_eq!(sent(&d, "/box/half", &[1]), [vec![0xB0, 8, 1]], "0.5 goes up");
    assert_eq!(sent(&d, "/box/half", &[200]), [vec![0xB0, 8, 100]]);
}

// ---- no way to send ---------------------------------------------------------

#[test]
fn a_control_without_a_way_loads_sends_nothing_and_answers_a_fault_at_its_address() {
    let d = controls(json!([{ "osc": "/partial", "ways": [] }, { "osc": "/ok", "ways": [cc(json!([1]))] }]));
    refused(&d, "/box/partial", &[5]);
    assert_eq!(sent(&d, "/box/ok", &[5]), [vec![0xB0, 1, 5]], "the rest of the driver is driven");
}

#[test]
fn a_control_whose_ways_the_engine_cannot_send_answers_a_fault_at_its_address() {
    let mut deaf = cc(json!([13]));
    deaf["recognize"] = json!("none");
    let d = controls(json!([{ "osc": "/a", "ways": [{ "ctrlType": "pBend" }, deaf] }]));
    refused(&d, "/box/a", &[5]);
}

#[test]
fn a_control_whose_ways_hold_only_in_a_context_or_a_slice_names_them_in_its_fault() {
    let mut in_mode = cc(json!([11]));
    in_mode["in"] = json!({ "mode": "single" });
    let mut sliced = cc(json!([12]));
    sliced["slice"] = json!([0, 63]);
    let d = controls(json!([{ "osc": "/in", "ways": [in_mode] }, { "osc": "/slice", "ways": [sliced] }]));
    let reason = refused(&d, "/box/in", &[5]);
    assert!(reason.contains("`in`"), "{reason}");
    let reason = refused(&d, "/box/slice", &[5]);
    assert!(reason.contains("slice"), "{reason}");
}

// ---- the data bytes of a way ------------------------------------------------

#[test]
fn a_way_sends_two_data_bytes_only_when_its_maximum_is_above_127() {
    let two_cc = |range: [i64; 2]| json!({ "ctrlType": "cc", "ctrlIndex": [9, 41], "value_bits": 14, "range": range });
    let rpn = |range: [i64; 2]| json!({ "ctrlType": "rpn", "ctrlIndex": [0, 1], "value_bits": 14, "range": range });
    let d = controls(json!([
        { "osc": "/cc/wide", "ways": [two_cc([0, 16383])] },
        { "osc": "/nrpn/wide", "ways": [nrpn(3, 75, 14, [0, 128])] },
        { "osc": "/nrpn/narrow", "ways": [nrpn(3, 76, 14, [0, 8])] },
        { "osc": "/nrpn/seven", "ways": [nrpn(3, 77, 7, [0, 127])] },
        { "osc": "/rpn/wide", "ways": [rpn([0, 16383])] }
    ]));
    assert_eq!(sent(&d, "/box/cc/wide", &[300]), [vec![0xB0, 9, 2], vec![0xB0, 41, 44]]);
    assert_eq!(sent(&d, "/box/nrpn/wide", &[128]), [vec![0xB0, 99, 3], vec![0xB0, 98, 75], vec![0xB0, 6, 1], vec![0xB0, 38, 0]]);
    assert_eq!(sent(&d, "/box/nrpn/narrow", &[5]), [vec![0xB0, 99, 3], vec![0xB0, 98, 76], vec![0xB0, 6, 5]]);
    assert_eq!(sent(&d, "/box/nrpn/seven", &[100]), [vec![0xB0, 99, 3], vec![0xB0, 98, 77], vec![0xB0, 6, 100]]);
    assert_eq!(sent(&d, "/box/rpn/wide", &[300]), [vec![0xB0, 101, 0], vec![0xB0, 100, 1], vec![0xB0, 6, 2], vec![0xB0, 38, 44]]);
}

#[test]
fn narrow_ways_of_two_cc_numbers_and_of_an_rpn_send_the_one_data_byte() {
    let d = controls(json!([
        { "osc": "/cc", "ways": [{ "ctrlType": "cc", "ctrlIndex": [9, 41], "value_bits": 14, "range": [0, 127] }] },
        { "osc": "/rpn", "ways": [{ "ctrlType": "rpn", "ctrlIndex": [0, 0], "value_bits": 14, "range": [0, 24] }] }
    ]));
    assert_eq!(sent(&d, "/box/cc", &[100]), [vec![0xB0, 9, 100]]);
    assert_eq!(sent(&d, "/box/rpn", &[12]), [vec![0xB0, 101, 0], vec![0xB0, 100, 0], vec![0xB0, 6, 12]]);
}

#[test]
fn an_nrpn_way_with_a_block_takes_its_lsb_from_the_call_bounded_to_the_block_then_the_channel() {
    let mut way = nrpn(20, 40, 7, [0, 127]);
    way["nrpn_lsb_range"] = json!([12, 108]);
    let d = controls(json!([{ "osc": "/drum", "ways": [way] }]));
    assert_eq!(sent(&d, "/box/drum", &[90, 36, 9]), [vec![0xB9, 99, 20], vec![0xB9, 98, 36], vec![0xB9, 6, 90]]);
    assert_eq!(sent(&d, "/box/drum", &[90, 5])[1], vec![0xB0, 98, 12]);
    assert_eq!(sent(&d, "/box/drum", &[90, 120])[1], vec![0xB0, 98, 108]);
    refused(&d, "/box/drum", &[90]);
}

#[test]
fn a_0_5_block_converted_with_both_numbers_takes_its_lsb_from_the_call_and_one_without_an_lsb_has_no_way() {
    let d = controls05(json!([
        { "osc": "/both", "nrpn_msb": 21, "nrpn_lsb": 40, "nrpn_lsb_range": [12, 108], "range": [0, 127] },
        { "osc": "/msb_only", "nrpn_msb": 20, "nrpn_lsb_range": [12, 108], "range": [0, 127] }
    ]));
    assert_eq!(sent(&d, "/box/both", &[90, 50])[1], vec![0xB0, 98, 50]);
    refused(&d, "/box/msb_only", &[90, 36]);
}

// ---- the numbers of a SysEx parameter ---------------------------------------

/// A driver whose parameter `/p` has one SysEx way of `numbers`, its frames naming the
/// arguments `names` between `F0 7D 02` and the value.
fn parameter(names: &[&str], numbers: Value) -> osc_bridge::Device {
    let args: Vec<Value> = names.iter().map(|n| json!({ "arg": n })).collect();
    let frame = |head: u8, tail: Vec<Value>| -> Vec<Value> {
        [vec![json!(240), json!(125), json!(head)], args.clone(), tail, vec![json!(247)]].concat()
    };
    loaded(json!({
        "$schema": "https://w3id.org/osd/0.7/device.schema.json",
        "device": { "name": "Box", "vendor": "Acme", "osc_prefix": "/box" },
        "params": {
            "value_type": "u7",
            "get_frame": frame(1, vec![]),
            "set_frame": frame(2, vec![json!({ "arg": "val" })]),
            "entries": [{ "osc": "/p", "ways": [{ "ctrlType": "sysex", "ctrlIndex": numbers }] }]
        }
    }))
}

#[test]
fn as_many_numbers_as_the_frames_name_arguments_fill_them_in_order() {
    let d = parameter(&["p", "c", "r"], json!([5, 6, 7]));
    assert_eq!(sent(&d, "/box/p", &[9]), [vec![0xF0, 0x7D, 2, 5, 6, 7, 9, 0xF7]]);
    let d = parameter(&["p", "c"], json!([3, 4]));
    assert_eq!(sent(&d, "/box/p", &[9]), [vec![0xF0, 0x7D, 2, 3, 4, 9, 0xF7]]);
    let d = parameter(&["pr", "p", "c", "r"], json!([1, 2, 3, 4]));
    assert_eq!(sent(&d, "/box/p", &[9]), [vec![0xF0, 0x7D, 2, 1, 2, 3, 4, 9, 0xF7]]);
}

#[test]
fn otherwise_the_numbers_fill_pr_p_c_r_in_order_and_the_absent_ones_are_zero() {
    let d = parameter(&["pr", "p", "c", "r"], json!([5, 6]));
    assert_eq!(sent(&d, "/box/p", &[9]), [vec![0xF0, 0x7D, 2, 5, 6, 0, 0, 9, 0xF7]]);
    let d = parameter(&["p", "c"], json!([3]));
    assert_eq!(sent(&d, "/box/p", &[9]), [vec![0xF0, 0x7D, 2, 0, 0, 9, 0xF7]], "3 is pr, which no frame names");
}

#[test]
fn a_number_alone_also_fills_the_id_of_the_frames() {
    let d = parameter(&["id"], json!([42]));
    assert_eq!(sent(&d, "/box/p", &[9]), [vec![0xF0, 0x7D, 2, 42, 9, 0xF7]]);
    let d = parameter(&["pr", "p", "id"], json!([42]));
    assert_eq!(sent(&d, "/box/p", &[9]), [vec![0xF0, 0x7D, 2, 42, 0, 42, 9, 0xF7]]);
}

#[test]
fn a_0_5_parameter_that_gives_only_p_c_and_r_emits_the_same_bytes_once_converted() {
    let d = loaded(json!({
        "$schema": crate::common::D05,
        "device": { "name": "Box", "vendor": "Acme", "osc_prefix": "/box" },
        "params": {
            "value_type": "u7",
            "get_frame": [240, 1, { "arg": "p" }, { "arg": "c" }, { "arg": "r" }, 247],
            "set_frame": [240, 2, { "arg": "p" }, { "arg": "c" }, { "arg": "r" }, { "arg": "val" }, 247],
            "entries": [{ "osc": "/p", "p": 5, "c": 6, "r": 7, "range": [0, 100] }]
        }
    }));
    assert_eq!(sent(&d, "/box/p", &[120]), [vec![0xF0, 2, 5, 6, 7, 100, 0xF7]]);
}

#[test]
fn a_parameter_without_a_way_the_engine_can_send_answers_a_fault_at_its_address() {
    let set = |ways: Value| loaded(json!({
        "$schema": "https://w3id.org/osd/0.7/device.schema.json",
        "device": { "name": "Box", "vendor": "Acme", "osc_prefix": "/box" },
        "params": { "value_type": "u7", "get_frame": [240, 1, { "arg": "p" }, 247],
            "set_frame": [240, 2, { "arg": "p" }, { "arg": "val" }, 247],
            "entries": [{ "osc": "/p", "ways": ways }] }
    }));
    refused(&set(json!([])), "/box/p", &[9]);
    refused(&set(json!([{ "ctrlType": "sysex", "ctrlIndex": [3], "recognize": "none", "transmit": "absolute" }])), "/box/p", &[9]);
}

// ---- parameterMap.get after conversion --------------------------------------

#[test]
fn parameter_map_get_of_a_sysex_number_reads_the_command_whose_id_is_that_number_value_sent_included() {
    let probe = |name: &str, number: i64| json!({ "osc": name, "frame": [240, 247],
        "script": format!("local ctx = ...\nctx.payload = {{240, parameterMap.get(1, PT_SYSEX, {number}), 247}}\nreturn ctx") });
    let level = |name: &str, extra: Value, default: i64| {
        let mut c = json!({ "osc": name, "frame": [], "args": [{ "name": "v", "type": "u7", "range": [0, 127], "default": default }] });
        for (k, v) in extra.as_object().unwrap() { c[k] = v.clone(); }
        c
    };
    let earlier = loaded(json!({
        "$schema": crate::common::D05,
        "device": { "name": "Box", "vendor": "Acme", "osc_prefix": "/box" },
        "capabilities": ["lua"],
        "commands": [
            level("/level", json!({ "_electra": { "parameter": { "type": "sysex", "number": 12 } } }), 100),
            probe("/probe", 12)
        ]
    }));
    let current = loaded(json!({
        "$schema": "https://w3id.org/osd/0.7/device.schema.json",
        "device": { "name": "Box", "vendor": "Acme", "osc_prefix": "/box" },
        "capabilities": ["lua"],
        "commands": [level("/level", json!({ "id": 12 }), 100), probe("/probe", 12)]
    }));
    for d in [earlier, current] {
        let (tx, rx) = bounded::<Vec<u8>>(8);
        let state = DeviceDispatch::build(d, tx);
        let send: SendOsc = Arc::new(|_| {});
        let say = |address: &str, args: Vec<OscType>| {
            dispatch_to_device(&state, OscMessage { addr: address.into(), args }, &send, &crate::common::no_reply(), "127.0.0.1:9999".parse().unwrap());
            rx.try_iter().collect::<Vec<_>>()
        };
        assert_eq!(say("/box/probe", vec![]), [vec![0xF0, 100, 0xF7]], "the default before anything is sent");
        say("/box/level", vec![OscType::Int(55)]);
        assert_eq!(say("/box/probe", vec![]), [vec![0xF0, 55, 0xF7]], "then the value sent");
    }
}
