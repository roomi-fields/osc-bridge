//! The engine reads its drivers through `osd`: `device::load` gives the
//! `osd::Device` of a driver as the catalogue resolves it, a driver `osd`
//! refuses is refused with `osd`'s fault, the companion note is read by the
//! engine, and `identity.sends` / `identity.usb` are kept as written.

use crate::common::{device05, files, temp, D05, P05};
use osc_bridge::device::{docs_of, full_osc_path, load};
use rosc::{OscMessage, OscType};
use std::path::Path;

const FIXTURES: &str = "tests/fixtures/devices";

fn fixture_files() -> Vec<std::path::PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "json") {
                out.push(p);
            }
        }
    }
    let mut out = Vec::new();
    walk(Path::new(FIXTURES), &mut out);
    out.sort();
    out
}

fn kind_of(path: &Path) -> osd::Kind {
    if path.file_name().unwrap().to_string_lossy().starts_with("_profile") { osd::Kind::Profile } else { osd::Kind::Device }
}

fn catalogue(files_: &[(&str, &str)]) -> tempfile::TempDir {
    let root = temp("osd-driver");
    files(root.path(), files_);
    root
}

fn refusal(root: &tempfile::TempDir, rel: &str) -> anyhow::Error {
    load(root.path().join(rel)).expect_err(rel)
}

fn osd_fault(err: &anyhow::Error) -> &osd::Fault {
    err.downcast_ref::<osd::Fault>().unwrap_or_else(|| panic!("the cause is not an osd fault: {err:#}"))
}

// ---- reading through osd ----------------------------------------------------

/// A test driver is 0.5, or 0.6 when it carries a field only 0.6 defines (named `values` of a CC entry).
fn assert_schema(schema: Option<osd::Version>, text: &str, path: &Path) {
    match schema {
        Some(osd::Version::V0_5) => {}
        Some(osd::Version::V0_6) => assert!(text.contains("\"values\""), "{}: 0.6 without a 0.6-only field", path.display()),
        other => panic!("{}: {other:?}", path.display()),
    }
}

#[test]
fn every_test_driver_is_the_converters_output_and_reads_without_fault() {
    let all = fixture_files();
    assert!(all.len() >= 14, "{all:?}");
    for path in all {
        let text = std::fs::read_to_string(&path).unwrap();
        let kind = kind_of(&path);
        let read = osd::read(&text, kind).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        match read {
            osd::Descriptor::Device(d) => assert_schema(d.schema, &text, &path),
            osd::Descriptor::Profile(p) => assert_schema(p.schema, &text, &path),
            other => panic!("{}: {:?}", path.display(), other.kind()),
        }
        // The converter goes forward to 0.5 only: a 0.6 driver is already past it.
        if !text.contains("/osd/0.6/") {
            assert_eq!(osd::convert(&text, kind).unwrap(), text, "{}: committed as it comes out of the converter", path.display());
        }
    }
}

#[test]
fn the_engine_loads_every_test_driver_as_an_osd_device() {
    for path in fixture_files().into_iter().filter(|p| kind_of(p) == osd::Kind::Device) {
        let dev: osd::Device = load(&path).unwrap_or_else(|e| panic!("{}: {e:#}", path.display()));
        let osd::Descriptor::Device(read) = osd::read(&std::fs::read_to_string(&path).unwrap(), osd::Kind::Device).unwrap() else { unreachable!() };
        if read.midi_of.is_none() && read.identity.as_ref().and_then(|i| i.profile.as_ref()).is_none()
            && read.programs.as_ref().and_then(|p| p.profile.as_ref()).is_none() {
            assert_eq!(dev, *read, "{}: a driver that names no other file is what osd reads", path.display());
        }
        assert_eq!(dev.device.name, read.device.name, "{}", path.display());
    }
}

