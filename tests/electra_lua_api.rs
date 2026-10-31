//! OB-d0b: the engine offers Electra One's `parameterMap.get` and the device
//! channel (`devices.get(id):getChannel()`) to a driver's declared Lua, so a
//! preset's functions run unchanged. Electra's documentation:
//! https://docs.electra.one/developers/luaext.html
//!
//! The parameter number is Electra's own: a command carries it under the
//! annotation key `_electra` (`{"parameter": {"type": "sysex", "number": 1}}`).
//! `ScriptEngine::new_for_device` gives the engine the driver;
//! `ScriptEngine::record_value(osc, value)` stores the last value sent or
//! received for the command at that OSC address.
//!
//! The Lua functions below are taken unchanged from the Electra presets
//! (kawai/k1 MuteS1, roland/jv_1080 patch editor, korg/03r_w, roland/d_550).

use osc_bridge::device::Device;
use osc_bridge::scripting::{ScriptContext, ScriptEngine};

/// A driver whose commands carry `_electra.parameter`.
fn driver(midi_out_channel: u8) -> Device {
    let json = format!(
        r#"{{
  "device": {{ "name": "Acme", "vendor": "Acme", "revision": "A1", "osc_prefix": "/acme" }},
  "midi_out": {{ "default_channel": {midi_out_channel} }},
  "commands": [
    {{ "osc": "/level_s1", "_electra": {{ "parameter": {{ "type": "sysex", "number": 1 }} }},
       "args": [ {{ "name": "v", "type": "int", "range": [0, 127], "default": 100 }} ], "frame": [] }},
    {{ "osc": "/level_s2", "_electra": {{ "parameter": {{ "type": "sysex", "number": 2 }} }},
       "args": [ {{ "name": "v", "type": "int", "range": [5, 127] }} ], "frame": [] }},
    {{ "osc": "/level_s3", "_electra": {{ "parameter": {{ "type": "sysex", "number": 3 }} }},
       "args": [ {{ "name": "v", "type": "int" }} ], "frame": [] }},
    {{ "osc": "/virt1", "_electra": {{ "parameter": {{ "type": "virtual", "number": 1 }} }},
       "args": [ {{ "name": "v", "type": "int", "range": [0, 127], "default": 9 }} ], "frame": [] }},
    {{ "osc": "/dev_id", "_electra": {{ "parameter": {{ "type": "virtual", "number": 405 }} }},
       "args": [ {{ "name": "v", "type": "int", "range": [0, 31] }} ], "frame": [] }},
    {{ "osc": "/tone", "_electra": {{ "parameter": {{ "type": "virtual", "number": 31 }} }},
       "args": [ {{ "name": "v", "type": "int", "range": [0, 9] }} ], "frame": [] }},
    {{ "osc": "/tempo", "_electra": {{ "parameter": {{ "type": "sysex", "number": 100 }} }},
       "args": [ {{ "name": "v", "type": "int", "range": [0, 255] }} ], "frame": [] }},
    {{ "osc": "/plain", "args": [ {{ "name": "v", "type": "int", "default": 77 }} ], "frame": [] }}
  ]
}}"#
    );
    serde_json::from_str(&json).expect("driver parses")
}

/// Run `body` (Lua) in the engine; `body` fills `ctx.payload` with bytes.
fn run(e: &ScriptEngine, body: &str) -> Vec<u8> {
    let code = format!("ctx = ...\n{body}\nreturn ctx");
    e.run_script(&code, ScriptContext::default()).expect("script runs").expect("ctx returned").payload
}

fn engine(channel: u8) -> ScriptEngine {
    ScriptEngine::new_for_device(&driver(channel)).expect("engine for the driver")
}

const K1_MUTE: &str = r#"
function MuteS1 (valueObject, value)
if (value == 0) then
local level = parameterMap.get(1, PT_SYSEX, 1)
print("level: " .. level)
return (level)
else
return (0)
end
end
function MuteS2 (valueObject, value)
if (value == 0) then
local level = parameterMap.get(1, PT_SYSEX, 2)
print("level: " .. level)
return (level)
else
return (0)
end
end
function MuteS3 (valueObject, value)
if (value == 0) then
local level = parameterMap.get(1, PT_SYSEX, 3)
print("level: " .. level)
return (level)
else
return (0)
end
end
"#;

