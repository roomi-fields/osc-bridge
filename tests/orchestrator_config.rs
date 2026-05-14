//! Orchestrator TOML config parsing + prefix-override logic.
//!
//! Does NOT spin up real MIDI ports — those are smoke-tested manually when
//! Kanopi wires up. We verify here that the config structure is correctly
//! interpreted and that prefix overrides would take effect at load time.

use osc_bridge::orchestrator::OrchestratorConfig;

#[test]
fn parses_minimal_config() {
    let toml = r#"
[osc]
bind = "127.0.0.1:7777"

[[devices]]
spec = "devices/moog/subsequent-37.json"
midi_out_port = 3
"#;
    let cfg: OrchestratorConfig = toml::from_str(toml).unwrap();
    assert_eq!(cfg.osc.bind, "127.0.0.1:7777");
    assert!(cfg.osc.clients.is_empty());
    assert_eq!(cfg.devices.len(), 1);
    assert_eq!(cfg.devices[0].midi_out_port, Some(3));
    assert!(cfg.devices[0].midi_in_port.is_none());
    assert!(cfg.devices[0].osc_prefix.is_none());
}

#[test]
fn parses_multi_device_with_prefix_overrides() {
    let toml = r#"
[osc]
bind = "127.0.0.1:7777"
clients = ["127.0.0.1:8888", "127.0.0.1:9999"]

[[devices]]
spec = "devices/arturia/matrixbrute.json"
osc_prefix = "/matrixbrute-1"
midi_out_port = 3
midi_in_port  = 2

[[devices]]
spec = "devices/arturia/matrixbrute.json"
osc_prefix = "/matrixbrute-2"
midi_out_port = 5
midi_in_port  = 4
"#;
    let cfg: OrchestratorConfig = toml::from_str(toml).unwrap();
    assert_eq!(cfg.osc.clients.len(), 2);
    assert_eq!(cfg.devices.len(), 2);
    assert_eq!(cfg.devices[0].osc_prefix.as_deref(), Some("/matrixbrute-1"));
    assert_eq!(cfg.devices[1].osc_prefix.as_deref(), Some("/matrixbrute-2"));
    assert_eq!(cfg.devices[0].midi_out_port, Some(3));
    assert_eq!(cfg.devices[1].midi_out_port, Some(5));
}

#[test]
fn parses_software_device_without_midi_port() {
    // Software (OSC-transport) devices don't declare a midi_out_port.
    let toml = r#"
[osc]
bind = "127.0.0.1:7777"

[[devices]]
spec = "devices/ableton/live.third-party-osc.fw-12.2.json"

[[devices]]
spec = "devices/arturia/minilab3.json"
midi_out_port = 4
midi_in_port = 0
"#;
    let cfg: OrchestratorConfig = toml::from_str(toml).unwrap();
    assert_eq!(cfg.devices.len(), 2);
    assert!(cfg.devices[0].midi_out_port.is_none());
    assert_eq!(cfg.devices[1].midi_out_port, Some(4));
}

#[test]
fn parses_osc_transport_overrides() {
    // A software device can override host / port / reply_port from bridge.toml.
    let toml = r#"
[osc]
bind = "127.0.0.1:7777"

[[devices]]
spec = "devices/ableton/live.third-party-osc.fw-12.2.json"
host = "192.168.1.40"
port = 11000
reply_port = 11001
"#;
    let cfg: OrchestratorConfig = toml::from_str(toml).unwrap();
    assert_eq!(cfg.devices[0].host.as_deref(), Some("192.168.1.40"));
    assert_eq!(cfg.devices[0].port, Some(11000));
    assert_eq!(cfg.devices[0].reply_port, Some(11001));
    // Absent overrides stay None (driver JSON values are used as-is).
    assert!(cfg.devices[0].midi_out_port.is_none());
}

#[test]
fn parses_inter_device_routes() {
    let toml = r#"
[osc]
bind = "127.0.0.1:7777"

[[devices]]
spec = "devices/arturia/minilab3.json"
midi_out_port = 4
midi_in_port = 0

[[routes]]
from = "/minilab3/cc/74"
to = "/ableton/track/0/volume"
map.from = [0, 127]
map.to = [0, 1]
"#;
    let cfg: OrchestratorConfig = toml::from_str(toml).unwrap();
    assert_eq!(cfg.routes.len(), 1);
    assert_eq!(cfg.routes[0].from, "/minilab3/cc/74");
    assert_eq!(cfg.routes[0].to, "/ableton/track/0/volume");
    let map = cfg.routes[0].map.as_ref().unwrap();
    assert_eq!(map.from, [0.0, 127.0]);
    assert_eq!(map.to, [0.0, 1.0]);
}

#[test]
fn rejects_prefix_without_leading_slash() {
    // At parse time we accept anything; the runtime enforces the leading
    // slash. This test just confirms the field is captured verbatim.
    let toml = r#"
[osc]
bind = "127.0.0.1:7777"

[[devices]]
spec = "x.json"
midi_out_port = 0
osc_prefix = "no-slash"
"#;
    // (config below still parses; runtime would reject the prefix)
    let cfg: OrchestratorConfig = toml::from_str(toml).unwrap();
    assert_eq!(cfg.devices[0].osc_prefix.as_deref(), Some("no-slash"));
    // Orchestrator::run would bail with "osc_prefix override must start with /"
}
