//! Where a driver comes from.
//!
//! The catalogue lives on the SynthHub server, which hands it out one driver
//! at a time (`crate::online`). A driver is named by a file on this machine,
//! or by its catalogue id (`korg/minilogue`) or slug (`minilogue`, the best
//! variant of the device): linked to the server, the driver is fetched into
//! the cache (`online::cache_dir`), downloaded again only when the server's
//! copy changed, and loaded from there; offline, only the cached drivers are
//! found.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

use crate::online::{driver_path, is_driver_id, Online};

/// The file of the driver `spec` names: a file on this machine, else a
/// catalogue id or slug, fetched when linked (`online::linked`), from the
/// cache otherwise or when the server cannot be reached.
pub fn locate(spec: &Path) -> Result<PathBuf> {
    if spec.exists() {
        return Ok(spec.to_path_buf());
    }
    let name = spec.to_str().filter(|s| is_driver_id(s) || is_slug(s))
        .with_context(|| format!("{}: no such file, nor a driver id (<maker>/<name>) or slug", spec.display()))?;
    let cache = crate::online::cache_dir().context("no cache directory (HOME is not set)")?;
    let online = crate::online::linked().then(|| Online::new(crate::online::SERVER));
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

/// The cached file of driver `name`: by id, or the best cached variant of a slug.
pub fn cached(name: &str, cache: &Path) -> Option<PathBuf> {
    if is_driver_id(name) {
        return Some(driver_path(cache, name)).filter(|p| p.is_file());
    }
    crate::mcp::DeviceIndex::build(cache).find_path(name).map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("osc-bridge-locate-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("acme")).unwrap();
        std::fs::write(root.join("acme/box.json"), r#"{"device":{"name":"Box","vendor":"Acme","osc_prefix":"/box"}}"#).unwrap();
        root
    }

    #[test]
    fn a_file_on_this_machine_is_used_as_it_is() {
        let root = cache("file");
        let file = root.join("acme/box.json");
        assert_eq!(locate(&file).unwrap(), file);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn offline_a_driver_is_found_in_the_cache_by_id_or_slug() {
        let root = cache("offline");
        assert_eq!(from_cache_or_server("acme/box", &root, None).unwrap(), root.join("acme/box.json"));
        assert_eq!(from_cache_or_server("box", &root, None).unwrap(), root.join("acme/box.json"));
        let missing = from_cache_or_server("acme/other", &root, None).unwrap_err().to_string();
        assert!(missing.contains("not in the drivers cached") && missing.contains("offline"), "{missing}");
        assert!(locate(Path::new("no/such/file.json")).unwrap_err().to_string().contains("no such file"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn an_unreachable_server_falls_back_to_the_cache() {
        let root = cache("unreachable");
        let server = Online::new("http://127.0.0.1:9");
        assert_eq!(from_cache_or_server("acme/box", &root, Some(&server)).unwrap(), root.join("acme/box.json"));
        assert!(from_cache_or_server("acme/other", &root, Some(&server)).is_err());
        std::fs::remove_dir_all(root).ok();
    }
}
