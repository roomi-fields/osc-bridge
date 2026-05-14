//! End-to-end Phase 4 integration: the real devices/electra-one/electra-one.json
//! is loaded, its /preset/upload custom_command is fed a realistic Electra
//! preset, and the resulting dynamic routes are verified. The /page/switched
//! reply script is also exercised.

use osc_bridge::device::Device;
use osc_bridge::routing::DynamicRoutes;
use osc_bridge::scripting::{Profile, ScriptContext, ScriptEngine};

/// Realistic Electra preset JSON — abbreviated but uses the real field names:
/// pages / devices / controls with cc7 messages on different channels.
const PRESET: &str = r#"
{
  "version": 2,
  "name": "TestBench",
  "pages": [
    { "id": 1, "name": "Osc" },
    { "id": 2, "name": "Filter" }
  ],
  "devices": [
    { "id": 1, "name": "Synth A", "port": 1, "channel": 1 },
    { "id": 2, "name": "Synth B", "port": 1, "channel": 5 }
  ],
  "controls": [
    { "id": 1, "pageId": 1, "name": "Osc1 Pitch", "type": "fader",
      "values": [ { "message": { "type": "cc7", "parameterNumber": 20, "deviceId": 1, "min": 0, "max": 127 } } ] },
    { "id": 2, "pageId": 1, "name": "Osc2 Pitch", "type": "fader",
      "values": [ { "message": { "type": "cc7", "parameterNumber": 21, "deviceId": 1, "min": 0, "max": 127 } } ] },
    { "id": 3, "pageId": 2, "name": "Cutoff", "type": "fader",
      "values": [ { "message": { "type": "cc7", "parameterNumber": 74, "deviceId": 2, "min": 0, "max": 127 } } ] },
    { "id": 4, "pageId": 2, "name": "Resonance", "type": "fader",
      "values": [ { "message": { "type": "cc7", "parameterNumber": 71, "deviceId": 2, "min": 0, "max": 127 } } ] }
  ]
}
"#;

fn electra() -> Device {
    Device::load("devices/electra-one/electra-one.json").expect("load electra-one.json")
}

#[test]
fn electra_one_json_declares_upload_command() {
    let d = electra();
    let upload = d.custom_commands.iter().find(|c| c.osc == "/preset/upload");
    let cmd = upload.expect("/preset/upload custom_command must exist");
    assert_eq!(cmd.profile.as_deref(), Some("preset_ingest"));
    assert!(cmd.script.contains("ob.json_decode"));
    assert!(cmd.script.contains("register_cc_route"));
}

#[test]
fn upload_script_registers_one_route_per_cc7_control() {
    let d = electra();
    let cmd = d.custom_commands.iter().find(|c| c.osc == "/preset/upload").unwrap();

    let routes = DynamicRoutes::handle();
    let e = ScriptEngine::new_with_routes(routes.clone()).unwrap();
    let ctx = ScriptContext {
        args_str: vec![PRESET.to_string()],
        ..Default::default()
    };
    let out = e.run_script_with(Profile::PresetIngest, &cmd.script, ctx).unwrap();
    assert!(out.is_some(), "script returned ctx, not nil");

    let g = routes.lock().unwrap();
    assert_eq!(g.by_cc.len(), 4, "4 cc7 controls → 4 routes");

    // Spot-check addresses were normalized and page-indexed.
    let r = g.lookup_osc("/electra1/page1/osc1_pitch").expect("osc1 pitch on page1");
    assert_eq!(r.cc, 20);
    assert_eq!(r.channel, 0, "device 1 → channel 1 → 0-indexed");
    assert_eq!(r.page, Some(1));

    let r = g.lookup_osc("/electra1/page2/cutoff").expect("cutoff on page2");
    assert_eq!(r.cc, 74);
    assert_eq!(r.channel, 4, "device 2 channel 5 → 0-indexed 4");
    assert_eq!(r.page, Some(2));
}

#[test]
fn upload_script_is_idempotent() {
    let d = electra();
    let cmd = d.custom_commands.iter().find(|c| c.osc == "/preset/upload").unwrap();

    let routes = DynamicRoutes::handle();
    let e = ScriptEngine::new_with_routes(routes.clone()).unwrap();
    for _ in 0..3 {
        let ctx = ScriptContext { args_str: vec![PRESET.into()], ..Default::default() };
        let _ = e.run_script_with(Profile::PresetIngest, &cmd.script, ctx).unwrap();
    }
    assert_eq!(routes.lock().unwrap().by_cc.len(), 4, "no duplicate accumulation");
}

