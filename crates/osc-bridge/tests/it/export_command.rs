//! The `export` command writes the document on its standard output and what the format could not
//! carry on its error output, never mixed.

const FIRMWARE: &str = "tests/fixtures/devices/arturia/microfreak.vendor.fw-microfreak-tools-0.1.0.json";
const OSC_ONLY: &str = "tests/fixtures/devices/bitwig/bitwig-studio.third-party-osc.fw-5.json";

fn export(device: &str, format: &str) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_osc-bridge")).args(["export", "--device", device, "--format", format]).output().unwrap()
}

#[test]
fn the_lost_elements_go_to_the_error_output_and_not_into_the_midnam_document() {
    let out = export(FIRMWARE, "midnam");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let (document, shown) = (String::from_utf8(out.stdout).unwrap(), String::from_utf8(out.stderr).unwrap());
    assert!(document.contains("<MIDINameDocument>"), "{document}");
    assert!(shown.contains("/sources/0/firmware"), "the lost firmware is shown with its place: {shown}");
    assert!(!document.contains("/sources/0/firmware"), "the document holds no list of losses");
}

#[test]
fn the_lost_elements_go_to_the_error_output_and_not_into_the_oscquery_document() {
    let out = export(FIRMWARE, "oscquery");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let shown = String::from_utf8(out.stderr).unwrap();
    let ns: serde_json::Value = serde_json::from_slice(&out.stdout).expect("the standard output is the document alone");
    assert_eq!(ns["FULL_PATH"], "/");
    assert!(shown.contains("/sources/0/firmware"), "{shown}");
}

#[test]
fn a_device_that_speaks_only_osc_fails_the_midnam_export_with_its_message() {
    let out = export(OSC_ONLY, "midnam");
    assert!(!out.status.success());
    assert!(out.stdout.is_empty());
    let shown = String::from_utf8(out.stderr).unwrap();
    assert!(shown.contains("speaks OSC, not MIDI: it has no MIDI names"), "{shown}");
}