#[test]
fn a_driver_naming_a_profile_is_completed_by_osd_from_the_catalogue_folder() {
    let dev = load(Path::new(FIXTURES).join("acme/box.vendor.fw-2.json")).unwrap();
    let id = dev.identity.as_ref().unwrap();
    assert_eq!(id.profile.as_deref(), Some("acme"));
    assert_eq!(id.reply.as_ref().map(Vec::len), Some(10), "the reply of the profile");
    assert_eq!(id.firmware.as_ref().and_then(|f| f.as_ref()).map(|f| f.decode.as_str()), Some("raw_bytes"));
    assert_eq!(id.family.as_deref(), Some(&[1u8, 0][..]), "the model's own bytes");
}

#[test]
fn a_driver_sharing_another_files_midi_keeps_its_own_identity_and_takes_the_routes() {
    let stock = device05("Box", "/box", r#","commands":[{"osc":"/init","frame":[240,67,247]}]"#);
    let fork = format!(r#"{{"$schema":"{D05}","device":{{"name":"Box","vendor":"Acme","osc_prefix":"/box_fork"}},
        "midi_of":"acme/box","commands":[{{"osc":"/more","frame":[1]}}]}}"#);
    let root = catalogue(&[("acme/box.json", &stock), ("acme/fork.json", &fork)]);
    let dev = load(root.path().join("acme/fork.json")).unwrap();
    assert_eq!(dev.device.osc_prefix, "/box_fork");
    let cmds: Vec<String> = dev.commands.iter().flatten().map(|c| match &c.osc { osd::Address::Text(t) => t.clone(), other => format!("{other:?}") }).collect();
    assert_eq!(cmds, ["/init", "/more"], "the target's routes, then its own");
}

#[test]
fn a_full_osc_path_is_the_prefix_then_the_relative_path() {
    let dev = load(Path::new(FIXTURES).join("examples/test_synth.json")).unwrap();
    assert_eq!(full_osc_path(&dev, "/filter/cutoff"), "/test_synth/filter/cutoff");
    assert_eq!(full_osc_path(&dev, "/test_synth/filter/cutoff"), "/test_synth/filter/cutoff", "already prefixed");
    assert_eq!(full_osc_path(&dev, "filter/cutoff"), "/test_synth/filter/cutoff", "no leading slash");
}

// ---- no driver type of its own ----------------------------------------------

#[test]
fn the_engine_defines_no_driver_type_outside_osd() {
    // The old structures and their resolution are gone: no source file of the
    // engine declares a driver type or a resolver.
    let mut sources = Vec::new();
    fn walk(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") && !p.file_name().unwrap().to_string_lossy().starts_with("tests") && p.components().all(|c| c.as_os_str() != "tests") {
                out.push(p);
            }
        }
    }
    walk(Path::new("src"), &mut sources);
    let banned = [
        "pub struct Device ", "pub struct DeviceMeta", "pub struct DeviceIdentity", "pub struct Programs", "pub struct Command ",
        "pub struct CcParamEntry", "pub struct ParamEntry", "pub struct ReplyPattern", "pub struct MidiInMap", "pub struct ForwardSpec",
        "pub enum FrameToken", "pub struct TriggerEntry", "pub struct FirmwareRule", "pub struct Bank ", "pub struct Sysex",
        "pub fn resolve_midi_of", "pub fn resolve_profiles", "pub fn profile_file", "pub fn profiles_named", "pub const MIDI_OF_DEPTH",
        "pub const PROFILE_DEPTH",
    ];
    for path in sources {
        let text = std::fs::read_to_string(&path).unwrap();
        for b in banned {
            assert!(!text.contains(b), "{}: still holds `{b}`", path.display());
        }
    }
}

#[test]
fn the_documents_of_the_driver_format_point_to_osd_and_the_old_schema_document_is_gone() {
    assert!(!Path::new("../../docs/DEVICE_JSON_SCHEMA.md").exists(), "docs/DEVICE_JSON_SCHEMA.md is removed");
}

// ---- identity.sends and identity.usb ----------------------------------------

fn recognised_by_signs() -> String {
    device05("Pad", "/pad", r#","identity":{"ports":["Pad"],
        "usb":[{"vendor_id":4661,"product_id":22136},{"vendor_id":1,"product_id":2}],
        "sends":{"active_sensing":true,"clock":false,"note_off":"note_on_velocity_0"}}"#)
}

