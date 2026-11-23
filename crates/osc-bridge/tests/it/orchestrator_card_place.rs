//! The orchestrator takes the place of each card from its caller: the function it receives turns the
//! `spec` of every device (hardware or OSC) into the file it loads.

use anyhow::anyhow;
use osc_bridge::orchestrator::Orchestrator;
use std::cell::RefCell;
use std::path::{Path, PathBuf};

fn config(dir: &Path, devices: &str) -> PathBuf {
    let toml = dir.join("bridge.toml");
    std::fs::write(&toml, format!("[osc]\nbind = \"127.0.0.1:0\"\n\n{devices}")).unwrap();
    toml
}

fn hardware_card(dir: &Path) -> PathBuf {
    let file = dir.join("mapped-hardware.json");
    std::fs::write(&file, r#"{ "device": { "name": "Mapped Synth", "vendor": "Test", "osc_prefix": "/mapped" },
        "commands": [] }"#).unwrap();
    file
}

fn osc_card(dir: &Path) -> PathBuf {
    let file = dir.join("mapped-osc.json");
    std::fs::write(&file, r#"{ "device": { "name": "Mapped Soft", "vendor": "Test", "osc_prefix": "/soft",
        "kind": "software", "transport": { "kind": "osc", "host": "127.0.0.1", "port": 9 } }, "commands": [] }"#).unwrap();
    file
}

#[test]
fn a_place_function_that_fails_fails_the_launch_with_its_error() {
    let dir = crate::common::temp("card-place-fails");
    let toml = config(dir.path(), "[[devices]]\nspec = \"anything\"\nmidi_out_port = 0\n");
    let err = Orchestrator::run(&toml, |_: &Path| Err(anyhow!("no card at this place"))).unwrap_err();
    assert!(format!("{err:#}").contains("no card at this place"), "{err:#}");
}

#[test]
fn a_place_function_that_returns_a_file_makes_the_orchestrator_load_that_file() {
    let dir = crate::common::temp("card-place-loads");
    let card = hardware_card(dir.path());
    let toml = config(dir.path(), "[[devices]]\nspec = \"nowhere-on-disk\"\n");
    let asked = RefCell::new(Vec::new());
    let err = Orchestrator::run(&toml, |spec: &Path| {
        asked.borrow_mut().push(spec.to_path_buf());
        Ok(card.clone())
    }).unwrap_err();
    // The mapped card was loaded: the launch reaches the hardware check, which names its device.
    assert!(format!("{err:#}").contains("Mapped Synth"), "{err:#}");
    assert_eq!(*asked.borrow(), [PathBuf::from("nowhere-on-disk")]);
}

#[test]
fn the_place_function_is_applied_to_an_osc_device_and_then_to_the_next_device() {
    let dir = crate::common::temp("card-place-osc");
    let card = osc_card(dir.path());
    let toml = config(dir.path(), "[[devices]]\nspec = \"first-soft\"\n\n[[devices]]\nspec = \"second-any\"\nmidi_out_port = 0\n");
    let asked = RefCell::new(Vec::new());
    let err = Orchestrator::run(&toml, |spec: &Path| {
        asked.borrow_mut().push(spec.to_path_buf());
        if spec == Path::new("first-soft") { Ok(card.clone()) } else { Err(anyhow!("second place refused")) }
    }).unwrap_err();
    assert!(format!("{err:#}").contains("second place refused"), "{err:#}");
    assert_eq!(*asked.borrow(), [PathBuf::from("first-soft"), PathBuf::from("second-any")]);
}
