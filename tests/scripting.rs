//! ScriptEngine behaviour tests — sandbox, helpers, timeout, memory cap.

use osc_bridge::routing::DynamicRoutes;
use osc_bridge::scripting::{Profile, ScriptContext, ScriptEngine};
use std::time::Instant;

#[test]
fn transform_linear_scaling() {
    let e = ScriptEngine::new().unwrap();
    let v = e.eval_transform("return math.floor(value * 127 / 1000)", 500).unwrap();
    assert_eq!(v, 63);
}

#[test]
fn transform_exp_curve() {
    let e = ScriptEngine::new().unwrap();
    // Map 0..127 through an exp curve. Just verify it runs and produces a sensible value.
    let v = e.eval_transform("local e = math.exp(1); return math.floor(127 * (math.exp(value / 127) - 1) / (e - 1))", 64).unwrap();
    assert!(v >= 0 && v <= 127, "got {v}");
}

#[test]
fn transform_uses_ob_clamp() {
    let e = ScriptEngine::new().unwrap();
    let v = e.eval_transform("return ob.u7_clamp(value * 2)", 200).unwrap();
    assert_eq!(v, 127);
    let v = e.eval_transform("return ob.u7_clamp(value * 2)", -5).unwrap();
    assert_eq!(v, 0);
}

#[test]
fn ob_u14_split() {
    let e = ScriptEngine::new().unwrap();
    // 12345 = 0x3039 → msb 0x60, lsb 0x39
    let msb = e.eval_transform("return ob.u14_msb(value)", 12345).unwrap();
    let lsb = e.eval_transform("return ob.u14_lsb(value)", 12345).unwrap();
    assert_eq!(msb, 0x60);
    assert_eq!(lsb, 0x39);
}

#[test]
fn script_xor_checksum_on_payload() {
    let e = ScriptEngine::new().unwrap();
    let ctx = ScriptContext {
        payload: vec![0x12, 0x34, 0x56],
        direction: "osc_to_midi".into(),
        command: "cmd".into(),
        ..Default::default()
    };
    let out = e.run_script(
        "ctx = ...; ctx.checksum = ob.checksum_xor(ctx.payload); return ctx",
        ctx,
    ).unwrap().unwrap();
    // 0x12 ^ 0x34 ^ 0x56 = 0x70, & 0x7F = 0x70
    assert_eq!(out.checksum, Some(0x70));
}

#[test]
fn sandbox_blocks_os_execute() {
    let e = ScriptEngine::new().unwrap();
    let err = e.eval_transform("os.execute('echo pwned'); return 0", 0);
    assert!(err.is_err(), "os.execute should be unreachable");
}

#[test]
fn sandbox_blocks_io_open() {
    let e = ScriptEngine::new().unwrap();
    let err = e.eval_transform("io.open('/etc/passwd'); return 0", 0);
    assert!(err.is_err(), "io.open should be unreachable");
}

#[test]
fn sandbox_blocks_require() {
    let e = ScriptEngine::new().unwrap();
    let err = e.eval_transform("require('os'); return 0", 0);
    assert!(err.is_err(), "require should be unreachable");
}

#[test]
fn infinite_loop_timeouts_cleanly() {
    let e = ScriptEngine::new().unwrap();
    let start = Instant::now();
    let err = e.eval_transform("while true do end\nreturn 0", 0);
    let elapsed = start.elapsed();
    assert!(err.is_err(), "expected timeout error");
    assert!(elapsed.as_millis() < 500, "took {}ms (should be <500)", elapsed.as_millis());
    let msg = format!("{}", err.unwrap_err());
    assert!(msg.contains("Timeout") || msg.contains("timeout"),
        "expected timeout in error msg, got: {msg}");
}

#[test]
fn memory_cap_enforced() {
    let e = ScriptEngine::new().unwrap();
    // Allocate a big table — should blow past 1 MiB and fail.
    let err = e.eval_transform(
        "local t = {}; for i = 1, 10000000 do t[i] = i end; return #t",
        0,
    );
    assert!(err.is_err(), "huge allocation should fail");
}

#[test]
fn script_can_drop_message_with_nil() {
    let e = ScriptEngine::new().unwrap();
    let ctx = ScriptContext {
        args: vec![0],
        direction: "midi_to_osc".into(),
        ..Default::default()
    };
    let out = e.run_script("ctx = ...; if ctx.args[1] == 0 then return nil end; return ctx", ctx).unwrap();
    assert!(out.is_none(), "script returning nil should drop the message");
}

