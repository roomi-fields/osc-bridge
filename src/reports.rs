//! Usage reports and the trust they earn a driver.
//!
//! Anyone using the hosted server can report how a driver behaved on their
//! device: it works, it fails, or it partly works (`Outcome`), for the whole
//! driver or for one OSC address, with the firmware version and, when the
//! device answered the MIDI Identity Request, its identity (which carries
//! the firmware as the device itself reports it, and proves it was really
//! connected). Reports are appended to a JSON-lines file and replayed at
//! startup, so trust is always recomputed from the full record.
//!
//! Concordance, no human review, per firmware. Each user counts once per
//! target and firmware — their latest report wins. Within one firmware, a
//! target is `usage-verified` when at least `MIN_CONCORDANT` distinct users
//! report that it works, none reports a failure or a partial result, and one
//! of them had the device identified; one contrary report makes it
//! `disputed`. Across firmwares, works on one and fails on another is not a
//! contradiction but `firmware-dependent`. A contrary report whose firmware
//! is unknown cannot be placed: the target is `disputed` and flagged
//! `needs_firmware`, which asks the next reporters to identify their device
//! (see `LEARN_FIRMWARE`). The identities seen for each firmware are kept, so
//! the catalogue learns how each device reports its versions.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::{BufRead, Write};
use std::path::Path;
use std::sync::Mutex;

/// Distinct users who must agree, on one firmware, before a target is
/// `usage-verified`.
pub const MIN_CONCORDANT: usize = 3;

/// Reports one user may file per UTC day.
pub const DAILY_CAP: usize = 100;

/// Reports one network client (address, as seen by the server) may file per
/// UTC day, whatever user ids it presents — user ids are free to mint, so
/// this is what bounds one source. Counted in memory only, never written.
pub const CLIENT_DAILY_CAP: usize = 30;

/// Reports the whole server accepts per UTC day.
pub const GLOBAL_DAILY_CAP: usize = 5000;

/// Size the report file may reach; past it, new reports are refused. It
/// bounds the disk used and the replay at startup (well under a second).
pub const MAX_FILE_BYTES: u64 = 50 * 1024 * 1024;

/// Longest free-text note kept with a report.
pub const MAX_NOTE: usize = 500;

/// Firmware group of the reports that state none and carry no identity.
pub const UNKNOWN_FIRMWARE: &str = "unknown";

/// What an agent is asked to do when a target needs firmware information.
pub const LEARN_FIRMWARE: &str = "Reports disagree and the firmware of some is unknown. \
    Before reporting, identify the device: send the MIDI Identity Request F0 7E 7F 06 01 F7 \
    (detect_hardware does it locally) and pass the reply as identity_hex, and give the \
    firmware version shown in the device's menu or system page as firmware.";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Works,
    Fails,
    Partial,
}

impl Outcome {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "works" => Some(Self::Works),
            "fails" => Some(Self::Fails),
            "partial" => Some(Self::Partial),
            _ => None,
        }
    }
}

/// The device's answer to the MIDI Identity Request, in hex.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceIdentity {
    pub manufacturer: String,
    pub family: u16,
    pub model: u16,
    /// The four version bytes, as the device sends them.
    pub version: String,
}

impl From<&crate::detect::Identity> for DeviceIdentity {
    fn from(id: &crate::detect::Identity) -> Self {
        let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
        Self { manufacturer: hex(&id.manufacturer), family: id.family, model: id.model, version: hex(&id.version) }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Report {
    pub ts: String,
    pub user: String,
    /// Driver id: its path under `devices/` without `.json`.
    pub device: String,
    /// One OSC address of the driver, or None for the driver as a whole.
    pub address: Option<String>,
    pub outcome: Outcome,
    /// Firmware version as the reporter states it.
    pub firmware: Option<String>,
    /// The device's identity, when it answered during the test.
    pub identity: Option<DeviceIdentity>,
    pub note: Option<String>,
}

impl Report {
    /// The firmware group the report counts in: the stated version, else the
    /// version the device reported, else `UNKNOWN_FIRMWARE`.
    pub fn firmware_key(&self) -> String {
        match (&self.firmware, &self.identity) {
            (Some(f), _) => f.clone(),
            (None, Some(id)) => format!("identity {}", id.version),
            (None, None) => UNKNOWN_FIRMWARE.to_string(),
        }
    }
}

/// Where a target stands, from its reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    Unreported,
    Unconfirmed,
    UsageVerified,
    FirmwareDependent,
    Failing,
    Disputed,
}

impl Status {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Unreported => "unreported",
            Self::Unconfirmed => "unconfirmed",
            Self::UsageVerified => "usage-verified",
            Self::FirmwareDependent => "firmware-dependent",
            Self::Failing => "failing",
            Self::Disputed => "disputed",
        }
    }
}

