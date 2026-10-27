//! The link between a copy of osc-bridge on the user's machine and the hosted
//! server (`SERVER`), so that every install shares in, and adds to, what the
//! community builds there:
//!
//! - the drivers: the catalogue lives on the server and is handed out one
//!   driver at a time, never whole. A local copy asks for the driver it needs
//!   — by id (`GET /api/v1/devices/<id>.json`, its notes `<id>.md`), by slug
//!   (`GET /v1/devices/<slug>`: the device's variants, best first) or for
//!   the MIDI ports it found (`POST /v1/detect`) — and keeps it in a cache
//!   (`cache_dir`), which is all it has offline. These requests carry no
//!   account;
//! - trust: how far other users confirmed each driver (`GET /v1/trust`);
//! - reports and contributions: `report_result`, `report_mapping`,
//!   `get_trust`, `submit_driver`, `submit_fix` and `get_contribution` are
//!   forwarded to the server's MCP endpoint under the account kept on this
//!   machine (`account_file`, the same one `osc-bridge connect` uses).
//!
//! The MIDI ports never leave the machine. Sharing is on by default; the
//! user turns it off with `osc-bridge privacy` (`Settings`): `--offline on`
//! cuts the link (only the drivers already cached are used, nothing is sent),
//! `--private <device>` keeps one device's reports on the machine.
//! `osc-bridge mcp --offline` and `OSC_BRIDGE_OFFLINE=1` cut it for one run.

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const SERVER: &str = "https://mcp.synthhub.roomi-fields.com";

/// The tools a local copy forwards to the server.
pub const FORWARDED_TOOLS: [&str; 18] = ["report_result", "report_mapping", "get_trust",
    "submit_driver", "submit_fix", "get_contribution",
    "share_preset", "update_preset", "find_presets", "get_preset", "report_preset", "withdraw_preset",
    "comment_preset", "star_preset", "preset_collection", "follow_author", "set_profile", "link_account"];

/// MIDI ports matched per `POST /v1/detect` request.
pub const PORTS_PER_REQUEST: usize = 32;

/// Whether the user cut the link (`OSC_BRIDGE_OFFLINE`, any value but 0).
pub fn offline_by_env() -> bool {
    std::env::var("OSC_BRIDGE_OFFLINE").is_ok_and(|v| !v.is_empty() && v != "0")
}

