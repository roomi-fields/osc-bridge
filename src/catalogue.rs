//! Where the device catalogue lives.
//!
//! From a git checkout the catalogue is `./devices`. Installed through npm,
//! the binary sits in `<package>/vendor/` and the catalogue ships as
//! `<package>/devices/`, while the current directory is wherever the user or
//! the MCP client started the process; built from source, it sits in
//! `<checkout>/target/release/`. Relative catalogue paths are therefore
//! tried, in order: as given (current directory), under `$OSC_BRIDGE_HOME`,
//! then in the executable's directory and each of its parents.

use std::path::{Path, PathBuf};

fn bases() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(home) = std::env::var_os("OSC_BRIDGE_HOME") {
        out.push(PathBuf::from(home));
    }
    if let Ok(exe) = std::env::current_exe() {
        let exe = exe.canonicalize().unwrap_or(exe);
        out.extend(exe.ancestors().skip(1).map(Path::to_path_buf));
    }
    out
}

/// Resolve a catalogue file or directory (`devices`,
/// `devices/arturia/minilab3.json`). Returns the first location that exists,
/// or the path unchanged so the caller reports it as missing.
pub fn resolve(path: &Path) -> PathBuf {
    if path.exists() || path.is_absolute() {
        return path.to_path_buf();
    }
    bases().into_iter()
        .map(|b| b.join(path))
        .find(|p| p.exists())
        .unwrap_or_else(|| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn existing_relative_path_is_kept() {
        assert_eq!(resolve(Path::new("devices")), PathBuf::from("devices"));
    }

    #[test]
    fn osc_bridge_home_is_searched() {
        let home = std::env::temp_dir().join(format!("osc-bridge-home-{}", std::process::id()));
        std::fs::create_dir_all(home.join("devices/acme")).unwrap();
        std::fs::write(home.join("devices/acme/box.json"), "{}").unwrap();
        std::env::set_var("OSC_BRIDGE_HOME", &home);
        let got = resolve(Path::new("devices/acme/box.json"));
        std::env::remove_var("OSC_BRIDGE_HOME");
        assert_eq!(got, home.join("devices/acme/box.json"));
        std::fs::remove_dir_all(home).ok();
    }

    #[test]
    fn missing_path_comes_back_unchanged() {
        assert_eq!(resolve(Path::new("devices/nope/none.json")), PathBuf::from("devices/nope/none.json"));
    }
}
