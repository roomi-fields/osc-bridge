//! End-to-end dynamic routing tests: Lua registers routes, runtime helpers
//! consume them. Exercises the full path from ScriptEngine → DynamicRoutes →
//! try_dynamic_cc_in / try_dynamic_osc_out.

use osc_bridge::routing::DynamicRoutes;
use osc_bridge::runtime::{try_dynamic_cc_in, try_dynamic_osc_out};
use osc_bridge::scripting::{Profile, ScriptEngine};
use rosc::{OscMessage, OscType};

fn register_via_lua(e: &ScriptEngine, code: &str) {
    let _ = e.eval_transform_with(Profile::PresetIngest, &format!("{code}\nreturn 0"), 0).unwrap();
}

#[test]
fn midi_cc_in_emits_semantic_osc_after_lua_register() {
    let routes = DynamicRoutes::handle();
    let e = ScriptEngine::new_with_routes(routes.clone()).unwrap();
    register_via_lua(&e, r#"ob.register_cc_route(nil, 0, 74, "/electra1/cutoff", 0, 127)"#);

    // Incoming MIDI: CC#74 = 64 on channel 0.
    let osc = try_dynamic_cc_in(&routes, &[0xB0, 74, 64]).expect("should route");
    assert_eq!(osc.addr, "/electra1/cutoff");
    match osc.args.first().unwrap() {
        OscType::Float(f) => assert!((*f - 64.0 / 127.0).abs() < 1e-4),
        other => panic!("expected Float, got {other:?}"),
    }
}

#[test]
fn midi_cc_in_falls_through_when_unregistered() {
    let routes = DynamicRoutes::handle();
    // No routes registered — helper returns None, letting the declarative
    // pipeline handle it.
    assert!(try_dynamic_cc_in(&routes, &[0xB0, 42, 100]).is_none());
}

#[test]
fn osc_semantic_addr_assembles_cc_bytes() {
    let routes = DynamicRoutes::handle();
    let e = ScriptEngine::new_with_routes(routes.clone()).unwrap();
    register_via_lua(&e, r#"ob.register_cc_route(nil, 3, 10, "/electra1/mix", 0, 127)"#);

    let msg = OscMessage {
        addr: "/electra1/mix".into(),
        args: vec![OscType::Float(0.5)],
    };
    let bytes = try_dynamic_osc_out(&routes, &msg).expect("should route");
    assert_eq!(bytes[0] & 0xF0, 0xB0);
    assert_eq!(bytes[0] & 0x0F, 3, "channel must be 3");
    assert_eq!(bytes[1], 10, "cc must be 10");
    assert!((bytes[2] as i32 - 64).abs() <= 1, "0.5 norm ≈ 64 u7 (got {})", bytes[2]);
}

#[test]
fn osc_int_arg_also_routed() {
    let routes = DynamicRoutes::handle();
    let e = ScriptEngine::new_with_routes(routes.clone()).unwrap();
    register_via_lua(&e, r#"ob.register_cc_route(nil, 0, 7, "/electra1/vol", 0, 127)"#);

    let msg = OscMessage {
        addr: "/electra1/vol".into(),
        args: vec![OscType::Int(127)],
    };
    let bytes = try_dynamic_osc_out(&routes, &msg).unwrap();
    assert_eq!(bytes[2], 127);
}

#[test]
fn page_switch_changes_cc_resolution() {
    let routes = DynamicRoutes::handle();
    let e = ScriptEngine::new_with_routes(routes.clone()).unwrap();
    // Same CC on two different pages points to different semantic addrs.
    register_via_lua(&e, r#"
        ob.register_cc_route(1, 0, 20, "/electra1/page1/filter", 0, 127)
        ob.register_cc_route(2, 0, 20, "/electra1/page2/reverb", 0, 127)
    "#);

    register_via_lua(&e, "ob.set_current_page(1)");
    let m1 = try_dynamic_cc_in(&routes, &[0xB0, 20, 50]).unwrap();
    assert_eq!(m1.addr, "/electra1/page1/filter");

    register_via_lua(&e, "ob.set_current_page(2)");
    let m2 = try_dynamic_cc_in(&routes, &[0xB0, 20, 50]).unwrap();
    assert_eq!(m2.addr, "/electra1/page2/reverb");
}

#[test]
fn clear_routes_wipes_mapping() {
    let routes = DynamicRoutes::handle();
    let e = ScriptEngine::new_with_routes(routes.clone()).unwrap();
    register_via_lua(&e, r#"ob.register_cc_route(nil, 0, 1, "/x", 0, 127)"#);
    assert!(try_dynamic_cc_in(&routes, &[0xB0, 1, 0]).is_some());
    register_via_lua(&e, "ob.clear_routes()");
    assert!(try_dynamic_cc_in(&routes, &[0xB0, 1, 0]).is_none());
}

#[test]
fn non_cc_midi_not_routed() {
    let routes = DynamicRoutes::handle();
    let e = ScriptEngine::new_with_routes(routes.clone()).unwrap();
    register_via_lua(&e, r#"ob.register_cc_route(nil, 0, 60, "/note_like", 0, 127)"#);
    // Note-on status 0x90 must NOT hit the CC route even if the "data1" byte
    // matches a registered CC number.
    assert!(try_dynamic_cc_in(&routes, &[0x90, 60, 100]).is_none());
}