#[test]
fn page_switched_reply_updates_current_page_via_script() {
    let d = electra();
    let page_reply = d.replies.iter().find(|r| r.emit_osc.starts_with("/page/switched")).unwrap();
    assert!(page_reply.script.is_some(), "page/switched must have a reply script");

    let routes = DynamicRoutes::handle();
    let e = ScriptEngine::new_with_routes(routes.clone()).unwrap();

    // Simulate the bindings the match_reply path would produce.
    let mut bindings = std::collections::HashMap::new();
    bindings.insert("page".into(), 3);
    let ctx = ScriptContext {
        bindings,
        direction: "midi_to_osc".into(),
        command: page_reply.emit_osc.clone(),
        ..Default::default()
    };
    let _ = e.run_script(page_reply.script.as_ref().unwrap(), ctx).unwrap();
    assert_eq!(routes.lock().unwrap().current_page, Some(3));
}

#[test]
fn upload_script_emits_sysex_blocks_for_hardware_push() {
    let d = electra();
    let cmd = d.custom_commands.iter().find(|c| c.osc == "/preset/upload").unwrap();

    let routes = DynamicRoutes::handle();
    let e = ScriptEngine::new_with_routes(routes).unwrap();
    let ctx = ScriptContext {
        args: vec![2, 5], // bank=2, slot=5
        args_str: vec![PRESET.into()],
        ..Default::default()
    };
    let _ = e.run_script_with(Profile::PresetIngest, &cmd.script, ctx).unwrap();

    let emits = e.drain_emits();
    // Per Electra MK2 firmware 4.x docs, preset upload is a single SysEx:
    // F0 00 21 45 01 01 <json> F7 — no separate arm step.
    assert_eq!(emits.len(), 1, "upload is one SysEx block per Electra doc");
    assert_eq!(&emits[0].bytes[0..2], &[0x01, 0x01], "opcode=upload, resource=Preset");
    assert_eq!(&emits[0].bytes[2..], PRESET.as_bytes(), "JSON ASCII payload");
    assert!(emits[0].wrap, "wrap=true so runtime adds F0 00 21 45 ... F7");
}

#[test]
fn routes_list_introspection_returns_entry_per_route() {
    let d = electra();
    let upload = d.custom_commands.iter().find(|c| c.osc == "/preset/upload").unwrap();
    let list = d.custom_commands.iter().find(|c| c.osc == "/routes/list")
        .expect("/routes/list introspection endpoint must exist");

    let routes = DynamicRoutes::handle();
    let e = ScriptEngine::new_with_routes(routes).unwrap();

    // Seed with the 4-control preset.
    let _ = e.run_script_with(
        Profile::PresetIngest,
        &upload.script,
        ScriptContext { args_str: vec![PRESET.into()], ..Default::default() },
    ).unwrap();
    let _ = e.drain_osc_emits(); // discard any upload-time emits

    // Query routes.
    let _ = e.run_script(&list.script, ScriptContext::default()).unwrap();
    let osc = e.drain_osc_emits();

    // 4 entries + 1 done marker.
    assert_eq!(osc.len(), 5, "4 routes + done sentinel");
    let entries: Vec<_> = osc.iter().filter(|m| m.addr == "/electra1/routes/entry").collect();
    let done: Vec<_> = osc.iter().filter(|m| m.addr == "/electra1/routes/done").collect();
    assert_eq!(entries.len(), 4);
    assert_eq!(done.len(), 1);

    // Each entry carries (osc_addr, channel, cc, page, min, max) — 6 args.
    for e in &entries {
        assert_eq!(e.args.len(), 6, "entry must carry 6 args");
    }
}

