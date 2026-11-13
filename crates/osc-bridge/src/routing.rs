//! Dynamic routes — runtime-mutable CC ↔ OSC mappings for reconfigurable
//! controllers (Electra One MK2, Faderfox custom, MIDI Fighter Twister custom…).
//!
//! Static device specs (JSON) describe fixed controllers like MiniLab3 or
//! Subharmonicon, where a given CC always means the same thing. Reconfigurable
//! controllers redefine their control meaning per preset — the mapping has to
//! be rebuilt at runtime after each preset upload.
//!
//! This module owns the data. The Lua scripting layer (`scripting.rs`) gets
//! a handle and exposes `ob.register_cc_route` / `ob.clear_routes` /
//! `ob.set_current_page`. The runtime dispatcher (`runtime.rs`) consults these
//! tables on every MIDI-in CC and unknown OSC-in address, before falling back
//! to the static spec.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Lookup key for incoming MIDI CC. `page` is `None` for page-agnostic
/// routes; specific pages shadow the page-agnostic entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CcKey {
    pub channel: u8,
    pub cc: u8,
    pub page: Option<i64>,
}

/// Outbound direction: MIDI CC in → semantic OSC addr out, with linear scaling
/// from the raw u7 into the `[min, max]` range declared by the preset.
#[derive(Debug, Clone)]
pub struct CcRoute {
    pub osc_addr: String,
    pub min: i64,
    pub max: i64,
    pub channel: u8,
    pub cc: u8,
    /// Page tag, for reverse lookup decisions. Mirrors the CcKey page.
    pub page: Option<i64>,
}

#[derive(Debug, Default)]
pub struct DynamicRoutes {
    /// CC → semantic route. Multiple pages can share the same (ch, cc) keys.
    pub by_cc: HashMap<CcKey, CcRoute>,
    /// Semantic OSC addr → CC route (for outbound OSC→MIDI).
    pub by_osc: HashMap<String, CcRoute>,
    /// Active page for shadowing (`None` = only page-agnostic entries apply).
    pub current_page: Option<i64>,
}

impl DynamicRoutes {
    pub fn new() -> Self { Self::default() }

    pub fn handle() -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self::new()))
    }

    /// Register a route in both directions. Overwrites any prior route for
    /// the same key or OSC addr.
    pub fn register(&mut self, route: CcRoute) {
        let key = CcKey { channel: route.channel, cc: route.cc, page: route.page };
        self.by_osc.insert(route.osc_addr.clone(), route.clone());
        self.by_cc.insert(key, route);
    }

    pub fn clear(&mut self) {
        self.by_cc.clear();
        self.by_osc.clear();
        // current_page deliberately preserved — clearing routes on a page
        // switch would be wrong.
    }

    /// Resolve an incoming CC. Page-specific match wins over page-agnostic.
    pub fn lookup_cc(&self, channel: u8, cc: u8) -> Option<&CcRoute> {
        if let Some(p) = self.current_page {
            let k = CcKey { channel, cc, page: Some(p) };
            if let Some(r) = self.by_cc.get(&k) { return Some(r); }
        }
        let k = CcKey { channel, cc, page: None };
        self.by_cc.get(&k)
    }

    pub fn lookup_osc(&self, addr: &str) -> Option<&CcRoute> {
        self.by_osc.get(addr)
    }
}

/// Convert a raw u7 (0..127) to a normalized float in `[0.0, 1.0]` that the
/// route's [min, max] describes. The normalized form is what semantic OSC
/// routes carry — clients see a clean 0..1, never raw MIDI values.
pub fn u7_to_norm(raw: u8) -> f32 {
    (raw.min(127) as f32) / 127.0
}

/// Convert a normalized float back to a u7. Clamps both directions.
pub fn norm_to_u7(n: f32) -> u8 {
    let clamped = n.clamp(0.0, 1.0);
    (clamped * 127.0).round().clamp(0.0, 127.0) as u8
}

/// Scale a raw u7 through a preset-declared [min, max] range, then normalize.
/// Keeps downstream OSC traffic uniform regardless of the preset's native range.
pub fn cc_in_to_norm(raw: u8, _min: i64, _max: i64) -> f32 {
    // For Electra, the u7 coming in from the device IS the raw MIDI value —
    // the preset's min/max defines the semantic range users see, but the wire
    // value is still 0..127. We return 0..1 normalized; clients that want the
    // scaled value can do `min + n * (max - min)` themselves.
    u7_to_norm(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_route(addr: &str, ch: u8, cc: u8, page: Option<i64>) -> CcRoute {
        CcRoute { osc_addr: addr.into(), min: 0, max: 127, channel: ch, cc, page }
    }

    #[test]
    fn register_and_lookup_bidi() {
        let mut r = DynamicRoutes::new();
        r.register(make_route("/electra1/p1/cutoff", 0, 74, Some(1)));
        r.current_page = Some(1);
        assert_eq!(r.lookup_cc(0, 74).unwrap().osc_addr, "/electra1/p1/cutoff");
        assert_eq!(r.lookup_osc("/electra1/p1/cutoff").unwrap().cc, 74);
    }

    #[test]
    fn page_shadows_agnostic() {
        let mut r = DynamicRoutes::new();
        r.register(make_route("/agnostic", 0, 10, None));
        r.register(make_route("/p2_specific", 0, 10, Some(2)));
        // No current page → agnostic wins
        assert_eq!(r.lookup_cc(0, 10).unwrap().osc_addr, "/agnostic");
        // Page 2 active → page-specific wins
        r.current_page = Some(2);
        assert_eq!(r.lookup_cc(0, 10).unwrap().osc_addr, "/p2_specific");
        // Page 3 active → no specific entry, fall back to agnostic
        r.current_page = Some(3);
        assert_eq!(r.lookup_cc(0, 10).unwrap().osc_addr, "/agnostic");
    }

    #[test]
    fn clear_preserves_current_page() {
        let mut r = DynamicRoutes::new();
        r.current_page = Some(4);
        r.register(make_route("/x", 0, 1, Some(4)));
        r.clear();
        assert!(r.by_cc.is_empty());
        assert!(r.by_osc.is_empty());
        assert_eq!(r.current_page, Some(4));
    }

    #[test]
    fn different_channels_do_not_collide() {
        let mut r = DynamicRoutes::new();
        r.register(make_route("/a", 0, 20, None));
        r.register(make_route("/b", 5, 20, None));
        assert_eq!(r.lookup_cc(0, 20).unwrap().osc_addr, "/a");
        assert_eq!(r.lookup_cc(5, 20).unwrap().osc_addr, "/b");
    }

    #[test]
    fn norm_roundtrip_endpoints() {
        assert_eq!(norm_to_u7(u7_to_norm(0)), 0);
        assert_eq!(norm_to_u7(u7_to_norm(127)), 127);
        assert_eq!(norm_to_u7(u7_to_norm(64)), 64);
    }

    #[test]
    fn norm_clamps_out_of_range() {
        assert_eq!(norm_to_u7(-0.5), 0);
        assert_eq!(norm_to_u7(1.5), 127);
    }
}