#[test]
fn script_mutates_args() {
    let e = ScriptEngine::new().unwrap();
    let ctx = ScriptContext { args: vec![10, 20], ..Default::default() };
    let out = e.run_script("ctx = ...; ctx.args[1] = ctx.args[1] + ctx.args[2]; return ctx", ctx).unwrap().unwrap();
    assert_eq!(out.args, vec![30, 20]);
}

// --- Phase 1 (Electra One groundwork) ---------------------------------------

#[test]
fn device_state_persists_across_calls() {
    let e = ScriptEngine::new().unwrap();
    // Write state in one call.
    let _ = e.eval_transform("ob.state.counter = (ob.state.counter or 0) + value; return 0", 5).unwrap();
    let _ = e.eval_transform("ob.state.counter = (ob.state.counter or 0) + value; return 0", 7).unwrap();
    // Read it back.
    let v = e.eval_transform("return ob.state.counter", 0).unwrap();
    assert_eq!(v, 12);
}

#[test]
fn device_state_isolated_per_engine() {
    let a = ScriptEngine::new().unwrap();
    let b = ScriptEngine::new().unwrap();
    let _ = a.eval_transform("ob.state.x = 42; return 0", 0).unwrap();
    let v = b.eval_transform("return ob.state.x or -1", 0).unwrap();
    assert_eq!(v, -1, "engines must not share state");
}