#[test]
fn preset_current_reports_last_uploaded_name() {
    let d = electra();
    let upload = d.custom_commands.iter().find(|c| c.osc == "/preset/upload").unwrap();
    let current = d.custom_commands.iter().find(|c| c.osc == "/preset/current").unwrap();

    let routes = DynamicRoutes::handle();
    let e = ScriptEngine::new_with_routes(routes).unwrap();

    let _ = e.run_script_with(
        Profile::PresetIngest,
        &upload.script,
        ScriptContext { args_str: vec![PRESET.into()], ..Default::default() },
    ).unwrap();
    let _ = e.drain_osc_emits();

    let _ = e.run_script(&current.script, ScriptContext::default()).unwrap();
    let osc = e.drain_osc_emits();
    assert_eq!(osc.len(), 1);
    assert_eq!(osc[0].addr, "/electra1/preset/current");
    // First arg is the preset name — our PRESET constant declares "TestBench".
    match &osc[0].args[0] {
        osc_bridge::scripting::EmitOscArg::Str(s) => assert_eq!(s, "TestBench"),
        other => panic!("expected Str, got {other:?}"),
    }
}

#[test]
fn page_current_reports_active_page() {
    let d = electra();
    let pc = d.custom_commands.iter().find(|c| c.osc == "/page/current").unwrap();

    let routes = DynamicRoutes::handle();
    routes.lock().unwrap().current_page = Some(4);
    let e = ScriptEngine::new_with_routes(routes).unwrap();

    let _ = e.run_script(&pc.script, ScriptContext::default()).unwrap();
    let osc = e.drain_osc_emits();
    assert_eq!(osc.len(), 1);
    assert_eq!(osc[0].addr, "/electra1/page/current");
    match &osc[0].args[0] {
        osc_bridge::scripting::EmitOscArg::Int(i) => assert_eq!(*i, 4),
        other => panic!("expected Int, got {other:?}"),
    }
}

#[test]
fn companion_markdown_is_autoloaded_into_device_docs() {
    let d = electra();
    let docs = d.docs.as_ref().expect("electra-one.md must be auto-loaded");
    assert!(docs.contains("Electra One MK2"));
    assert!(docs.contains("/electra1/preset/upload"));
    assert!(docs.contains("routes/list"));
}

#[test]
fn bridge_docs_emits_markdown_reply_for_documented_devices() {
    use osc_bridge::runtime::bridge_docs_replies;
    let d = electra();
    let replies = bridge_docs_replies(std::slice::from_ref(&d));
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0].addr, "/bridge/docs/device");
    // First arg: slug. Second arg: markdown content.
    match (&replies[0].args[0], &replies[0].args[1]) {
        (rosc::OscType::String(slug), rosc::OscType::String(md)) => {
            assert_eq!(slug, "electra1");
            assert!(md.contains("Electra One MK2"));
        }
        _ => panic!("expected two String args, got {:?}", replies[0].args),
    }
}

#[test]
fn bridge_docs_skips_devices_without_companion_markdown() {
    use osc_bridge::runtime::bridge_docs_replies;
    // Load any device that has NO .md next to it. The scripted-example has
    // no companion file.
    let d = osc_bridge::device::Device::load("devices/examples/scripted-example.json").unwrap();
    assert!(d.docs.is_none(), "scripted-example.json must not have a .md");
    let replies = bridge_docs_replies(std::slice::from_ref(&d));
    assert!(replies.is_empty(), "undocumented devices produce no reply");
}

#[test]
fn upload_then_page_switch_resolves_correct_cc_route() {
    // Integration: upload a preset, then simulate a page switch, verify a
    // CC coming in on that page resolves to the right semantic address.
    let d = electra();
    let upload = d.custom_commands.iter().find(|c| c.osc == "/preset/upload").unwrap();
    let page_reply = d.replies.iter().find(|r| r.emit_osc.starts_with("/page/switched")).unwrap();

    let routes = DynamicRoutes::handle();
    let e = ScriptEngine::new_with_routes(routes.clone()).unwrap();

    // 1. Upload.
    let _ = e.run_script_with(
        Profile::PresetIngest,
        &upload.script,
        ScriptContext { args_str: vec![PRESET.into()], ..Default::default() },
    ).unwrap();

    // 2. Device reports page 2 is now active.
    let mut b = std::collections::HashMap::new();
    b.insert("page".into(), 2);
    let _ = e.run_script(
        page_reply.script.as_ref().unwrap(),
        ScriptContext { bindings: b, ..Default::default() },
    ).unwrap();

    // 3. Incoming CC#74 on channel 5 (device 2) must resolve to /electra1/page2/cutoff.
    let g = routes.lock().unwrap();
    let r = g.lookup_cc(4, 74).expect("page 2 active → cutoff route resolves");
    assert_eq!(r.osc_addr, "/electra1/page2/cutoff");
}