/// The latest report of each user on one target, for one firmware.
#[derive(Default)]
struct Votes {
    by_user: HashMap<String, Report>,
}

impl Votes {
    fn count(&self, o: Outcome) -> usize {
        self.by_user.values().filter(|r| r.outcome == o).count()
    }

    fn status(&self) -> Status {
        let works = self.count(Outcome::Works);
        let against = self.count(Outcome::Fails) + self.count(Outcome::Partial);
        let identified = self.by_user.values().any(|r| r.outcome == Outcome::Works && r.identity.is_some());
        match (works, against) {
            (0, 0) => Status::Unreported,
            (0, _) => Status::Failing,
            (_, n) if n > 0 => Status::Disputed,
            (w, _) if w >= MIN_CONCORDANT && identified => Status::UsageVerified,
            _ => Status::Unconfirmed,
        }
    }

    fn summary(&self) -> Value {
        json!({
            "status": self.status().as_str(),
            "works": self.count(Outcome::Works),
            "fails": self.count(Outcome::Fails),
            "partial": self.count(Outcome::Partial),
        })
    }
}

/// All the votes on one target (a driver as a whole, or one address),
/// grouped by firmware.
#[derive(Default)]
struct Target {
    by_firmware: BTreeMap<String, Votes>,
}

impl Target {
    /// A user who reports with a known firmware has identified their device:
    /// their earlier report of unknown firmware on this target is taken as
    /// superseded.
    fn cast(&mut self, r: &Report) {
        let fw = r.firmware_key();
        if fw != UNKNOWN_FIRMWARE {
            if let Some(unknown) = self.by_firmware.get_mut(UNKNOWN_FIRMWARE) {
                unknown.by_user.remove(&r.user);
            }
        }
        self.by_firmware.entry(fw).or_default().by_user.insert(r.user.clone(), r.clone());
    }

    /// The target's status across firmwares, and whether placing a contrary
    /// report needs its firmware.
    fn status(&self) -> (Status, bool) {
        let groups: Vec<(&str, Status)> = self.by_firmware.iter()
            .map(|(fw, v)| (fw.as_str(), v.status()))
            .filter(|(_, s)| *s != Status::Unreported)
            .collect();
        if groups.is_empty() {
            return (Status::Unreported, false);
        }
        let positive = |s: Status| matches!(s, Status::UsageVerified | Status::Unconfirmed);
        let any_works = groups.iter().any(|(_, s)| positive(*s));
        let any_failing = groups.iter().any(|(_, s)| *s == Status::Failing);
        let unknown = groups.iter().find(|(fw, _)| *fw == UNKNOWN_FIRMWARE).map(|(_, s)| *s);
        let unknown_conflicts = match unknown {
            Some(Status::Disputed) => groups.len() > 1,
            Some(s) if positive(s) => any_failing,
            Some(Status::Failing) => any_works,
            _ => false,
        };
        if unknown_conflicts {
            return (Status::Disputed, true);
        }
        if groups.iter().any(|(_, s)| *s == Status::Disputed) {
            return (Status::Disputed, unknown == Some(Status::Disputed));
        }
        let status = match (any_works, any_failing) {
            (true, true) => Status::FirmwareDependent,
            (false, _) => Status::Failing,
            (true, false) if groups.iter().any(|(_, s)| *s == Status::UsageVerified) => Status::UsageVerified,
            (true, false) => Status::Unconfirmed,
        };
        (status, false)
    }

