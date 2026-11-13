//! Phase 3: custom_commands end-to-end. Exercises the Lua → DynamicRoutes
//! path via a ScriptContext populated as the runtime would.

use osc_bridge::routing::DynamicRoutes;
use osc_bridge::scripting::{Profile, ScriptContext, ScriptEngine};

const MINI_PRESET: &str = r#"
{
  "name": "TestPreset",
  "pages": [
    { "id": 1, "name": "page1" }
  ],
  "controls": [
    { "id": 1, "pageId": 1, "name": "Cutoff", "type": "fader",
      "values": [ { "message": { "type": "cc7", "parameterNumber": 74, "deviceId": 1, "min": 0, "max": 127 } } ] },
    { "id": 2, "pageId": 1, "name": "Resonance", "type": "fader",
      "values": [ { "message": { "type": "cc7", "parameterNumber": 71, "deviceId": 1, "min": 0, "max": 127 } } ] }
  ]
}
"#;

/// The kind of script a real Electra custom_command would run.
const UPLOAD_SCRIPT: &str = r#"
ctx = ...
local json = ctx.args_str[1]
local preset = ob.json_decode(json)
ob.clear_routes()
-- Build page-id → page-index lookup.
local page_idx = {}
for i, p in ipairs(preset.pages) do page_idx[p.id] = i end
for _, c in ipairs(preset.controls) do
  local m = c.values[1].message
  if m.type == "cc7" then
    local page = page_idx[c.pageId]
    local addr = string.format("/electra1/p%d/%s", page, string.lower(c.name))
    ob.register_cc_route(page, 0, m.parameterNumber, addr, m.min, m.max)
  end
end
return ctx
"#;

#[test]
fn upload_script_registers_one_route_per_control() {
    let routes = DynamicRoutes::handle();
    let e = ScriptEngine::new_with_routes(routes.clone()).unwrap();
    let ctx = ScriptContext {
        args_str: vec![MINI_PRESET.to_string()],
        direction: "osc_to_midi".into(),
        command: "/preset/upload".into(),
        ..Default::default()
    };
    let out = e.run_script_with(Profile::PresetIngest, UPLOAD_SCRIPT, ctx).unwrap();
    assert!(out.is_some(), "script returned ctx");

    let g = routes.lock().unwrap();
    assert_eq!(g.by_cc.len(), 2, "two routes registered");
    assert_eq!(g.lookup_osc("/electra1/p1/cutoff").unwrap().cc, 74);
    assert_eq!(g.lookup_osc("/electra1/p1/resonance").unwrap().cc, 71);
    // Page 1 → cutoff resolves; page 2 → falls back to agnostic (none here).
    drop(g);
    routes.lock().unwrap().current_page = Some(1);
    assert_eq!(
        routes.lock().unwrap().lookup_cc(0, 74).unwrap().osc_addr,
        "/electra1/p1/cutoff"
    );
}

#[test]
fn upload_script_is_idempotent_via_clear_routes() {
    let routes = DynamicRoutes::handle();
    let e = ScriptEngine::new_with_routes(routes.clone()).unwrap();
    for _ in 0..3 {
        let ctx = ScriptContext {
            args_str: vec![MINI_PRESET.to_string()],
            ..Default::default()
        };
        let _ = e.run_script_with(Profile::PresetIngest, UPLOAD_SCRIPT, ctx).unwrap();
    }
    assert_eq!(routes.lock().unwrap().by_cc.len(), 2, "no duplicate buildup");
}

#[test]
fn script_can_emit_raw_sysex_via_payload() {
    // Script that doesn't touch routes but emits a chunk of bytes. Mirrors
    // the pattern of "arm_upload then send JSON as sysex data block".
    let e = ScriptEngine::new().unwrap();
    let ctx = ScriptContext {
        args_str: vec!["hello".into()],
        ..Default::default()
    };
    let out = e.run_script(
        r#"ctx = ...
           local s = ctx.args_str[1]
           for i = 1, #s do ctx.payload[i] = string.byte(s, i) end
           return ctx"#,
        ctx,
    ).unwrap().unwrap();
    assert_eq!(out.payload, b"hello");
}
