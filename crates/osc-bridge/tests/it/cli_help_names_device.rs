//! The command line names a device `device`, never a "slug": the help of the root and of every
//! sub-command, and the error of a device that is nowhere found.

use std::process::{Command, Output};

fn cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_osc-bridge")).args(args).output().unwrap()
}

fn text(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

/// The sub-commands the root help lists under `Commands:`.
fn sub_commands(root_help: &str) -> Vec<String> {
    root_help
        .lines()
        .skip_while(|l| !l.starts_with("Commands:"))
        .skip(1)
        .take_while(|l| l.starts_with("  ") && !l.trim().is_empty())
        .filter_map(|l| l.split_whitespace().next().map(str::to_string))
        .filter(|n| n != "help")
        .collect()
}

#[test]
fn the_root_help_does_not_say_slug() {
    let help = text(&cli(&["--help"]));
    assert!(help.contains("Commands:"), "{help}");
    assert!(!help.to_lowercase().contains("slug"), "{help}");
}

#[test]
fn the_help_of_every_sub_command_does_not_say_slug() {
    let names = sub_commands(&text(&cli(&["--help"])));
    for expected in ["run", "detect", "backup", "restore", "export", "privacy", "orchestrate", "inspect"] {
        assert!(names.iter().any(|n| n == expected), "{expected} in {names:?}");
    }
    for name in &names {
        let help = text(&cli(&[name, "--help"]));
        assert!(!help.is_empty(), "{name} has a help");
        assert!(!help.to_lowercase().contains("slug"), "{name}: {help}");
    }
}

#[test]
fn the_help_of_a_device_argument_says_a_catalogue_id_or_an_osc_prefix() {
    for args in [["inspect", "--help"], ["backup", "--help"], ["export", "--help"]] {
        let help = text(&cli(&args));
        assert!(help.contains("device"), "{args:?}: {help}");
    }
}

#[test]
fn a_device_found_nowhere_is_named_device_without_slug() {
    let o = cli(&["inspect", "no such device!"]);
    assert!(!o.status.success());
    let err = String::from_utf8_lossy(&o.stderr).to_lowercase();
    assert!(err.contains("device"), "{err}");
    assert!(!err.contains("slug"), "{err}");
}