    fn summary(&self) -> Value {
        let (status, needs_firmware) = self.status();
        let users = |o: Outcome| -> usize {
            self.by_firmware.values()
                .flat_map(|v| v.by_user.iter().filter(move |(_, r)| r.outcome == o).map(|(u, _)| u.as_str()))
                .collect::<BTreeSet<_>>().len()
        };
        let mut identities: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
        for (fw, v) in &self.by_firmware {
            for id in v.by_user.values().filter_map(|r| r.identity.as_ref()) {
                identities.entry(fw.as_str()).or_default()
                    .insert(format!("{} {:04x} {:04x} {}", id.manufacturer, id.family, id.model, id.version));
            }
        }
        let mut out = json!({
            "status": status.as_str(),
            "needs_firmware": needs_firmware,
            "works": users(Outcome::Works),
            "fails": users(Outcome::Fails),
            "partial": users(Outcome::Partial),
            "firmwares": self.by_firmware.iter().map(|(fw, v)| (fw.clone(), v.summary())).collect::<BTreeMap<_, _>>(),
            "identities": identities,
            "last_report": self.by_firmware.values().flat_map(|v| v.by_user.values().map(|r| r.ts.as_str())).max(),
        });
        if needs_firmware {
            out["learn"] = json!(LEARN_FIRMWARE);
        }
        out
    }
}

/// All the votes on one driver: per OSC address, and on the driver as a
/// whole — each user's overall verdict per firmware (see `cast`).
#[derive(Default)]
struct DeviceVotes {
    overall: Target,
    by_address: BTreeMap<String, Target>,
    /// Latest report of each user on each (address, firmware).
    latest: HashMap<(String, Option<String>, String), Report>,
}

impl DeviceVotes {
    /// A user's overall verdict on a driver, for one firmware: fails if any
    /// of their latest reports on it fails, works if all work, partial
    /// otherwise.
    fn cast(&mut self, r: &Report) {
        if let Some(a) = &r.address {
            self.by_address.entry(a.clone()).or_default().cast(r);
        }
        let fw = r.firmware_key();
        if fw != UNKNOWN_FIRMWARE {
            self.latest.retain(|(u, _, f), _| !(*u == r.user && f == UNKNOWN_FIRMWARE));
        }
        self.latest.insert((r.user.clone(), r.address.clone(), fw.clone()), r.clone());
        let mine: Vec<&Report> = self.latest.iter()
            .filter(|((u, _, f), _)| *u == r.user && *f == fw)
            .map(|(_, v)| v).collect();
        let outcome = if mine.iter().any(|x| x.outcome == Outcome::Fails) {
            Outcome::Fails
        } else if mine.iter().all(|x| x.outcome == Outcome::Works) {
            Outcome::Works
        } else {
            Outcome::Partial
        };
        let overall = Report {
            outcome,
            identity: r.identity.clone().or_else(|| mine.iter().find_map(|x| x.identity.clone())),
            address: None,
            ..r.clone()
        };
        self.overall.cast(&overall);
    }
}

#[derive(Default)]
struct State {
    devices: BTreeMap<String, DeviceVotes>,
    /// Reports filed per (user, UTC day), for `DAILY_CAP`.
    per_day: HashMap<(String, String), usize>,
    /// Reports filed per (network client, UTC day), for `CLIENT_DAILY_CAP`.
    per_client_day: HashMap<(String, String), usize>,
    /// Reports filed per UTC day, for `GLOBAL_DAILY_CAP`.
    per_global_day: HashMap<String, usize>,
    /// Size of the report file.
    bytes: u64,
}

impl State {
    fn apply(&mut self, r: &Report) {
        self.devices.entry(r.device.clone()).or_default().cast(r);
        *self.per_day.entry((r.user.clone(), day(&r.ts))).or_default() += 1;
        *self.per_global_day.entry(day(&r.ts)).or_default() += 1;
    }
}

fn day(ts: &str) -> String {
    ts.get(..10).unwrap_or(ts).to_string()
}

/// The reports of a running server: in memory, and appended to a file when
/// it has one.
pub struct ReportStore {
    file: Option<Mutex<std::fs::File>>,
    state: Mutex<State>,
}

impl ReportStore {
    /// Replay `path` (if it exists) and keep appending to it. None keeps
    /// reports in memory only (tests, local runs).
    pub fn open(path: Option<&Path>) -> Result<Self> {
        let mut state = State::default();
        let Some(path) = path else {
            return Ok(Self { file: None, state: Mutex::new(state) });
        };
        if let Ok(f) = std::fs::File::open(path) {
            for line in std::io::BufReader::new(f).lines().map_while(Result::ok) {
                state.bytes += line.len() as u64 + 1;
                if let Ok(r) = serde_json::from_str::<Report>(&line) {
                    state.apply(&r);
                }
            }
        }
        let mut options = std::fs::OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let file = options.open(path).map_err(|e| anyhow!("open reports {}: {e}", path.display()))?;
        Ok(Self { file: Some(Mutex::new(file)), state: Mutex::new(state) })
    }