const JV1080: &str = r#"
GlobDeviceId = 1
PMAP_DEVICE_ID = 405
PMAP_DEFAULT_TEMPO = 100
PMAP_SELECTED_TONE = 31
function getDevId()
    return parameterMap.get (GlobDeviceId, PT_VIRTUAL,PMAP_DEVICE_ID)
end
function getDefaultTempoHigh()
    local pmValue= parameterMap.get (GlobDeviceId, PT_SYSEX,PMAP_DEFAULT_TEMPO)
    local defaultTempoLow = (pmValue)%16
    local defaultTempoHigh = (pmValue-defaultTempoLow)/16
    return defaultTempoHigh
end
function getDefaultTempoLow()
    local pmValue= parameterMap.get (GlobDeviceId, PT_SYSEX,PMAP_DEFAULT_TEMPO)
    local defaultTempoLow = (pmValue)%16
    return defaultTempoLow
end
function getToneAddress()
    local selectedTone = parameterMap.get (GlobDeviceId, PT_VIRTUAL, PMAP_SELECTED_TONE)
    return selectedTone * 2 + 0x10
end
function getToneAddressPlusOne()
    local selectedTone = parameterMap.get (GlobDeviceId, PT_VIRTUAL, PMAP_SELECTED_TONE)
    return selectedTone * 2 + 0x11
end
"#;

// ---- criterion 1: parameterMap.get ---------------------------------------

#[test]
fn c1_parameter_map_get_answers_the_last_value_held_for_the_command_numbered_so() {
    let e = engine(0);
    e.record_value("/level_s1", 64);
    e.record_value("/level_s2", 33);
    let out = run(&e, "ctx.payload = { parameterMap.get(1, PT_SYSEX, 1), parameterMap.get(1, PT_SYSEX, 2) }");
    assert_eq!(out, vec![64, 33]);
}

#[test]
fn c1_a_newer_value_replaces_the_older_one() {
    let e = engine(0);
    e.record_value("/level_s1", 10);
    e.record_value("/level_s1", 90);
    assert_eq!(run(&e, "ctx.payload = { parameterMap.get(1, PT_SYSEX, 1) }"), vec![90]);
}

#[test]
fn c1_without_a_value_it_answers_the_commands_default() {
    let e = engine(0);
    assert_eq!(run(&e, "ctx.payload = { parameterMap.get(1, PT_SYSEX, 1) }"), vec![100]);
}

#[test]
fn c1_without_value_or_default_it_answers_the_low_end_of_the_range() {
    let e = engine(0);
    assert_eq!(run(&e, "ctx.payload = { parameterMap.get(1, PT_SYSEX, 2) }"), vec![5]);
}

#[test]
fn c1_without_value_default_or_range_it_answers_zero() {
    let e = engine(0);
    assert_eq!(run(&e, "ctx.payload = { parameterMap.get(1, PT_SYSEX, 3) }"), vec![0]);
}

#[test]
fn c1_a_number_no_command_carries_reads_zero() {
    let e = engine(0);
    assert_eq!(run(&e, "ctx.payload = { parameterMap.get(1, PT_SYSEX, 999) }"), vec![0]);
}

#[test]
fn c1_the_type_is_part_of_the_address() {
    // sysex 1 and virtual 1 are two parameters.
    let e = engine(0);
    e.record_value("/virt1", 5);
    e.record_value("/level_s1", 60);
    let out = run(&e, "ctx.payload = { parameterMap.get(1, PT_SYSEX, 1), parameterMap.get(1, PT_VIRTUAL, 1) }");
    assert_eq!(out, vec![60, 5]);
}

#[test]
fn c1_a_virtual_parameter_with_no_value_and_no_default_reads_zero() {
    let e = engine(0);
    assert_eq!(run(&e, "ctx.payload = { parameterMap.get(1, PT_VIRTUAL, 405) }"), vec![0]);
}

#[test]
fn c1_a_command_without_the_annotation_is_not_addressable() {
    // `/plain` has a default (77) but no `_electra`: nothing answers for it.
    let e = engine(0);
    e.record_value("/plain", 12);
    let out = run(&e, "ctx.payload = { parameterMap.get(1, PT_SYSEX, 0), parameterMap.get(1, PT_VIRTUAL, 0) }");
    assert_eq!(out, vec![0, 0]);
}