/// Whether this machine is linked to the server: neither the environment
/// nor the settings file cut the link.
pub fn linked() -> bool {
    !offline_by_env() && !settings_file().and_then(|f| Settings::load(Some(&f)).ok()).is_some_and(|s| s.offline)
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
# Sharing is on by default: the drivers your devices need come from the
# server, one at a time and without your account, as other users and agents
# fixed and confirmed them; your reports and fixes go to the server under an
# anonymous account. MIDI ports and the traffic of your devices never leave
# this machine.
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
# true: do not contact the server at all (drivers already cached only, nothing sent).
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

/// Where the drivers fetched from the server are kept: `$XDG_CACHE_HOME`,
/// `%LOCALAPPDATA%` or `~/.cache`, under `osc-bridge/drivers`, each at its
/// id (`korg/minilogue.json`, its notes `korg/minilogue.md`).
pub fn cache_dir() -> Option<PathBuf> {
    Some(dir_from(&["XDG_CACHE_HOME", "LOCALAPPDATA"], ".cache")?.join("osc-bridge").join("drivers"))
}

pub fn is_user_id(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes).as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

/// A driver id as the catalogue names it (`korg/minilogue`,
/// `arturia/microfreak.vendor.fw-1.0`): a maker folder and a file name
/// without `.json`, nothing that could leave the cache.
pub fn is_driver_id(s: &str) -> bool {
    let parts: Vec<&str> = s.split('/').collect();
    parts.len() == 2 && parts.iter().all(|c| !c.is_empty() && !c.starts_with('.')
        && c.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.+".contains(&b)))
}

/// Where driver `id` is kept under `root`.
pub fn driver_path(root: &Path, id: &str) -> PathBuf {
    let (maker, name) = id.split_once('/').unwrap_or(("", id));
    root.join(maker).join(format!("{name}.json"))
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

    /// `GET path` as text, for a driver file and its notes.
    fn get_text(&self, path: &str) -> Result<(u16, String)> {
        let mut resp = self.agent.get(format!("{}{path}", self.base)).call().with_context(|| format!("GET {path}"))?;
        let status = resp.status().as_u16();
        Ok((status, resp.body_mut().with_config().limit(16 << 20).read_to_string()?))
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

    /// Fetch driver `id` into `cache` (`driver_path`), with its notes, the
    /// drivers it takes its MIDI from (`midi_of`) and the maker profiles its
    /// blocks name (`device::resolve_profiles`, kept as
    /// `<maker>/_profile….json` beside the drivers), so that it loads there.
    /// A driver the server no longer has is an error; the cached copy stays.
    pub fn fetch_driver(&self, id: &str, cache: &Path) -> Result<PathBuf> {
        let mut queue = vec![id.to_string()];
        let mut seen = Vec::new();
        let limit = 1 + crate::device::MIDI_OF_DEPTH * (1 + 2 * crate::device::PROFILE_DEPTH);
        while let Some(id) = queue.pop() {
            if !is_driver_id(&id) { bail!("{id:?} is not a driver id (<maker>/<name>)"); }
            if seen.contains(&id) { continue; }
            if seen.len() > limit { break; }
            let profile = id.split_once('/').is_some_and(|(_, f)| f.starts_with("_profile"));
            let (status, text) = self.get_text(&format!("/api/v1/devices/{id}.json"))?;
            match status {
                200 => {}
                404 => bail!("the SynthHub catalogue has no driver {id}"),
                s => bail!("driver {id}: HTTP {s}"),
            }
            let doc: Value = serde_json::from_str(&text).with_context(|| format!("driver {id} from the server"))?;
            let path = driver_path(cache, &id);
            std::fs::create_dir_all(path.parent().expect("a maker folder"))?;
            std::fs::write(&path, &text).with_context(|| format!("write {}", path.display()))?;
            if !profile {
                let notes = path.with_extension("md");
                match self.get_text(&format!("/api/v1/devices/{id}.md"))? {
                    (200, md) => std::fs::write(&notes, md).with_context(|| format!("write {}", notes.display()))?,
                    (404, _) => { let _ = std::fs::remove_file(&notes); }
                    (s, _) => bail!("notes of {id}: HTTP {s}"),
                }
                queue.extend(doc["midi_of"].as_str().map(str::to_string));
            }
            queue.extend(crate::device::profiles_named(&doc).iter().filter_map(|p| crate::device::profile_file(p)));
            seen.push(id);
        }
        Ok(driver_path(cache, id))
    }

    /// The variants of a device, by one of its ids or its slug, best first
    /// (`GET /v1/devices/<id or slug>`): `{ "best": id, "variants": [...] }`.
    pub fn variants(&self, id_or_slug: &str) -> Result<Value> {
        let (status, v) = self.get(&format!("/v1/devices/{id_or_slug}"))?;
        match status {
            200 => Ok(v),
            404 => bail!("the SynthHub catalogue has no device {id_or_slug:?} (see list_devices)"),
            s => bail!("{}", v["error"].as_str().map(str::to_string).unwrap_or_else(|| format!("device {id_or_slug}: HTTP {s}"))),
        }
    }

    /// The maker requests to send while detecting, after the Universal
    /// Identity Request (`GET /v1/detect`): those the catalogue's identity
    /// blocks name, each with the ports it may go to.
    pub fn probes(&self) -> Result<Vec<crate::detect::Probe>> {
        let (status, v) = self.get("/v1/detect")?;
        if status != 200 { bail!("detect probes: HTTP {status}"); }
        v["probes"].as_array().into_iter().flatten().map(|p| {
            let request = crate::librarian::parse_hex(p["request_hex"].as_str().unwrap_or_default())?;
            if request.first() != Some(&0xF0) || request.last() != Some(&0xF7) { bail!("a detect probe is not one SysEx message"); }
            let ports = p["ports"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
            Ok(crate::detect::Probe { request, ports })
        }).collect()
    }

    /// The drivers matching MIDI ports found on this machine
    /// (`POST /v1/detect`): `ports` is `[{ "port", "replies_hex": [...] }]`
    /// (the SysEx each device sent back while detected), the answer
    /// `{ "ports": [{ "port", "candidates": [...] }] }` in the same order.
    pub fn detect(&self, ports: &[Value]) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        for batch in ports.chunks(PORTS_PER_REQUEST) {
            let (status, v) = self.post("/v1/detect", &json!({ "ports": batch }))?;
            if status != 200 { bail!("{}", v["error"].as_str().map(str::to_string).unwrap_or_else(|| format!("detect: HTTP {status}"))); }
            out.extend(v["ports"].as_array().cloned().unwrap_or_default());
        }
        Ok(out)
    }

    /// One of the server's catalogue tools (`list_devices`…), called without
    /// an account (`POST /mcp`).
    pub fn catalogue_call(&self, name: &str, args: &Value) -> Result<Value> {
        let rpc = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": name, "arguments": args } });
        let (status, v) = self.post("/mcp", &rpc)?;
        match (v.get("result"), v.get("error")) {
            (Some(r), _) => Ok(r.clone()),
            (None, Some(e)) => bail!("{}", e["message"].as_str().unwrap_or("error")),
            _ => bail!("the server answered HTTP {status}"),
        }
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
    fn driver_ids_stay_inside_the_cache() {
        assert!(is_driver_id("korg/minilogue") && is_driver_id("arturia/microfreak.vendor.fw-freakpatch-0.3.0"));
        for bad in ["../etc/passwd", "/abs", "korg/../x", "korg//x", "a\\b", "korg", "a/b/c", "korg/.hidden", "korg/x y", ""] {
            assert!(!is_driver_id(bad), "{bad}");
        }
        assert_eq!(driver_path(Path::new("/c"), "korg/minilogue"), Path::new("/c/korg/minilogue.json"));
        assert_eq!(driver_path(Path::new("/c"), "arturia/x.fw-1.0"), Path::new("/c/arturia/x.fw-1.0.json"));
        assert_eq!(sha256_hex(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    }
}