    /// Record a report from network `client`, within the daily caps (per
    /// user, per client, server-wide) and `MAX_FILE_BYTES`.
    pub fn add(&self, r: Report, client: &str) -> std::result::Result<(), String> {
        let mut state = self.state.lock().map_err(|_| "report store unavailable")?;
        let today = day(&r.ts);
        if state.per_day.get(&(r.user.clone(), today.clone())).copied().unwrap_or(0) >= DAILY_CAP {
            return Err(format!("daily limit of {DAILY_CAP} reports reached for this user"));
        }
        if state.per_client_day.get(&(client.to_string(), today.clone())).copied().unwrap_or(0) >= CLIENT_DAILY_CAP {
            return Err(format!("daily limit of {CLIENT_DAILY_CAP} reports reached from this connection"));
        }
        if state.per_global_day.get(&today).copied().unwrap_or(0) >= GLOBAL_DAILY_CAP {
            return Err("the server takes no more reports today; try again tomorrow".into());
        }
        let line = serde_json::to_string(&r).map_err(|e| e.to_string())?;
        if state.bytes + line.len() as u64 + 1 > MAX_FILE_BYTES {
            return Err("the report store is full".into());
        }
        if let Some(file) = &self.file {
            let mut f = file.lock().map_err(|_| "report file unavailable")?;
            writeln!(f, "{line}").map_err(|e| format!("write report: {e}"))?;
        }
        state.bytes += line.len() as u64 + 1;
        state.per_client_day.retain(|(_, d), _| *d == today);
        *state.per_client_day.entry((client.to_string(), today)).or_default() += 1;
        state.apply(&r);
        Ok(())
    }

    /// Trust of one driver: overall, and per reported address.
    pub fn trust(&self, device: &str) -> Value {
        let Ok(state) = self.state.lock() else { return json!(null) };
        match state.devices.get(device) {
            Some(d) => json!({
                "device": device,
                "overall": d.overall.summary(),
                "addresses": d.by_address.iter().map(|(a, t)| (a.clone(), t.summary())).collect::<BTreeMap<_, _>>(),
            }),
            None => json!({ "device": device, "overall": Target::default().summary(), "addresses": {} }),
        }
    }

