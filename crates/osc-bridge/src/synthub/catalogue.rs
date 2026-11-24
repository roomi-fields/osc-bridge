//! Where a driver comes from.
//!
//! The catalogue lives on the SynthHub server, which hands it out one driver
//! at a time (`crate::synthub::online`). A driver is named by a file on this machine,
//! or by its catalogue id (`korg/minilogue`) or OSC prefix (`minilogue`, the best
//! variant of the device): linked to the server, the driver is fetched into
//! the cache (`online::cache_dir`), downloaded again only when the server's
//! copy changed, and loaded from there; offline, only the cached drivers are
//! found.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

use crate::synthub::online::{driver_path, is_driver_id, Online};

/// The file of the driver `spec` names: a file on this machine, else a
/// catalogue id or OSC prefix, fetched when linked (`online::linked`), from the
/// cache otherwise or when the server cannot be reached.
pub fn locate(spec: &Path) -> Result<PathBuf> {
    if spec.exists() {
        return Ok(spec.to_path_buf());
    }
    let name = spec.to_str().filter(|s| is_driver_id(s) || is_slug(s))
        .with_context(|| format!("{}: no such file, nor a device (a catalogue id <maker>/<name>, or an OSC prefix)", spec.display()))?;
    let cache = crate::synthub::online::cache_dir().context("no cache directory (HOME is not set)")?;
    let online = crate::synthub::online::linked().then(|| Online::new(crate::synthub::online::SERVER));
    from_cache_or_server(name, &cache, online.as_ref())
}

/// A device slug (`minilogue`, `tr_8s`): the OSC prefix without its slash.
pub fn is_slug(s: &str) -> bool {
    !s.is_empty() && !s.starts_with('.') && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.+".contains(&b))
}

/// Driver `name` (an id or a slug), fetched from `online` into `cache`
/// when given, else (or when that fails) as cached.
pub fn from_cache_or_server(name: &str, cache: &Path, online: Option<&Online>) -> Result<PathBuf> {
    let fetched = online.map(|o| fetch(o, name, cache));
    match fetched {
        Some(Ok(path)) => return Ok(path),
        Some(Err(e)) => match cached(name, cache) {
            Some(path) => {
                eprintln!("{name}: {e:#}; using the cached copy");
                return Ok(path);
            }
            None => return Err(e),
        },
        None => {}
    }
    cached(name, cache).with_context(|| format!("{name} is not in the drivers cached on this machine ({}), \
        and this copy is offline (osc-bridge privacy --offline off links it to the SynthHub server)", cache.display()))
}

/// Fetch driver `name` (an id, or a slug: its best variant) into `cache`.
pub fn fetch(online: &Online, name: &str, cache: &Path) -> Result<PathBuf> {
    let id = if is_driver_id(name) {
        name.to_string()
    } else {
        let found = online.variants(name)?;
        match found["best"].as_str() {
            Some(id) => id.to_string(),
            None => bail!("the SynthHub server named no driver for {name:?}"),
        }
    };
    online.fetch_driver(&id, cache)
}

/// Lower is more trustworthy: tested on the device, then observed on it or
/// described by the vendor, then curated OSC surfaces, then community sources,
/// then what was contributed (checked automatically, not reviewed). Every type
/// of the OSD 0.6 format has its rank; an `x-` extension or unknown term ranks last.
pub fn tier_rank(tier: &str) -> u8 {
    match tier {
        "hardware-verified" | "software-verified" => 0,
        "hardware-observed" | "vendor-doc" | "vendor-self-described" => 1,
        "vendor-osc-api" | "third-party-osc" => 2,
        "electra-preset" => 3,
        "pencilresearch" | "knobkraft" | "midnam" | "community-re" => 4,
        "contributed" => 5,
        "test-fixture" => 6,
        _ => 7,
    }
}

/// The cached file of driver `name`: by id, or the best cached variant of a slug.
pub fn cached(name: &str, cache: &Path) -> Option<PathBuf> {
    if is_driver_id(name) {
        return Some(driver_path(cache, name)).filter(|p| p.is_file());
    }
    crate::mcp::DeviceIndex::build(cache, tier_rank).find_path(name).map(Path::to_path_buf)
}

#[cfg(test)]
mod tests;
