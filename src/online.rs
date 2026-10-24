//! The link between a copy of osc-bridge on the user's machine and the hosted
//! server (`SERVER`), so that every install shares in, and adds to, what the
//! community builds there:
//!
//! - the catalogue: the server publishes the list of its driver files with
//!   their SHA-256 (`GET /v1/catalogue`); the local copy mirrors it into a
//!   cache (`cache_dir`), fetching only the files that differ from those it
//!   already has (`POST /v1/catalogue/files`), so it gets the drivers other
//!   users and agents contributed or fixed since its release;
//! - trust: how far other users confirmed each driver (`GET /v1/trust`);
//! - reports and contributions: `report_result`, `report_mapping`,
//!   `get_trust`, `submit_driver`, `submit_fix` and `get_contribution` are
//!   forwarded to the server's MCP endpoint under the account kept on this
//!   machine (`account_file`, the same one `osc-bridge connect` uses).
//!
//! The MIDI ports never leave the machine. Sharing is on by default; the
//! user turns it off with `osc-bridge privacy` (`Settings`): `--offline on`
//! cuts the link (the bundled catalogue is used as is and nothing is sent),
//! `--private <device>` keeps one device's reports on the machine.
//! `osc-bridge mcp --offline` and `OSC_BRIDGE_OFFLINE=1` cut it for one run.

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const SERVER: &str = "https://mcp.synthhub.roomi-fields.com";

/// The tools a local copy forwards to the server.
pub const FORWARDED_TOOLS: [&str; 18] = ["report_result", "report_mapping", "get_trust",
    "submit_driver", "submit_fix", "get_contribution",
    "share_preset", "update_preset", "find_presets", "get_preset", "report_preset", "withdraw_preset",
    "comment_preset", "favorite_preset", "preset_collection", "follow_author", "set_profile", "link_account"];

/// Files fetched per `POST /v1/catalogue/files` request.
pub const FILES_PER_REQUEST: usize = 100;

/// Marks a complete cache, with the version of osc-bridge that filled it.
const COMPLETE: &str = ".complete";

/// Whether the user cut the link (`OSC_BRIDGE_OFFLINE`, any value but 0).
pub fn offline_by_env() -> bool {
    std::env::var("OSC_BRIDGE_OFFLINE").is_ok_and(|v| !v.is_empty() && v != "0")
}

fn dir_from(env: &[&str], home_sub: &str) -> Option<PathBuf> {
    env.iter().find_map(|k| std::env::var_os(k).map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(home_sub)))
}

/// Where the account id is kept: `$XDG_CONFIG_HOME`, `%APPDATA%` or
/// `~/.config`, under `osc-bridge/user`.
pub fn account_file() -> Option<PathBuf> {
    Some(dir_from(&["XDG_CONFIG_HOME", "APPDATA"], ".config")?.join("osc-bridge").join("user"))
}

/// Where the privacy settings are kept: next to the account, in
/// `osc-bridge/settings.toml`.
pub fn settings_file() -> Option<PathBuf> {
    Some(account_file()?.with_file_name("settings.toml"))
}

/// The user's privacy choices on this machine: a commented file the user may
/// edit by hand, or through `osc-bridge privacy`. The default shares: linked
/// to the server, no device private.
#[derive(Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Settings {
    /// Cut the link to the server.
    #[serde(default)]
    pub offline: bool,
    /// Devices whose reports, fixes and trust lookups stay on this machine:
    /// a catalogue id (`korg/minilogue`), a slug (`minilogue`, every variant
    /// of the device) or a whole vendor (`korg/*`).
    #[serde(default)]
    pub private: Vec<String>,
}

const SETTINGS_HELP: &str = "\
# osc-bridge — what this machine shares with the SynthHub server.
# Edit this file freely, or use `osc-bridge privacy`. Read when osc-bridge starts.
#
# Sharing is on by default: the catalogue stays up to date with the drivers
# other users and agents fixed and confirmed, and your reports and fixes go to
# the server under an anonymous account. MIDI ports and the traffic of your
# devices never leave this machine.
";

impl Settings {
    /// The settings at `path`, or the defaults when there are none.
    pub fn load(path: Option<&Path>) -> Result<Self> {
        let Some(path) = path else { return Ok(Self::default()) };
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).with_context(|| format!("read {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("read {}", path.display())),
        }
    }