    /// Overall status of one driver.
    pub fn status(&self, device: &str) -> &'static str {
        self.state.lock().ok()
            .and_then(|s| s.devices.get(device).map(|d| d.overall.status().0.as_str()))
            .unwrap_or(Status::Unreported.as_str())
    }

    /// Overall trust of every reported driver.
    pub fn all(&self) -> Value {
        let Ok(state) = self.state.lock() else { return json!({}) };
        let devices: BTreeMap<&String, Value> = state.devices.iter().map(|(k, d)| (k, d.overall.summary())).collect();
        json!({ "min_concordant": MIN_CONCORDANT, "devices": devices })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(version: &str) -> Option<DeviceIdentity> {
        Some(DeviceIdentity { manufacturer: "42".into(), family: 0x2c, model: 1, version: version.into() })
    }

    fn report(user: &str, address: Option<&str>, outcome: Outcome, firmware: Option<&str>, identified: bool) -> Report {
        Report {
            ts: "2026-10-22T12:00:00Z".into(),
            user: user.into(),
            device: "korg/minilogue".into(),
            address: address.map(str::to_string),
            outcome,
            firmware: firmware.map(str::to_string),
            identity: if identified { identity("02010000") } else { None },
            note: None,
        }
    }

    fn overall(store: &ReportStore) -> Value {
        store.trust("korg/minilogue")["overall"].clone()
    }

    #[test]
    fn three_concordant_users_with_one_identified_device_verify_a_driver() {
        let store = ReportStore::open(None).unwrap();
        assert_eq!(overall(&store)["status"], "unreported");
        store.add(report("a", None, Outcome::Works, Some("2.1"), false), "net").unwrap();
        store.add(report("b", None, Outcome::Works, Some("2.1"), false), "net").unwrap();
        assert_eq!(overall(&store)["status"], "unconfirmed");
        store.add(report("c", None, Outcome::Works, Some("2.1"), false), "net").unwrap();
        assert_eq!(overall(&store)["status"], "unconfirmed", "no identified device yet");
        store.add(report("c", None, Outcome::Works, Some("2.1"), true), "net").unwrap();
        assert_eq!(overall(&store)["status"], "usage-verified");
        assert_eq!(overall(&store)["works"], 3, "a user counts once");
        assert_eq!(overall(&store)["identities"]["2.1"][0], "42 002c 0001 02010000");
    }

    #[test]
    fn a_failure_on_another_firmware_does_not_demote() {
        let store = ReportStore::open(None).unwrap();
        for u in ["a", "b", "c"] {
            store.add(report(u, None, Outcome::Works, Some("2.1"), true), "net").unwrap();
        }
        store.add(report("d", Some("/minilogue/filter/cutoff"), Outcome::Fails, Some("1.0"), false), "net").unwrap();
        let o = overall(&store);
        assert_eq!(o["status"], "firmware-dependent");
        assert_eq!(o["firmwares"]["2.1"]["status"], "usage-verified");
        assert_eq!(o["firmwares"]["1.0"]["status"], "failing");
        assert_eq!(o["needs_firmware"], false);
    }

    #[test]
    fn a_failure_on_the_same_firmware_disputes_and_a_correction_restores() {
        let store = ReportStore::open(None).unwrap();
        for u in ["a", "b", "c"] {
            store.add(report(u, None, Outcome::Works, Some("2.1"), true), "net").unwrap();
        }
        store.add(report("d", Some("/minilogue/filter/cutoff"), Outcome::Fails, Some("2.1"), false), "net").unwrap();
        assert_eq!(overall(&store)["status"], "disputed");
        let t = store.trust("korg/minilogue");
        assert_eq!(t["addresses"]["/minilogue/filter/cutoff"]["status"], "failing");
        store.add(report("d", Some("/minilogue/filter/cutoff"), Outcome::Works, Some("2.1"), false), "net").unwrap();
        assert_eq!(overall(&store)["status"], "usage-verified");
    }

    #[test]
    fn a_failure_of_unknown_firmware_disputes_and_asks_to_learn_it() {
        let store = ReportStore::open(None).unwrap();
        for u in ["a", "b", "c"] {
            store.add(report(u, None, Outcome::Works, Some("2.1"), true), "net").unwrap();
        }
        store.add(report("d", None, Outcome::Fails, None, false), "net").unwrap();
        let o = overall(&store);
        assert_eq!(o["status"], "disputed");
        assert_eq!(o["needs_firmware"], true);
        assert!(o["learn"].as_str().unwrap().contains("F0 7E 7F 06 01 F7"));
        // The same user comes back with their device identified: the failure
        // is placed on its firmware and is no longer a contradiction.
        let mut placed = report("d", None, Outcome::Fails, None, true);
        placed.identity = identity("01000000");
        store.add(placed, "net").unwrap();
        let o = overall(&store);
        assert_eq!(o["needs_firmware"], false);
        assert_eq!(o["status"], "firmware-dependent");
        assert_eq!(o["firmwares"]["identity 01000000"]["status"], "failing");
    }

    #[test]
    fn one_connection_cannot_pose_as_many_users() {
        let store = ReportStore::open(None).unwrap();
        for i in 0..CLIENT_DAILY_CAP {
            store.add(report(&format!("u{i}"), None, Outcome::Works, Some("2.1"), true), "net").unwrap();
        }
        let err = store.add(report("fresh", None, Outcome::Works, Some("2.1"), true), "net").unwrap_err();
        assert!(err.contains("connection"), "{err}");
        assert!(store.add(report("fresh", None, Outcome::Works, Some("2.1"), true), "elsewhere").is_ok());
    }

    #[test]
    fn reports_survive_a_restart_and_the_daily_cap_holds() {
        let path = std::env::temp_dir().join(format!("osc-bridge-reports-{}.jsonl", std::process::id()));
        let _ = std::fs::remove_file(&path);
        {
            let store = ReportStore::open(Some(&path)).unwrap();
            for u in ["a", "b", "c"] {
                store.add(report(u, None, Outcome::Works, Some("2.1"), true), "net").unwrap();
            }
        }
        let store = ReportStore::open(Some(&path)).unwrap();
        assert_eq!(overall(&store)["status"], "usage-verified");
        for i in 0..DAILY_CAP - 1 {
            store.add(report("a", None, Outcome::Works, Some("2.1"), true), &format!("net{i}")).unwrap();
        }
        assert!(store.add(report("a", None, Outcome::Works, Some("2.1"), true), "other").is_err(), "user cap");
        assert!(store.add(report("b", None, Outcome::Works, Some("2.1"), true), "other").is_ok());
        std::fs::remove_file(path).ok();
    }
}