#[test]
fn the_signs_a_device_gives_of_itself_are_kept_as_written() {
    let root = catalogue(&[("acme/pad.json", &recognised_by_signs())]);
    let dev = load(root.path().join("acme/pad.json")).unwrap();
    let id: &osd::Identity = dev.identity.as_ref().unwrap();
    let usb = id.usb.as_ref().unwrap();
    assert_eq!(usb.iter().map(|u| (u.vendor_id, u.product_id)).collect::<Vec<_>>(), [(4661, 22136), (1, 2)]);
    let sends = id.sends.as_ref().unwrap();
    assert_eq!((sends.active_sensing, sends.clock, sends.note_off), (Some(true), Some(false), Some(osd::NoteOff::NoteOnVelocity0)));
}

#[test]
fn a_profile_brings_the_signs_and_the_driver_keeps_its_own() {
    let profile = format!(r#"{{"$schema":"{P05}","identity":{{"usb":[{{"vendor_id":9,"product_id":9}}],"sends":{{"clock":true}}}}}}"#);
    let own = device05("Pad", "/pad", r#","identity":{"profile":"acme","ports":["Pad"],"sends":{"active_sensing":true}}"#);
    let root = catalogue(&[("acme/_profile.json", &profile), ("acme/pad.json", &own)]);
    let id = load(root.path().join("acme/pad.json")).unwrap().identity.unwrap();
    assert_eq!(id.usb.as_ref().map(|u| (u[0].vendor_id, u[0].product_id)), Some((9, 9)), "from the profile");
    assert_eq!(id.sends.as_ref().and_then(|s| s.active_sensing), Some(true), "its own, in place of the profile's");
}

#[test]
fn the_signs_are_never_messages_the_engine_emits() {
    let with = device05("Pad", "/pad", r#","identity":{"request":[240,65,127,0,1,247],"reply":[240,126],
        "usb":[{"vendor_id":1,"product_id":2}],"sends":{"active_sensing":true,"clock":true}}"#);
    let without = device05("Pad", "/pad", r#","identity":{"request":[240,65,127,0,1,247],"reply":[240,126]}"#);
    let root = catalogue(&[("acme/with.json", &with), ("acme/without.json", &without)]);
    let probe = |f: &str| {
        let dev = load(root.path().join(f)).unwrap();
        osc_bridge::detect::probe_of(dev.identity.as_ref().unwrap()).unwrap().unwrap()
    };
    assert_eq!(probe("acme/with.json").request, probe("acme/without.json").request);
    assert_eq!(probe("acme/with.json").request, vec![0xF0, 0x41, 0x7F, 0x00, 0x01, 0xF7]);
}

#[test]
fn recognising_a_reply_ignores_the_signs_a_device_gives_of_itself() {
    let reply = [0xF0, 0x7E, 0x00, 0x06, 0x02, 0x41, 0x01, 0x02, 0x03, 0x04, 0x00, 0x00, 0x00, 0x00, 0xF7];
    let text = |signs: &str| device05("Pad", "/pad", &format!(r#","identity":{{"reply":[240,126,{{"device_id":true}},6,2,65],{signs}"device_id":{{"default":0}}}}"#));
    let root = catalogue(&[
        ("acme/with.json", &text(r#""usb":[{"vendor_id":1,"product_id":2}],"sends":{"clock":true},"#)),
        ("acme/without.json", &text("")),
    ]);
    let rec = |f: &str| {
        let dev = load(root.path().join(f)).unwrap();
        osc_bridge::detect::recognise(dev.identity.as_ref().unwrap(), "Pad", &[reply.to_vec()]).is_some()
    };
    assert_eq!(rec("acme/with.json"), rec("acme/without.json"));
}

// ---- refusals --------------------------------------------------------------

#[test]
fn a_driver_osd_refuses_fails_the_load_with_osds_fault() {
    let root = catalogue(&[
        ("acme/syntax.json", "{ not json"),
        ("acme/unknown.json", &device05("B", "/b", r#","wat":1"#)),
        ("acme/missing.json", &format!(r#"{{"$schema":"{D05}","device":{{"name":"B"}}}}"#)),
        ("acme/word.json", &device05("B", "/b", r#","identity":{"to":"some_ports","reply":[240]}"#)),
    ]);
    for (f, kind, at) in [("syntax", osd::FaultKind::Syntax, ""), ("unknown", osd::FaultKind::UnknownField, "/wat"),
                          ("missing", osd::FaultKind::Missing, "/device/osc_prefix"), ("word", osd::FaultKind::BadValue, "/identity/to")] {
        let err = refusal(&root, &format!("acme/{f}.json"));
        let fault = osd_fault(&err);
        assert_eq!((fault.kind, fault.pointer.as_str()), (kind, at), "{f}: {err:#}");
    }
}

#[test]
fn a_driver_whose_shared_files_osd_cannot_resolve_fails_with_the_typed_fault() {
    let link = |extra: &str| device05("B", "/b", extra);
    let root = catalogue(&[
        ("acme/other.json", &device05("C", "/c", "")),
        ("acme/lost.json", &link(r#","midi_of":"acme/none""#)),
        ("acme/c1.json", &link(r#","midi_of":"acme/c2""#)),
        ("acme/c2.json", &link(r#","midi_of":"acme/c1""#)),
        ("acme/maker.json", &link(r#","midi_of":"other/box""#)),
        ("acme/device.json", &link(r#","midi_of":"acme/other""#)),
        ("acme/body.json", &link(r#","midi_of":"acme/other","sysex":{}"#)),
        ("acme/noprofile.json", &link(r#","identity":{"profile":"acme/none","ports":["x"]}"#)),
        ("acme/_profile.p1.json", &format!(r#"{{"$schema":"{P05}","identity":{{"profile":"acme/p2"}}}}"#)),
        ("acme/_profile.p2.json", &format!(r#"{{"$schema":"{P05}","identity":{{"profile":"acme/p1"}}}}"#)),
        ("acme/circle.json", &link(r#","identity":{"profile":"acme/p1","ports":["x"]}"#)),
        ("acme/far.json", &link(r#","identity":{"profile":"other","ports":["x"]}"#)),
    ]);
    for (f, kind) in [("lost", osd::FaultKind::LinkMissing), ("c1", osd::FaultKind::LinkCycle), ("maker", osd::FaultKind::BadValue),
                      ("device", osd::FaultKind::BadValue), ("body", osd::FaultKind::UnknownField),
                      ("noprofile", osd::FaultKind::LinkMissing), ("circle", osd::FaultKind::LinkCycle), ("far", osd::FaultKind::BadValue)] {
        let err = refusal(&root, &format!("acme/{f}.json"));
        assert_eq!(osd_fault(&err).kind, kind, "{f}: {err:#}");
    }
}

#[test]
fn a_chain_of_shared_midi_links_stops_past_its_depth() {
    let mut list = vec![("acme/l0.json".to_string(), device05("Box", "/b", ""))];
    for n in 1..=9 {
        list.push((format!("acme/l{n}.json"), device05("Box", "/b", &format!(r#","midi_of":"acme/l{}""#, n - 1))));
    }
    let refs: Vec<(&str, &str)> = list.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    let root = catalogue(&refs);
    assert!(load(root.path().join("acme/l8.json")).is_ok());
    assert_eq!(osd_fault(&refusal(&root, "acme/l9.json")).kind, osd::FaultKind::LinkTooDeep);
}

/// The rules the old loader held for a driver, refused with a fault that
/// names the defect (by `osd` or by the engine on `osd`'s types).
#[test]
fn a_driver_that_breaks_a_rule_of_the_engine_or_the_format_is_refused_naming_the_defect() {
    let identity = |block: &str| device05("B", "/b", &format!(r#","identity":{block}"#));
    let programs = |block: &str| device05("B", "/b", &format!(r#","programs":{block}"#));
    let cases: Vec<(&str, String, Vec<&str>)> = vec![
        ("to-word", identity(r#"{"to":"some_ports","reply":[240]}"#), vec!["to"]),
        ("to-ports", identity(r#"{"to":"named_ports","reply":[240]}"#), vec!["ports"]),
        ("request-ends", identity(r#"{"request":[240,126,6,1],"reply":[240]}"#), vec!["request", "F7"]),
        ("request-dev", identity(r#"{"request":[240,126,{"device_id":true},6,1,247],"reply":[240]}"#), vec!["device_id"]),
        ("request-name", identity(r#"{"request":[240,126,{"arg":"slot"},6,1,247],"reply":[240]}"#), vec!["slot"]),
        ("empty", identity("{}"), vec!["identity"]),
        ("family", identity(r#"{"reply":[240,{"identity":"family"}]}"#), vec!["family"]),
        ("fw-word", identity(r#"{"reply":[240],"firmware":{"decode":"bcd","at":[1]}}"#), vec!["decode"]),
        ("fw-offsets", identity(r#"{"reply":[240],"firmware":{"decode":"korg_minor_major","at":[1]}}"#), vec!["offsets", "at"]),
        ("fw-field", identity(r#"{"reply":[240],"firmware":{"decode":"json","at":[1]}}"#), vec!["field"]),
        ("fw-reply", identity(r#"{"ports":["x"],"firmware":{"decode":"raw_bytes","at":[1]}}"#), vec!["firmware", "reply"]),
        ("unpack", identity(r#"{"reply":[240],"unpack":"x"}"#), vec!["unpack"]),
        ("pc-past", programs(r#"{"banks":[{"name":"A","first_pc":100,"size":64}]}"#), vec!["127"]),
        ("bank-twice", programs(r#"{"banks":[{"name":"A","size":1},{"name":"A","size":1}]}"#), vec!["twice", "A"]),
        ("bank-empty", programs(r#"{"banks":[{"name":"A","size":0}]}"#), vec!["size"]),
        ("frame-name", programs(r#"{"after":[[240,{"arg":"slot"},247]],"banks":[]}"#), vec!["slot"]),
        ("dump-kind", programs(r#"{"dump":{"kind":"sometimes"}}"#), vec!["kind"]),
        ("no-recall", programs(r#"{"program_change":false}"#), vec!["recall", "message"]),
        ("program-dev", programs(r#"{"before":[[240,{"device_id":true},247]],"banks":[]}"#), vec!["device_id"]),
    ];
    let list: Vec<(String, &str)> = cases.iter().map(|(n, t, _)| (format!("acme/{n}.json"), t.as_str())).collect();
    let refs: Vec<(&str, &str)> = list.iter().map(|(a, b)| (a.as_str(), *b)).collect();
    let root = catalogue(&refs);
    for (name, _, words) in &cases {
        let err = refusal(&root, &format!("acme/{name}.json"));
        let said = format!("{err:#}");
        assert!(words.iter().any(|w| said.contains(w)), "{name}: the refusal does not name the defect ({words:?}): {said}");
    }
}

#[test]
fn a_driver_the_old_rules_accepted_still_loads() {
    let root = catalogue(&[("acme/ok.json", &device05("B", "/b", r#","identity":{"reply":[240,126],"firmware":{"decode":"raw_bytes","at":[12]}},
        "programs":{"banks":[{"name":"A","msb":81,"size":128}],"dump":{"kind":"edit_buffer"}}"#))]);
    assert!(load(root.path().join("acme/ok.json")).is_ok());
}

// ---- the companion note -----------------------------------------------------

#[test]
fn the_note_beside_a_driver_is_read_with_it() {
    let docs = docs_of(Path::new(FIXTURES).join("electra-one/electra-one.json")).expect("electra-one.md is read");
    assert!(docs.contains("Electra"), "{}", &docs[..docs.len().min(80)]);
    assert!(docs_of(Path::new(FIXTURES).join("examples/scripted-example.json")).is_none(), "no note, none");
}

#[test]
fn a_firmware_sharing_midi_has_its_own_note_not_its_targets() {
    let stock = device05("Box", "/box", "");
    let fork = format!(r#"{{"$schema":"{D05}","device":{{"name":"Box","osc_prefix":"/fork"}},"midi_of":"acme/box"}}"#);
    let root = catalogue(&[("acme/box.json", &stock), ("acme/box.md", "# Box"), ("acme/fork.json", &fork), ("acme/fork.md", "# Fork")]);
    assert_eq!(docs_of(root.path().join("acme/fork.json")).as_deref(), Some("# Fork"));
    let bare = catalogue(&[("acme/box.json", &stock), ("acme/box.md", "# Box"), ("acme/fork.json", &fork)]);
    assert_eq!(docs_of(bare.path().join("acme/fork.json")), None);
}

// ---- the messages the engine emits stay the same -----------------------------

fn msg(addr: &str, args: Vec<OscType>) -> OscMessage {
    OscMessage { addr: addr.into(), args }
}

#[test]
fn the_virtual_synth_emits_the_same_bytes_in_0_5_as_in_0_4() {
    let dev = load(Path::new(FIXTURES).join("examples/test_synth.json")).unwrap();
    assert_eq!(osc_bridge::runtime::midi_out_for_msg(&dev, &msg("/test_synth/note/on", vec![OscType::Int(60), OscType::Int(100)])), Some(vec![0x90, 60, 100]));
    assert_eq!(osc_bridge::runtime::midi_out_for_msg(&dev, &msg("/test_synth/pitchbend", vec![OscType::Int(8192)])), Some(vec![0xE0, 0x00, 0x40]));
    let kick = osc_bridge::runtime::trigger_note_on(&dev, &msg("/test_synth/drum/kick", vec![OscType::Int(110)])).unwrap().unwrap();
    assert_eq!(kick, vec![0x99, 36, 110]);
    let hat = osc_bridge::runtime::trigger_note_on(&dev, &msg("/test_synth/drum/hat", vec![])).unwrap().unwrap();
    assert_eq!(hat, vec![0x99, 42, 100]);
}

#[test]
fn a_scripted_command_builds_the_same_frame_in_0_5_as_in_0_4() {
    use osc_bridge::frame::{build_frame, Bindings};
    let dev = load(Path::new(FIXTURES).join("examples/scripted-example.json")).unwrap();
    let cmd = &dev.commands.as_ref().unwrap()[0];
    let mut b = Bindings::new();
    b.set_u7("a", 0x12);
    b.set_u7("b", 0x34);
    b.set_u7("c", 0x56);
    let raw = build_frame(&dev, cmd.frame.as_ref().unwrap(), &b).unwrap();
    assert_eq!(raw, vec![0xF0, 0x7F, 0x00, 0x00, 0x10, 0x12, 0x34, 0x56, 0xF7]);
}

#[test]
fn what_agents_are_served_for_a_driver_is_the_same_in_0_5_as_in_0_4() {
    // The catalogue index agents are served lists each driver with what the 0.4 file said.
    let index = osc_bridge::mcp::DeviceIndex::build(Path::new(FIXTURES));
    let entry = |id: &str| index.entries.iter().find(|e| e.id == id).unwrap_or_else(|| panic!("{id} is listed"));
    let synth = entry("examples/test_synth");
    assert_eq!((synth.slug.as_str(), synth.name.as_str(), synth.vendor.as_str(), synth.tier.as_str()),
        ("test_synth", "Test Synth", "osc-bridge", "test-fixture"));
    assert_eq!((synth.firmware.as_deref(), synth.manufacturer_id.as_slice()), (Some("1.0"), &[125u8][..]));
    let mini = entry("korg/minilogue");
    assert_eq!((mini.slug.as_str(), mini.name.as_str(), mini.vendor.as_str(), mini.tier.as_str()),
        ("minilogue", "minilogue", "KORG", "pencilresearch"));
    assert_eq!(mini.firmware, None, "firmware \"unknown\" is not a firmware");
    let lc = entry("novation/launch-control-xl-3");
    assert_eq!((lc.slug.as_str(), lc.name.as_str(), lc.tier.as_str()), ("lcxl3", "Launch Control XL 3", "vendor-doc"));
    let boxed = entry("acme/box.vendor.fw-2");
    assert_eq!(boxed.identity.as_ref().map(|i: &osd::Identity| i.family.clone()), Some(Some(vec![1, 0])), "the profile applied");
}
