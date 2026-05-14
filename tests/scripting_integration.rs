//! End-to-end test of transform + script on a real device JSON.

use osc_bridge::device::Device;
use osc_bridge::frame::{build_frame, Bindings};
use osc_bridge::scripting::{ScriptContext, ScriptEngine};

#[test]
fn loads_example_device() {
    let d = Device::load("devices/examples/scripted-example.json").unwrap();
    assert_eq!(d.device.osc_prefix, "/example");
    let ccs = d.cc_params.as_ref().unwrap();
    let e = &ccs.entries[0];
    assert_eq!(e.osc, "/volume_scaled");
    assert!(e.transform.is_some());
    assert_eq!(d.commands.len(), 1);
    assert!(d.commands[0].script.is_some());
}

#[test]
fn transform_on_cc_param_scales_0_1000_to_0_127() {
    let d = Device::load("devices/examples/scripted-example.json").unwrap();
    let expr = d.cc_params.as_ref().unwrap().entries[0].transform.clone().unwrap();
    let engine = ScriptEngine::new().unwrap();
    assert_eq!(engine.eval_transform(&expr, 0).unwrap(),    0);
    assert_eq!(engine.eval_transform(&expr, 500).unwrap(),  63);
    assert_eq!(engine.eval_transform(&expr, 1000).unwrap(), 127);
}

#[test]
fn script_on_command_appends_xor_checksum() {
    let d = Device::load("devices/examples/scripted-example.json").unwrap();
    let cmd = &d.commands[0];
    let mut b = Bindings::new();
    b.set_u7("a", 0x12);
    b.set_u7("b", 0x34);
    b.set_u7("c", 0x56);
    // Raw frame: F0 7F 00 00 10 12 34 56 F7
    let raw = build_frame(&d, &cmd.frame, &b).unwrap();
    assert_eq!(raw, vec![0xF0, 0x7F, 0x00, 0x00, 0x10, 0x12, 0x34, 0x56, 0xF7]);

    // Run the command's script: XOR bytes from index 5 (opcode) onwards, insert just before F7.
    let engine = ScriptEngine::new().unwrap();
    let ctx = ScriptContext {
        payload: raw.clone(),
        direction: "osc_to_midi".into(),
        device: "example".into(),
        command: cmd.osc.clone(),
        ..Default::default()
    };
    let out = engine.run_script(cmd.script.as_ref().unwrap(), ctx).unwrap().unwrap();
    // Script XORs bytes from Lua index 5 (opcode 0x10) through second-to-last,
    // inclusive: 0x10 ^ 0x12 ^ 0x34 ^ 0x56 = 0x60.
    assert_eq!(out.payload, vec![0xF0, 0x7F, 0x00, 0x00, 0x10, 0x12, 0x34, 0x56, 0x60, 0xF7]);
}