#[test]
fn c1_the_electra_parameter_types_exist_as_constants() {
    let e = engine(0);
    let out = run(
        &e,
        "local n = 0\nfor _, t in ipairs({PT_SYSEX, PT_VIRTUAL, PT_CC7, PT_CC14, PT_NRPN, PT_RPN, PT_NOTE, PT_PROGRAM}) do\n\
         if t ~= nil then n = n + 1 end end\nctx.payload = { n }",
    );
    assert_eq!(out, vec![8]);
}

#[test]
fn c1_print_goes_to_the_log_and_does_not_fail_the_script() {
    let e = engine(0);
    assert_eq!(run(&e, "print('level: ' .. 3)\nctx.payload = { 1 }"), vec![1]);
}

#[test]
fn c1_the_k1_mute_functions_run_unchanged() {
    let e = engine(0);
    e.record_value("/level_s1", 80);
    e.record_value("/level_s2", 45);
    // /level_s3 holds none: its range-less command reads 0.
    let body = format!(
        "{K1_MUTE}\nctx.payload = {{ MuteS1(nil, 0), MuteS1(nil, 1), MuteS2(nil, 0), MuteS2(nil, 127), MuteS3(nil, 0) }}"
    );
    assert_eq!(run(&e, &body), vec![80, 0, 45, 0, 0]);
}

// ---- criterion 2: the device channel -------------------------------------

#[test]
fn c2_get_channel_gives_the_channel_the_engine_sends_on_from_one_to_sixteen() {
    for (zero_based, expected) in [(0u8, 1u8), (4, 5), (15, 16)] {
        let e = engine(zero_based);
        let out = run(&e, "ctx.payload = { devices.get(1):getChannel() }");
        assert_eq!(out, vec![expected], "midi_out.default_channel {zero_based}");
    }
}

#[test]
fn c2_a_cc_params_driver_gives_its_table_channel() {
    let json = r#"{ "device": { "name": "Acme", "vendor": "Acme", "revision": "A1", "osc_prefix": "/acme" },
                    "cc_params": { "channel": 2, "entries": [] } }"#;
    let d: Device = serde_json::from_str(json).unwrap();
    let e = ScriptEngine::new_for_device(&d).unwrap();
    assert_eq!(run(&e, "ctx.payload = { devices.get(1):getChannel() }"), vec![3]);
}

#[test]
fn c2_the_preset_global_channel_is_set_from_the_device_as_the_presets_do() {
    let e = engine(2);
    let body = "channel = devices.get(1):getChannel()\nctx.payload = { channel }";
    assert_eq!(run(&e, body), vec![3]);
}

#[test]
fn c2_get_channel_byte_of_the_d_550_and_the_03r_w_run_unchanged() {
    let e = engine(9); // channel 10
    let body = r#"
function getChannelByteD550(device)
  return (0x00 + (channel - 1))
end
function getChannelByte(device)
  return (0x30 + (channel - 1))
end
local device = devices.get(1)
channel = device:getChannel()
ctx.payload = { getChannelByteD550(device), getChannelByte(device) }
"#;
    assert_eq!(run(&e, body), vec![0x09, 0x39]);
}

// ---- criterion 3: the JV-1080 functions -----------------------------------

#[test]
fn c3_jv_1080_device_id_reads_the_virtual_parameter() {
    let e = engine(0);
    e.record_value("/dev_id", 17);
    let body = format!("{JV1080}\nctx.payload = {{ getDevId() }}");
    assert_eq!(run(&e, &body), vec![17]);
}

#[test]
fn c3_jv_1080_default_tempo_splits_the_held_sysex_value_into_high_and_low() {
    let e = engine(0);
    e.record_value("/tempo", 0x53);
    let body = format!("{JV1080}\nctx.payload = {{ getDefaultTempoHigh(), getDefaultTempoLow() }}");
    assert_eq!(run(&e, &body), vec![0x05, 0x03]);
    e.record_value("/tempo", 0xA7);
    assert_eq!(run(&e, &body), vec![0x0A, 0x07]);
}

#[test]
fn c3_jv_1080_tone_addresses_follow_the_selected_tone() {
    let e = engine(0);
    let body = format!("{JV1080}\nctx.payload = {{ getToneAddress(), getToneAddressPlusOne() }}");
    // No tone selected: reads 0, so addresses 0x10 and 0x11.
    assert_eq!(run(&e, &body), vec![0x10, 0x11]);
    e.record_value("/tone", 3);
    assert_eq!(run(&e, &body), vec![0x16, 0x17]);
    e.record_value("/tone", 9);
    assert_eq!(run(&e, &body), vec![0x22, 0x23]);
}