    /// The file's text: the settings, each with what it does.
    pub fn render(&self) -> String {
        let list = self.private.iter().map(|p| format!("  {},\n", toml::Value::String(p.clone()))).collect::<String>();
        format!("{SETTINGS_HELP}
# true: do not contact the server at all (bundled catalogue, nothing sent).
offline = {}

# Devices kept private: nothing about them is sent to the server (reports,
# fixes, trust lookups). Each entry is a catalogue id (\"korg/minilogue\"), a
# slug for every variant of a device (\"minilogue\"), or a vendor (\"korg/*\").
private = [
{list}]
", self.offline)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() { std::fs::create_dir_all(dir)?; }
        std::fs::write(path, self.render()).with_context(|| format!("write {}", path.display()))
    }

    /// The settings at `path`, writing the defaults there first when the
    /// file does not exist yet, so the user finds it ready to edit.
    pub fn load_or_create(path: &Path) -> Result<Self> {
        if !path.exists() {
            Self::default().save(path)?;
        }
        Self::load(Some(path))
    }

    /// Whether `private` covers a driver, by its id and slug.
    pub fn covers(private: &[String], id: &str, slug: &str) -> bool {
        private.iter().any(|p| match p.strip_suffix("/*") {
            Some(vendor) => id.starts_with(&format!("{vendor}/")),
            None => p == id || p == slug,
        })
    }
}

/// Where the mirrored catalogue lives: `$XDG_CACHE_HOME`, `%LOCALAPPDATA%`
/// or `~/.cache`, under `osc-bridge/catalogue`.
pub fn cache_dir() -> Option<PathBuf> {
    Some(dir_from(&["XDG_CACHE_HOME", "LOCALAPPDATA"], ".cache")?.join("osc-bridge").join("catalogue"))
}

pub fn is_user_id(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes).as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

fn file_hash(path: &Path) -> Option<String> {
    std::fs::read(path).ok().map(|b| sha256_hex(&b))
}

/// A catalogue path as the server lists it (`korg/minilogue.json`): relative,
/// `/`-separated, no `.` or `..`, a driver or its notes.
pub fn is_catalogue_path(p: &str) -> bool {
    (p.ends_with(".json") || p.ends_with(".md")) && !p.contains('\\') && !p.starts_with('/')
        && p.split('/').all(|c| !c.is_empty() && c != "." && c != ".." && !c.starts_with('.'))
}

fn local(root: &Path, path: &str) -> PathBuf {
    path.split('/').fold(root.to_path_buf(), |p, c| p.join(c))
}

/// Every driver file (`.json`) and its notes (`.md`) under `root`, by
/// catalogue path, with its SHA-256.
pub fn catalogue_files(root: &Path) -> BTreeMap<String, String> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, String>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(root, &p, out);
            } else if let Ok(rel) = p.strip_prefix(root) {
                let rel = rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/");
                if is_catalogue_path(&rel) {
                    if let Some(h) = file_hash(&p) { out.insert(rel, h); }
                }
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

/// The cache, when a complete copy made by this version is there.
pub fn usable_cache() -> Option<PathBuf> {
    let dir = cache_dir()?;
    let made_by = std::fs::read_to_string(dir.join(COMPLETE)).ok()?;
    (made_by.trim() == env!("CARGO_PKG_VERSION")).then_some(dir)
}

/// What a catalogue sync did.
#[derive(Debug, Default, PartialEq)]
pub struct Synced {
    pub files: usize,
    pub copied: usize,
    pub downloaded: usize,
    pub removed: usize,
}

impl Synced {
    pub fn changed(&self) -> bool {
        self.copied + self.downloaded + self.removed > 0
    }
}

/// A client of the hosted server.
pub struct Online {
    base: String,
    agent: ureq::Agent,
    /// Where the account id is kept (`account_file` unless set otherwise).
    account_path: Option<PathBuf>,
}

impl Online {
    pub fn new(base: &str) -> Self {
        Self::keeping_account_in(base, account_file())
    }

    pub fn keeping_account_in(base: &str, account_path: Option<PathBuf>) -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .timeout_connect(Some(Duration::from_secs(5)))
            .user_agent(concat!("osc-bridge/", env!("CARGO_PKG_VERSION")))
            .http_status_as_error(false)
            .build()
            .new_agent();
        Self { base: base.trim_end_matches('/').to_string(), agent, account_path }
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    fn read(&self, mut resp: ureq::http::Response<ureq::Body>) -> Result<(u16, Value)> {
        let status = resp.status().as_u16();
        let text = resp.body_mut().with_config().limit(64 << 20).read_to_string()?;
        Ok((status, serde_json::from_str(&text).unwrap_or(Value::Null)))
    }

    fn get(&self, path: &str) -> Result<(u16, Value)> {
        let resp = self.agent.get(format!("{}{path}", self.base)).call().with_context(|| format!("GET {path}"))?;
        self.read(resp)
    }

    fn post(&self, path: &str, body: &Value) -> Result<(u16, Value)> {
        let resp = self.agent.post(format!("{}{path}", self.base))
            .header("Content-Type", "application/json")
            .send(body.to_string()).with_context(|| format!("POST {path}"))?;
        self.read(resp)
    }

    /// The account kept on this machine, or a new one the server issues
    /// (then kept). `forget` drops a kept id the server no longer knows.
    pub fn account(&self, forget: bool) -> Result<String> {
        let file = self.account_path.clone();
        if !forget {
            let kept = file.as_ref().and_then(|f| std::fs::read_to_string(f).ok()).map(|s| s.trim().to_ascii_lowercase());
            if let Some(id) = kept.filter(|id| is_user_id(id)) {
                return Ok(id);
            }
        }
        let (status, v) = self.get("/u/new")?;
        let id = v["user_id"].as_str().filter(|id| status == 200 && is_user_id(id))
            .ok_or_else(|| anyhow!("the server issued no account (HTTP {status})"))?.to_string();
        if let Some(f) = file {
            if let Some(dir) = f.parent() { let _ = std::fs::create_dir_all(dir); }
            let _ = std::fs::write(f, &id);
        }
        Ok(id)
    }

    /// Call one of the server's MCP tools as `user`; the result as the
    /// server gives it. Fails with the server's message on a protocol error.
    pub fn call_tool(&self, user: &str, name: &str, args: &Value) -> Result<Value> {
        let rpc = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": name, "arguments": args } });
        let (status, v) = self.post(&format!("/u/{user}/mcp"), &rpc)?;
        if status == 404 {
            bail!("unknown account");
        }
        match (v.get("result"), v.get("error")) {
            (Some(r), _) => Ok(r.clone()),
            (None, Some(e)) => bail!("{}", e["message"].as_str().unwrap_or("error")),
            _ => bail!("the server answered HTTP {status}"),
        }
    }

    /// Attach a preview (`audio` or `image`) to a preset the account shared.
    pub fn upload(&self, user: &str, preset: &str, kind: &str, bytes: &[u8]) -> Result<()> {
        let path = format!("/u/{user}/v1/presets/{preset}/{kind}");
        let resp = self.agent.put(format!("{}{path}", self.base))
            .header("Content-Type", "application/octet-stream")
            .send(bytes).with_context(|| format!("PUT {path}"))?;
        let (status, v) = self.read(resp)?;
        if status != 200 { bail!("{}", v["error"].as_str().unwrap_or("upload refused")); }
        Ok(())
    }

    /// Trust of every reported driver (`GET /v1/trust`).
    pub fn trust(&self) -> Result<Value> {
        let (status, v) = self.get("/v1/trust")?;
        if status != 200 { bail!("trust: HTTP {status}"); }
        Ok(v)
    }

    /// The synths the server's librarian can take sounds from (`GET /v1/librarian`).
    pub fn librarian_synths(&self) -> Result<Value> {
        let (status, v) = self.get("/v1/librarian")?;
        if status != 200 { bail!("{}", v["error"].as_str().unwrap_or("the server has no librarian")); }
        Ok(v)
    }

    /// One librarian step (`POST /v1/librarian/<op>`): recognize, request, receive.
    pub fn librarian(&self, op: &str, body: &Value) -> Result<Value> {
        let (status, v) = self.post(&format!("/v1/librarian/{op}"), body)?;
        if status != 200 { bail!("{}", v["error"].as_str().unwrap_or("librarian: refused")); }
        Ok(v)
    }

    /// The MIDI messages that load a shared preset, fitted to a synth on
    /// `channel` when the librarian took it (`GET /v1/presets/<id>/messages`).
    pub fn preset_messages(&self, id: &str, channel: Option<i64>) -> Result<Value> {
        let query = channel.map(|c| format!("?channel={c}")).unwrap_or_default();
        let (status, v) = self.get(&format!("/v1/presets/{id}/messages{query}"))?;
        if status != 200 { bail!("{}", v["error"].as_str().unwrap_or("no such preset")); }
        Ok(v)
    }

    /// Mirror the server's catalogue into `cache`: keep what matches, copy
    /// what `bundled` already has, download the rest, remove what the server
    /// no longer lists. The cache is marked complete only when all is there.
    pub fn sync_catalogue(&self, user: &str, bundled: &Path, cache: &Path) -> Result<Synced> {
        let (status, manifest) = self.get(&format!("/u/{user}/v1/catalogue"))?;
        if status != 200 { bail!("catalogue list: HTTP {status}"); }
        let wanted: BTreeMap<String, String> = manifest["files"].as_array().ok_or_else(|| anyhow!("no file list"))?
            .iter().filter_map(|f| Some((f["path"].as_str()?.to_string(), f["sha256"].as_str()?.to_string())))
            .filter(|(p, _)| is_catalogue_path(p)).collect();
        if wanted.is_empty() { bail!("the server lists no driver"); }
        let _ = std::fs::remove_file(cache.join(COMPLETE));
        let have = catalogue_files(cache);
        let mut done = Synced { files: wanted.len(), ..Synced::default() };
        let mut missing = Vec::new();
        for (path, hash) in &wanted {
            if have.get(path) == Some(hash) { continue; }
            let (src, dst) = (local(bundled, path), local(cache, path));
            if file_hash(&src).as_ref() == Some(hash) {
                std::fs::create_dir_all(dst.parent().expect("file in a directory"))?;
                std::fs::copy(&src, &dst).with_context(|| format!("copy {path}"))?;
                done.copied += 1;
            } else {
                missing.push(path.clone());
            }
        }
        for batch in missing.chunks(FILES_PER_REQUEST) {
            let (status, got) = self.post("/v1/catalogue/files", &json!({ "paths": batch }))?;
            if status != 200 { bail!("catalogue files: HTTP {status}"); }
            for path in batch {
                let content = got["files"][path].as_str().ok_or_else(|| anyhow!("{path} missing from the answer"))?;
                if sha256_hex(content.as_bytes()) != wanted[path] { bail!("{path} does not match its checksum"); }
                let dst = local(cache, path);
                std::fs::create_dir_all(dst.parent().expect("file in a directory"))?;
                std::fs::write(&dst, content).with_context(|| format!("write {path}"))?;
                done.downloaded += 1;
            }
        }
        for path in have.keys().filter(|p| !wanted.contains_key(*p)) {
            if std::fs::remove_file(local(cache, path)).is_ok() { done.removed += 1; }
        }
        std::fs::write(cache.join(COMPLETE), env!("CARGO_PKG_VERSION"))?;
        Ok(done)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_default_to_sharing_and_round_trip() {
        let dir = std::env::temp_dir().join(format!("osc-bridge-settings-{}", std::process::id()));
        let path = dir.join("osc-bridge/settings.toml");
        assert_eq!(Settings::load(Some(&path)).unwrap(), Settings { offline: false, private: vec![] });
        assert_eq!(Settings::load(None).unwrap(), Settings::default());
        let mine = Settings { offline: true, private: vec!["korg/minilogue".into()] };
        mine.save(&path).unwrap();
        assert_eq!(Settings::load(Some(&path)).unwrap(), mine);
        std::fs::write(&path, "private = [\"arturia/minilab3\"]\n").unwrap();
        assert_eq!(Settings::load(Some(&path)).unwrap(), Settings { offline: false, private: vec!["arturia/minilab3".into()] });
        let text = mine.render();
        assert!(text.contains("# true: do not contact the server") && text.contains("\"korg/minilogue\","), "{text}");
        assert_eq!(toml::from_str::<Settings>(&text).unwrap(), mine, "the commented file reads back");
        let fresh = dir.join("fresh/settings.toml");
        assert_eq!(Settings::load_or_create(&fresh).unwrap(), Settings::default());
        assert!(std::fs::read_to_string(&fresh).unwrap().contains("Edit this file freely"));
        let p = vec!["korg/*".to_string(), "minilab3".to_string(), "moog/sub37".to_string()];
        assert!(Settings::covers(&p, "korg/minilogue", "minilogue"));
        assert!(Settings::covers(&p, "arturia/minilab3.vendor.fw-1", "minilab3"));
        assert!(Settings::covers(&p, "moog/sub37", "sub37"));
        assert!(!Settings::covers(&p, "korgish/x", "x") && !Settings::covers(&p, "moog/sub37_v2", "sub37_v2"));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn catalogue_paths_stay_inside_the_catalogue() {
        assert!(is_catalogue_path("korg/minilogue.json"));
        assert!(is_catalogue_path("examples/test_synth.md"));
        for bad in ["../etc/passwd.json", "/abs.json", "korg/../x.json", "korg//x.json", "a\\b.json",
                    "korg/x.txt", ".complete", "korg/.hidden.json"] {
            assert!(!is_catalogue_path(bad), "{bad}");
        }
    }

    #[test]
    fn catalogue_files_lists_drivers_and_notes_with_their_checksum() {
        let root = std::env::temp_dir().join(format!("osc-bridge-cat-{}", std::process::id()));
        std::fs::create_dir_all(root.join("acme")).unwrap();
        std::fs::write(root.join("acme/box.json"), "{}").unwrap();
        std::fs::write(root.join("acme/box.md"), "# Box").unwrap();
        std::fs::write(root.join("acme/notes.txt"), "x").unwrap();
        std::fs::write(root.join(COMPLETE), "0").unwrap();
        let files = catalogue_files(&root);
        assert_eq!(files.keys().collect::<Vec<_>>(), ["acme/box.json", "acme/box.md"]);
        assert_eq!(files["acme/box.json"], sha256_hex(b"{}"));
        assert_eq!(sha256_hex(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        std::fs::remove_dir_all(root).ok();
    }
}