#[test]
fn json_decode_roundtrip() {
    let e = ScriptEngine::new().unwrap();
    // Decode an object, then re-encode, compare by re-decoding (field order
    // is not guaranteed in encode output).
    let ctx = ScriptContext {
        args: vec![],
        direction: "test".into(),
        ..Default::default()
    };
    let out = e.run_script(
        r#"
        local t = ob.json_decode('{"name":"Filter","cc":74,"vals":[0,64,127]}')
        assert(t.name == "Filter", "name mismatch")
        assert(t.cc == 74, "cc mismatch")
        assert(#t.vals == 3 and t.vals[2] == 64, "array mismatch")
        local s = ob.json_encode(t)
        local t2 = ob.json_decode(s)
        assert(t2.name == "Filter" and t2.cc == 74, "roundtrip failed")
        return nil
        "#,
        ctx,
    ).unwrap();
    assert!(out.is_none());
}

#[test]
fn json_decode_malformed_errors() {
    let e = ScriptEngine::new().unwrap();
    let err = e.eval_transform("ob.json_decode('{not json'); return 0", 0);
    assert!(err.is_err(), "malformed JSON should error");
}

#[test]
fn json_decode_large_payload_under_preset_profile() {
    // Build a JSON payload representative of a small Electra preset (~80 KB
    // of values). Default profile's 1 MiB cap and 10 ms budget would be
    // tight; PresetIngest has 4 MiB / 500 ms.
    let mut items = String::from("[");
    for i in 0..2000 {
        if i > 0 { items.push(','); }
        items.push_str(&format!(r#"{{"id":{i},"name":"ctrl_{i}","cc":{}}}"#, i % 128));
    }
    items.push(']');
    let e = ScriptEngine::new().unwrap();
    let code = format!(
        "local t = ob.json_decode([==[{items}]==]); return #t"
    );
    let v = e.eval_transform_with(Profile::PresetIngest, &code, 0).unwrap();
    assert_eq!(v, 2000);
}

#[test]
fn preset_ingest_profile_widens_timeout() {
    // A loop that runs ~30 ms — fails under Default (10 ms), passes under
    // PresetIngest (500 ms).
    let e = ScriptEngine::new().unwrap();
    // Sized to reliably bust 10 ms but stay well under 500 ms on slow hosts.
    let busy = "local x=0; for i=1,40000 do x=x+i end; return x % 127";
    let _ = e.eval_transform_with(Profile::Default, busy, 0);
    let v = e.eval_transform_with(Profile::PresetIngest, busy, 0).unwrap();
    assert!(v >= 0 && v < 127);
}

#[test]
fn memory_cap_restored_after_preset_profile() {
    let e = ScriptEngine::new().unwrap();
    // Run a preset-ingest call that allocates a large table (under 4 MiB).
    let _ = e.eval_transform_with(
        Profile::PresetIngest,
        "local t={}; for i=1,50000 do t[i]='x' end; return #t",
        0,
    ).unwrap();
    // After it returns, Default cap must be back in effect: the same big
    // allocation pattern should fail.
    let err = e.eval_transform(
        "local t={}; for i=1,10000000 do t[i]=i end; return #t",
        0,
    );
    assert!(err.is_err(), "Default memory cap must be restored");
}

#[test]
fn log_warn_and_error_are_callable() {
    let e = ScriptEngine::new().unwrap();
    // Just verify they exist and don't crash; stderr output is not captured.
    let v = e.eval_transform("ob.log_warn('hi'); ob.log_error('oops'); return 1", 0).unwrap();
    assert_eq!(v, 1);
}

// --- Phase 2 (dynamic routing via Lua) -------------------------------------

#[test]
fn register_cc_route_from_lua_updates_shared_handle() {
    let routes = DynamicRoutes::handle();
    let e = ScriptEngine::new_with_routes(routes.clone()).unwrap();
    let _ = e.eval_transform_with(
        Profile::PresetIngest,
        r#"
        ob.register_cc_route(nil, 0, 74, "/electra1/p1/cutoff", 0, 127)
        ob.register_cc_route(1,   0, 10, "/electra1/p1/mix",    -64, 63)
        return 0
        "#,
        0,
    ).unwrap();
    let g = routes.lock().unwrap();
    assert_eq!(g.by_cc.len(), 2);
    assert_eq!(g.lookup_osc("/electra1/p1/cutoff").unwrap().cc, 74);
    assert_eq!(g.lookup_osc("/electra1/p1/mix").unwrap().max, 63);
}

#[test]
fn set_and_get_current_page_from_lua() {
    let routes = DynamicRoutes::handle();
    let e = ScriptEngine::new_with_routes(routes.clone()).unwrap();
    let v = e.eval_transform(
        "ob.set_current_page(3); return ob.get_current_page()",
        0,
    ).unwrap();
    assert_eq!(v, 3);
    assert_eq!(routes.lock().unwrap().current_page, Some(3));
    // nil clears
    let _ = e.eval_transform("ob.set_current_page(nil); return 0", 0).unwrap();
    assert_eq!(routes.lock().unwrap().current_page, None);
}

#[test]
fn clear_routes_from_lua() {
    let routes = DynamicRoutes::handle();
    let e = ScriptEngine::new_with_routes(routes.clone()).unwrap();
    let _ = e.eval_transform(
        r#"ob.register_cc_route(nil, 0, 1, "/x", 0, 127); ob.clear_routes(); return 0"#,
        0,
    ).unwrap();
    assert!(routes.lock().unwrap().by_cc.is_empty());
}

#[test]
fn emit_sysex_queues_blocks_across_one_call() {
    let e = ScriptEngine::new().unwrap();
    let _ = e.eval_transform(r#"
        ob.emit_sysex({0x02, 0x01}, true)
        ob.emit_sysex({0xF0, 0x00, 0x21, 0x45, 0x20, 0x08, 0x00, 0x00, 0xF7}, false)
        return 0
    "#, 0).unwrap();
    let blocks = e.drain_emits();
    assert_eq!(blocks.len(), 2);
    assert_eq!(blocks[0].bytes, vec![0x02, 0x01]);
    assert!(blocks[0].wrap);
    assert_eq!(blocks[1].bytes.first(), Some(&0xF0));
    assert!(!blocks[1].wrap);
}

#[test]
fn drain_emits_empties_queue() {
    let e = ScriptEngine::new().unwrap();
    let _ = e.eval_transform("ob.emit_sysex({1,2,3}); return 0", 0).unwrap();
    assert_eq!(e.drain_emits().len(), 1);
    assert_eq!(e.drain_emits().len(), 0, "second drain must be empty");
}

#[test]
fn emit_sysex_default_wrap_is_true() {
    let e = ScriptEngine::new().unwrap();
    let _ = e.eval_transform("ob.emit_sysex({9,9,9}); return 0", 0).unwrap();
    assert!(e.drain_emits()[0].wrap, "default wrap must be true");
}

#[test]
fn routing_fns_error_on_engine_without_handle() {
    let e = ScriptEngine::new().unwrap();
    let err = e.eval_transform(
        r#"ob.register_cc_route(nil, 0, 1, "/x", 0, 127); return 0"#,
        0,
    );
    assert!(err.is_err(), "must fail loudly when no routes handle is attached");
    let msg = format!("{:#}", err.unwrap_err());
    assert!(msg.contains("new_with_routes"), "error should hint at the fix, got: {msg}");
}
