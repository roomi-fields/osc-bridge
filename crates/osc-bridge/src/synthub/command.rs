//! The commands of the link that `main.rs` dispatches to: `privacy` and the linked `mcp`.

use anyhow::{Context, Result};
use std::path::PathBuf;

use super::catalogue::tier_rank;
use super::learn::detect_offline;
use super::online::{cache_dir, linked, offline_by_env, settings_file, Online, Settings, SERVER};
use crate::mcp::{DeviceIndex, McpOptions};

/// `privacy`: show or change what this machine shares.
pub fn privacy(offline: Option<String>, private: Vec<String>, public: Vec<String>) -> Result<()> {
    let path = settings_file().context("no configuration directory (HOME is not set)")?;
    let offline = offline.map(|o| o == "on");
    let cached = DeviceIndex::build(&cache_dir().unwrap_or_default(), tier_rank);
    // A device not cached here is looked up on the server, unless the link is being cut.
    let server = (offline != Some(true) && linked()).then(|| Online::new(SERVER));
    print!("{}", super::mcp::privacy(&path, &cached, server.as_ref(), offline, &private, &public)?);
    Ok(())
}

/// `mcp`: the linked server wraps the local one; offline, the local one serves alone.
pub fn mcp(devices_dir: Option<PathBuf>, default_target: String, offline: bool) -> Result<()> {
    let default_target = default_target.parse::<std::net::SocketAddr>()
        .with_context(|| format!("parse --default-target {default_target}"))?;
    let settings = match settings_file() {
        Some(path) => Settings::load_or_create(&path)?,
        None => Settings::default(),
    };
    let online = !offline && !settings.offline && !offline_by_env();
    // Linked, the server of the link wraps the local one; otherwise the local one serves
    // alone, over the folder of cards the user gave, else the cards cached on this machine.
    if online {
        super::mcp::run(McpOptions { devices_dir, default_target, tier_order: tier_rank, detect_mode: None }, &settings.private)
    } else {
        let devices_dir = devices_dir.or_else(cache_dir);
        crate::mcp::run(McpOptions {
            devices_dir, default_target, tier_order: tier_rank,
            detect_mode: Some(detect_offline),
        })
    }
}
