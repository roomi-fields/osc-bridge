//! Accounts, usage reports and the trust they earn a driver.
//!
//! Accounts: the hosted server issues every user id itself and keeps the
//! list; an id it did not issue is not an account. Each report belongs to
//! one account.
//!
//! Reports: how a driver behaved on a real device — it works, it fails, or
//! it partly works (`Outcome`) — for the whole driver or one OSC address,
//! with the firmware version and, when the device answered the MIDI Identity
//! Request, its identity (which carries the firmware as the device reports
//! it, and proves it was connected).
//!
//! Trust, per firmware, from accepted reports only. Each account counts once
//! per target and firmware — its latest report wins. Within one firmware, a
//! target is `usage-verified` when at least `MIN_CONCORDANT` accounts report
//! that it works, none reports a failure or a partial result, and one of
//! them had the device identified; one contrary report makes it `disputed`.
//! Works on one firmware and fails on another is `firmware-dependent`, not a
//! contradiction. A contrary report of unknown firmware cannot be placed: the
//! target is `disputed` and flagged `needs_firmware`, asking the next
//! reporters to identify their device (`LEARN_FIRMWARE`). The identities seen
//! for each firmware are kept, so the catalogue learns how devices report
//! their versions.
//!
//! Automatic rollback. A report that at least `MIN_CONCORDANT` other accounts
//! contradict on the same target and firmware, more than agree with it, is
//! rolled back: trust returns to what it was without it. Its account becomes
//! suspect, and every other report of that account goes to `pending` — out
//! of the trust count until approved. While an account is suspect, its new
//! reports are pending too. An administrator approves or rejects pending
//! reports and clears or suspects accounts (`Decision`); an approved report
//! is never rolled back automatically.
//!
//! Learned mappings. While a control is moved on the device, an agent
//! observes which MIDI message it sends (`Observation`): this driver address
//! — or a control the driver lacks, by its panel label — sends CC 74. Per
//! control and firmware, the message that at least `MIN_CONCORDANT` accounts
//! observed, more than any other, is learned (`Ledger::learned`); suspect
//! accounts are not counted. Learned mappings that differ from a driver are
//! what corrections of that driver are made from.
//!
//! Contributions. An account submits a driver file — a new device, or a
//! corrected variant built from learned mappings (`Contribution`). It waits
//! as `pending` until the catalogue's merge job (a GitHub workflow) picks it
//! up, checks it, merges it and reports back `merged` or `rejected`
//! (`ContributionStatus`). Contributions of a suspect account are held.
//!
//! Everything — accounts issued, reports, observations, contributions,
//! decisions — is one append-only
//! ledger (JSON lines) replayed in order at startup, so the outcome is the
//! same after a restart.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io::{BufRead, Write};
use std::path::Path;
use std::sync::Mutex;

/// Distinct accounts that must agree, on one firmware, before a target is
/// `usage-verified` — and that must contradict a report to roll it back.
pub const MIN_CONCORDANT: usize = 3;

/// Size the ledger may reach; past it, new entries are refused. It protects
/// the disk the server shares with other sites and bounds the replay at
/// startup (well under a second).
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
    /// Sequence number in the ledger, set when the report is recorded.
    #[serde(default)]
    pub id: u64,
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

/// Where a report stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReportStatus {
    /// Counts in the trust.
    Accepted,
    /// Out of the trust until an administrator approves it.
    Pending,
    /// Contradicted by other accounts and taken out automatically.
    RolledBack,
    /// Refused by an administrator.
    Rejected,
}

/// An administrator's decision, on one report or one account.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Decision {
    /// A report counts, and is no longer rolled back automatically.
    Approve,
    /// A report is out for good.
    Reject,
    /// An account's reports stop counting until approved.
    Suspect,
    /// An account is trusted again: its reports held only because of it count again.
    Clear,
}

/// The MIDI message a control sends.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Sends {
    Cc { cc: u8 },
    Nrpn { msb: u8, lsb: u8 },
    Note { note: u8 },
}

/// What one account saw a control send.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Observation {
    pub ts: String,
    pub user: String,
    /// Driver id.
    pub device: String,
    /// The driver's OSC address for the control, or `label:<panel label>`
    /// for a control the driver does not map.
    pub control: String,
    pub sends: Sends,
    pub channel: Option<u8>,
    pub firmware: Option<String>,
    pub identity: Option<DeviceIdentity>,
}

impl Observation {
    fn firmware_key(&self) -> String {
        match (&self.firmware, &self.identity) {
            (Some(f), _) => f.clone(),
            (None, Some(id)) => format!("identity {}", id.version),
            (None, None) => UNKNOWN_FIRMWARE.to_string(),
        }
    }
}

/// A driver file an account submitted for the catalogue.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Contribution {
    #[serde(default)]
    pub id: u64,
    pub ts: String,
    pub user: String,
    /// Where the file goes, under the repository root (`devices/...json`).
    pub path: String,
    /// The driver file, as it will be committed.
    pub content: String,
    /// `driver` (a new or replaced file) or `fix` (built from learned mappings).
    pub kind: String,
    pub note: Option<String>,
}

/// Where a contribution stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContributionState {
    Pending,
    Merged,
    Rejected,
}

/// One line of the ledger.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
enum Event {
    Account { ts: String, user: String },
    Report(Report),
    Observation(Observation),
    Contribution(Contribution),
    ContributionStatus { ts: String, id: u64, state: ContributionState, detail: Option<String>, pr: Option<String> },
    Decision { ts: String, decision: Decision, report: Option<u64>, user: Option<String> },
}

struct Entry {
    report: Report,
    status: ReportStatus,
    approved: bool,
}

/// Opposite verdicts, for rollback: a partial result counts against "works".
fn agrees(a: Outcome, b: Outcome) -> bool {
    (a == Outcome::Works) == (b == Outcome::Works)
}

#[derive(Default)]
struct State {
    accounts: HashSet<String>,
    suspect: HashSet<String>,
    entries: Vec<Entry>,
    by_device: HashMap<String, Vec<usize>>,
    observations: Vec<Observation>,
    contributions: Vec<(Contribution, ContributionState, Option<String>, Option<String>)>,
    bytes: u64,
}

impl State {
    fn apply(&mut self, e: &Event) {
        match e {
            Event::Account { user, .. } => {
                self.accounts.insert(user.clone());
            }
            Event::Report(r) => {
                let status = if self.suspect.contains(&r.user) { ReportStatus::Pending } else { ReportStatus::Accepted };
                let idx = self.entries.len();
                self.entries.push(Entry { report: Report { id: idx as u64, ..r.clone() }, status, approved: false });
                self.by_device.entry(r.device.clone()).or_default().push(idx);
                self.review(vec![r.device.clone()]);
            }
            Event::Observation(o) => self.observations.push(o.clone()),
            Event::Contribution(c) => {
                let id = self.contributions.len() as u64;
                self.contributions.push((Contribution { id, ..c.clone() }, ContributionState::Pending, None, None));
            }
            Event::ContributionStatus { id, state, detail, pr, .. } => {
                if let Some(c) = self.contributions.get_mut(*id as usize) {
                    (c.1, c.2, c.3) = (*state, detail.clone(), pr.clone());
                }
            }
            Event::Decision { decision, report, user, .. } => {
                let mut touched = Vec::new();
                if let Some(e) = report.and_then(|id| self.entries.get_mut(id as usize)) {
                    match decision {
                        Decision::Approve => { e.status = ReportStatus::Accepted; e.approved = true; }
                        Decision::Reject => { e.status = ReportStatus::Rejected; e.approved = false; }
                        _ => {}
                    }
                    touched.push(e.report.device.clone());
                }
                if let Some(u) = user {
                    match decision {
                        Decision::Suspect => touched.extend(self.suspect_account(u)),
                        Decision::Clear => {
                            self.suspect.remove(u);
                            for e in self.entries.iter_mut().filter(|e| e.report.user == *u && e.status == ReportStatus::Pending) {
                                e.status = ReportStatus::Accepted;
                                touched.push(e.report.device.clone());
                            }
                        }
                        _ => {}
                    }
                }
                self.review(touched);
            }
        }
    }

    /// Mark an account suspect and hold its accepted, unapproved reports.
    /// Returns the devices whose trust changed.
    fn suspect_account(&mut self, user: &str) -> Vec<String> {
        self.suspect.insert(user.to_string());
        self.entries.iter_mut()
            .filter(|e| e.report.user == user && e.status == ReportStatus::Accepted && !e.approved)
            .map(|e| { e.status = ReportStatus::Pending; e.report.device.clone() })
            .collect()
    }

    /// Roll back contradicted reports on these devices, and on every device
    /// a resulting suspicion touches, until nothing changes.
    fn review(&mut self, mut queue: Vec<String>) {
        while let Some(device) = queue.pop() {
            while let Some(idx) = self.contradicted(&device) {
                self.entries[idx].status = ReportStatus::RolledBack;
                let user = self.entries[idx].report.user.clone();
                queue.extend(self.suspect_account(&user));
            }
        }
    }

    /// The first accepted, unapproved report on `device` that at least
    /// `MIN_CONCORDANT` other accounts contradict on its target and firmware,
    /// more than agree with it (each account's latest report only).
    fn contradicted(&self, device: &str) -> Option<usize> {
        let mut latest: BTreeMap<(Option<String>, String), BTreeMap<&str, usize>> = BTreeMap::new();
        for &i in self.by_device.get(device)? {
            let e = &self.entries[i];
            if e.status == ReportStatus::Accepted {
                latest.entry((e.report.address.clone(), e.report.firmware_key())).or_default()
                    .insert(e.report.user.as_str(), i);
            }
        }
        latest.values().find_map(|group| group.values().copied().find(|&i| {
            let e = &self.entries[i];
            if e.approved {
                return false;
            }
            let others = group.values().filter(|&&j| j != i).map(|&j| self.entries[j].report.outcome);
            let (agree, oppose) = others.fold((0, 0), |(a, o), x| if agrees(x, e.report.outcome) { (a + 1, o) } else { (a, o + 1) });
            oppose >= MIN_CONCORDANT && oppose > agree
        }))
    }

    /// The votes of the accepted reports on one driver, in ledger order.
    fn votes(&self, device: &str) -> Option<DeviceVotes> {
        let idx = self.by_device.get(device)?;
        let mut votes = DeviceVotes::default();
        for &i in idx {
            if self.entries[i].status == ReportStatus::Accepted {
                votes.cast(&self.entries[i].report);
            }
        }
        Some(votes)
    }

    fn count(&self, device: &str, status: ReportStatus) -> usize {
        self.by_device.get(device)
            .map(|idx| idx.iter().filter(|&&i| self.entries[i].status == status).count())
            .unwrap_or(0)
    }

    fn summary(&self, device: &str) -> Value {
        let mut overall = self.votes(device).map(|v| v.overall.summary()).unwrap_or_else(|| Target::default().summary());
        overall["pending"] = json!(self.count(device, ReportStatus::Pending));
        overall["rolled_back"] = json!(self.count(device, ReportStatus::RolledBack));
        overall
    }
}

/// The ledger of a running server: in memory, and appended to a file when
/// it has one.
pub struct Ledger {
    file: Option<Mutex<std::fs::File>>,
    state: Mutex<State>,
}

impl Ledger {
    /// Replay `path` (if it exists) and keep appending to it. None keeps
    /// everything in memory only (tests, local runs).
    pub fn open(path: Option<&Path>) -> Result<Self> {
        let mut state = State::default();
        let Some(path) = path else {
            return Ok(Self { file: None, state: Mutex::new(state) });
        };
        if let Ok(f) = std::fs::File::open(path) {
            for line in std::io::BufReader::new(f).lines().map_while(Result::ok) {
                state.bytes += line.len() as u64 + 1;
                if let Ok(e) = serde_json::from_str::<Event>(&line) {
                    state.apply(&e);
                }
            }
        }
        let mut options = std::fs::OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let file = options.open(path).map_err(|e| anyhow!("open ledger {}: {e}", path.display()))?;
        Ok(Self { file: Some(Mutex::new(file)), state: Mutex::new(state) })
    }

    /// Write `e` and apply it, within `MAX_FILE_BYTES`.
    fn record(&self, state: &mut State, e: Event) -> std::result::Result<(), String> {
        let line = serde_json::to_string(&e).map_err(|e| e.to_string())?;
        if state.bytes + line.len() as u64 + 1 > MAX_FILE_BYTES {
            return Err("the ledger is full".into());
        }
        if let Some(file) = &self.file {
            let mut f = file.lock().map_err(|_| "ledger file unavailable")?;
            writeln!(f, "{line}").map_err(|e| format!("write ledger: {e}"))?;
        }
        state.bytes += line.len() as u64 + 1;
        state.apply(&e);
        Ok(())
    }

    /// Issue a new account: a random id, kept in the ledger.
    pub fn issue_account(&self) -> std::result::Result<String, String> {
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes).map_err(|e| format!("random source: {e}"))?;
        let user: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        let mut state = self.state.lock().map_err(|_| "ledger unavailable")?;
        self.record(&mut state, Event::Account { ts: now(), user: user.clone() })?;
        Ok(user)
    }

    /// The server issued this id.
    pub fn knows(&self, user: &str) -> bool {
        self.state.lock().map(|s| s.accounts.contains(user)).unwrap_or(false)
    }

    /// Record a report from an account the server issued. Returns its id and
    /// status: pending if the account is suspect, and possibly rolled back at
    /// once if other accounts already contradict it.
    pub fn add(&self, r: Report) -> std::result::Result<(u64, ReportStatus), String> {
        let mut state = self.state.lock().map_err(|_| "ledger unavailable")?;
        if !state.accounts.contains(&r.user) {
            return Err("unknown account: start a session with the server to get one".into());
        }
        let id = state.entries.len();
        self.record(&mut state, Event::Report(r))?;
        Ok((id as u64, state.entries[id].status))
    }

    /// Apply an administrator's decision on a report or an account.
    pub fn decide(&self, decision: Decision, report: Option<u64>, user: Option<&str>) -> std::result::Result<Value, String> {
        let mut state = self.state.lock().map_err(|_| "ledger unavailable")?;
        match (decision, report, user) {
            (Decision::Approve | Decision::Reject, Some(id), None) if (id as usize) < state.entries.len() => {}
            (Decision::Suspect | Decision::Clear, None, Some(u)) if state.accounts.contains(u) => {}
            _ => return Err("approve / reject take an existing report id; suspect / clear an existing account".into()),
        }
        self.record(&mut state, Event::Decision { ts: now(), decision, report, user: user.map(str::to_string) })?;
        Ok(json!({ "applied": true }))
    }

    /// Queue a driver file from an account; returns its id.
    pub fn contribute(&self, c: Contribution) -> std::result::Result<u64, String> {
        let mut state = self.state.lock().map_err(|_| "ledger unavailable")?;
        if !state.accounts.contains(&c.user) {
            return Err("unknown account: start a session with the server to get one".into());
        }
        let id = state.contributions.len() as u64;
        self.record(&mut state, Event::Contribution(c))?;
        Ok(id)
    }

    /// Where a contribution stands, for its author or anyone with its id.
    pub fn contribution(&self, id: u64) -> Option<Value> {
        let state = self.state.lock().ok()?;
        let (c, st, detail, pr) = state.contributions.get(id as usize)?;
        let held = *st == ContributionState::Pending && state.suspect.contains(&c.user);
        Some(json!({ "id": id, "path": c.path, "kind": c.kind, "state": if held { "held" } else {
            match st { ContributionState::Pending => "pending", ContributionState::Merged => "merged", ContributionState::Rejected => "rejected" }
        }, "detail": detail, "pr": pr, "submitted": c.ts }))
    }

    /// Contributions the merge job should take: pending, from accounts that
    /// are not suspect, latest per path.
    pub fn pending_contributions(&self) -> Vec<Contribution> {
        let Ok(state) = self.state.lock() else { return Vec::new() };
        let mut by_path: BTreeMap<&str, &Contribution> = BTreeMap::new();
        for (c, st, _, _) in &state.contributions {
            if *st == ContributionState::Pending && !state.suspect.contains(&c.user) {
                by_path.insert(c.path.as_str(), c);
            }
        }
        by_path.into_values().cloned().collect()
    }

    /// The merge job's verdict on a contribution.
    pub fn settle_contribution(&self, id: u64, st: ContributionState, detail: Option<String>, pr: Option<String>)
        -> std::result::Result<(), String> {
        let mut state = self.state.lock().map_err(|_| "ledger unavailable")?;
        if id as usize >= state.contributions.len() || st == ContributionState::Pending {
            return Err("unknown contribution, or not a final state".into());
        }
        self.record(&mut state, Event::ContributionStatus { ts: now(), id, state: st, detail, pr })
    }

    /// Record what an account saw a control send.
    pub fn observe(&self, o: Observation) -> std::result::Result<(), String> {
        let mut state = self.state.lock().map_err(|_| "ledger unavailable")?;
        if !state.accounts.contains(&o.user) {
            return Err("unknown account: start a session with the server to get one".into());
        }
        self.record(&mut state, Event::Observation(o))
    }

    /// Per control and firmware of one driver (all drivers with None): the
    /// observed messages, by how many accounts (latest observation of each,
    /// suspect accounts left out), and the learned one, if any.
    pub fn learned(&self, device: Option<&str>) -> Vec<Value> {
        let Ok(state) = self.state.lock() else { return Vec::new() };
        let mut latest: BTreeMap<(String, String, String), BTreeMap<String, &Observation>> = BTreeMap::new();
        for o in state.observations.iter().filter(|o| device.is_none_or(|d| o.device == d)) {
            if state.suspect.contains(&o.user) { continue; }
            latest.entry((o.device.clone(), o.control.clone(), o.firmware_key())).or_default().insert(o.user.clone(), o);
        }
        latest.into_iter().map(|((device, control, firmware), by_user)| {
            let mut counts: BTreeMap<&Sends, usize> = BTreeMap::new();
            for o in by_user.values() { *counts.entry(&o.sends).or_default() += 1; }
            let mut ranked: Vec<(&Sends, usize)> = counts.into_iter().collect();
            ranked.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
            let learned = match ranked.as_slice() {
                [(s, n), rest @ ..] if *n >= MIN_CONCORDANT && rest.first().is_none_or(|(_, m)| m < n) => Some((*s).clone()),
                _ => None,
            };
            json!({
                "device": device, "control": control, "firmware": firmware,
                "observed": ranked.iter().map(|(s, n)| json!({ "sends": s, "accounts": n })).collect::<Vec<_>>(),
                "learned": learned,
                "identified": by_user.values().any(|o| o.identity.is_some()),
            })
        }).collect()
    }

    /// What waits for an administrator: pending reports and suspect accounts.
    pub fn pending(&self) -> Value {
        let Ok(state) = self.state.lock() else { return json!(null) };
        let reports: Vec<&Report> = state.entries.iter().filter(|e| e.status == ReportStatus::Pending).map(|e| &e.report).collect();
        let mut suspect: Vec<&String> = state.suspect.iter().collect();
        suspect.sort();
        json!({ "pending_reports": reports, "suspect_accounts": suspect })
    }

    /// Status of one report.
    pub fn report_status(&self, id: u64) -> Option<ReportStatus> {
        self.state.lock().ok()?.entries.get(id as usize).map(|e| e.status)
    }

    /// Trust of one driver: overall, and per reported address.
    pub fn trust(&self, device: &str) -> Value {
        let Ok(state) = self.state.lock() else { return json!(null) };
        let addresses: BTreeMap<String, Value> = state.votes(device)
            .map(|v| v.by_address.iter().map(|(a, t)| (a.clone(), t.summary())).collect())
            .unwrap_or_default();
        json!({ "device": device, "overall": state.summary(device), "addresses": addresses })
    }

    /// Overall status of one driver.
    pub fn status(&self, device: &str) -> &'static str {
        self.state.lock().ok()
            .and_then(|s| s.votes(device).map(|v| v.overall.status().0.as_str()))
            .unwrap_or(Status::Unreported.as_str())
    }

    /// Overall trust of every reported driver.
    pub fn all(&self) -> Value {
        let Ok(state) = self.state.lock() else { return json!({}) };
        let devices: BTreeMap<&String, Value> = state.by_device.keys().map(|k| (k, state.summary(k))).collect();
        json!({ "min_concordant": MIN_CONCORDANT, "devices": devices })
    }
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(version: &str) -> Option<DeviceIdentity> {
        Some(DeviceIdentity { manufacturer: "42".into(), family: 0x2c, model: 1, version: version.into() })
    }

    fn report(user: &str, address: Option<&str>, outcome: Outcome, firmware: Option<&str>, identified: bool) -> Report {
        Report {
            id: 0,
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

    /// A ledger with `n` issued accounts, returned in order.
    fn ledger(n: usize) -> (Ledger, Vec<String>) {
        let l = Ledger::open(None).unwrap();
        let users = (0..n).map(|_| l.issue_account().unwrap()).collect();
        (l, users)
    }

    fn overall(l: &Ledger) -> Value {
        l.trust("korg/minilogue")["overall"].clone()
    }

    #[test]
    fn only_accounts_the_server_issued_can_report() {
        let (l, users) = ledger(1);
        assert!(l.knows(&users[0]));
        assert!(!l.knows(&"0".repeat(32)));
        let err = l.add(report(&"0".repeat(32), None, Outcome::Works, Some("2.1"), true)).unwrap_err();
        assert!(err.contains("unknown account"), "{err}");
        assert!(l.add(report(&users[0], None, Outcome::Works, Some("2.1"), true)).is_ok());
    }

    #[test]
    fn three_concordant_accounts_with_one_identified_device_verify_a_driver() {
        let (l, u) = ledger(3);
        assert_eq!(overall(&l)["status"], "unreported");
        l.add(report(&u[0], None, Outcome::Works, Some("2.1"), false)).unwrap();
        l.add(report(&u[1], None, Outcome::Works, Some("2.1"), false)).unwrap();
        assert_eq!(overall(&l)["status"], "unconfirmed");
        l.add(report(&u[2], None, Outcome::Works, Some("2.1"), false)).unwrap();
        assert_eq!(overall(&l)["status"], "unconfirmed", "no identified device yet");
        l.add(report(&u[2], None, Outcome::Works, Some("2.1"), true)).unwrap();
        assert_eq!(overall(&l)["status"], "usage-verified");
        assert_eq!(overall(&l)["works"], 3, "an account counts once");
        assert_eq!(overall(&l)["identities"]["2.1"][0], "42 002c 0001 02010000");
    }

    #[test]
    fn a_failure_on_another_firmware_does_not_demote() {
        let (l, u) = ledger(4);
        for x in &u[..3] {
            l.add(report(x, None, Outcome::Works, Some("2.1"), true)).unwrap();
        }
        l.add(report(&u[3], Some("/minilogue/filter/cutoff"), Outcome::Fails, Some("1.0"), false)).unwrap();
        let o = overall(&l);
        assert_eq!(o["status"], "firmware-dependent");
        assert_eq!(o["firmwares"]["2.1"]["status"], "usage-verified");
        assert_eq!(o["firmwares"]["1.0"]["status"], "failing");
        assert_eq!(o["needs_firmware"], false);
    }

    #[test]
    fn a_report_three_accounts_contradict_is_rolled_back_and_its_account_held() {
        let (l, u) = ledger(4);
        let liar = &u[3];
        // The liar's other report, elsewhere, counts for now.
        let mut other = report(liar, None, Outcome::Works, Some("1.0"), true);
        other.device = "moog/matriarch".into();
        let (other_id, _) = l.add(other).unwrap();
        for x in &u[..3] {
            l.add(report(x, None, Outcome::Works, Some("2.1"), true)).unwrap();
        }
        assert_eq!(overall(&l)["status"], "usage-verified");
        let (bad, status) = l.add(report(liar, None, Outcome::Fails, Some("2.1"), false)).unwrap();
        assert_eq!(status, ReportStatus::RolledBack, "the trust returns to what it was");
        assert_eq!(overall(&l)["status"], "usage-verified");
        assert_eq!(overall(&l)["rolled_back"], 1);
        assert_eq!(l.report_status(other_id), Some(ReportStatus::Pending), "its other reports wait for approval");
        assert_eq!(l.trust("moog/matriarch")["overall"]["status"], "unreported");
        // New reports of the suspect account wait too.
        let (_, next) = l.add(report(liar, Some("/minilogue/filter/cutoff"), Outcome::Works, Some("2.1"), true)).unwrap();
        assert_eq!(next, ReportStatus::Pending);
        let pending = l.pending();
        assert_eq!(pending["suspect_accounts"][0], liar.as_str());
        assert_eq!(pending["pending_reports"].as_array().unwrap().len(), 2);
        // An administrator clears the account: its held reports count again.
        l.decide(Decision::Clear, None, Some(liar)).unwrap();
        assert_eq!(l.report_status(other_id), Some(ReportStatus::Accepted));
        assert_eq!(l.report_status(bad), Some(ReportStatus::RolledBack), "a rolled-back report stays out");
    }

    #[test]
    fn a_lone_contradiction_disputes_without_rollback_and_approval_protects_a_report() {
        let (l, u) = ledger(4);
        for x in &u[..2] {
            l.add(report(x, None, Outcome::Works, Some("2.1"), true)).unwrap();
        }
        let (lone, status) = l.add(report(&u[2], None, Outcome::Fails, Some("2.1"), false)).unwrap();
        assert_eq!(status, ReportStatus::Accepted, "two against one is not enough to roll back");
        assert_eq!(overall(&l)["status"], "disputed");
        l.decide(Decision::Approve, Some(lone), None).unwrap();
        l.add(report(&u[3], None, Outcome::Works, Some("2.1"), true)).unwrap();
        assert_eq!(l.report_status(lone), Some(ReportStatus::Accepted), "approved reports are not rolled back");
        assert_eq!(overall(&l)["status"], "disputed");
        l.decide(Decision::Reject, Some(lone), None).unwrap();
        assert_eq!(overall(&l)["status"], "usage-verified");
    }

    #[test]
    fn a_failure_of_unknown_firmware_disputes_and_asks_to_learn_it() {
        let (l, u) = ledger(4);
        for x in &u[..3] {
            l.add(report(x, None, Outcome::Works, Some("2.1"), true)).unwrap();
        }
        l.add(report(&u[3], None, Outcome::Fails, None, false)).unwrap();
        let o = overall(&l);
        assert_eq!(o["status"], "disputed");
        assert_eq!(o["needs_firmware"], true);
        assert!(o["learn"].as_str().unwrap().contains("F0 7E 7F 06 01 F7"));
        // The same account comes back with its device identified: the failure
        // is placed on its firmware and is no longer a contradiction.
        let mut placed = report(&u[3], None, Outcome::Fails, None, true);
        placed.identity = identity("01000000");
        l.add(placed).unwrap();
        let o = overall(&l);
        assert_eq!(o["needs_firmware"], false);
        assert_eq!(o["status"], "firmware-dependent");
        assert_eq!(o["firmwares"]["identity 01000000"]["status"], "failing");
    }

    #[test]
    fn three_concordant_observations_teach_a_mapping_and_suspects_do_not_count() {
        let (l, u) = ledger(5);
        let obs = |user: &str, cc: u8| Observation {
            ts: "2026-10-22T12:00:00Z".into(), user: user.into(), device: "korg/minilogue".into(),
            control: "/minilogue/filter/cutoff".into(), sends: Sends::Cc { cc }, channel: Some(1),
            firmware: Some("2.1".into()), identity: None,
        };
        l.observe(obs(&u[0], 43)).unwrap();
        l.observe(obs(&u[1], 43)).unwrap();
        l.observe(obs(&u[2], 74)).unwrap();
        assert_eq!(l.learned(Some("korg/minilogue"))[0]["learned"], Value::Null, "two agreeing is not enough");
        l.observe(obs(&u[3], 43)).unwrap();
        let learned = l.learned(None);
        assert_eq!(learned[0]["learned"], json!({"kind": "cc", "cc": 43}));
        assert_eq!(learned[0]["observed"][0]["accounts"], 3);
        l.decide(Decision::Suspect, None, Some(&u[3])).unwrap();
        assert_eq!(l.learned(None)[0]["learned"], Value::Null, "a suspect account's observations do not count");
        assert!(l.observe(obs(&"f".repeat(32), 43)).is_err(), "only issued accounts observe");
    }

    #[test]
    fn contributions_wait_for_the_merge_job_and_suspects_are_held() {
        let (l, u) = ledger(2);
        let c = |user: &str, path: &str| Contribution {
            id: 0, ts: "2026-10-22T12:00:00Z".into(), user: user.into(), path: path.into(),
            content: "{}".into(), kind: "driver".into(), note: None,
        };
        let a = l.contribute(c(&u[0], "devices/acme/box.agent-contributed.fw-unknown.json")).unwrap();
        let b = l.contribute(c(&u[1], "devices/acme/other.agent-contributed.fw-unknown.json")).unwrap();
        assert_eq!(l.pending_contributions().len(), 2);
        l.decide(Decision::Suspect, None, Some(&u[1])).unwrap();
        assert_eq!(l.contribution(b).unwrap()["state"], "held");
        assert_eq!(l.pending_contributions().len(), 1);
        l.settle_contribution(a, ContributionState::Merged, None, Some("https://github.com/x/y/pull/1".into())).unwrap();
        assert_eq!(l.contribution(a).unwrap()["state"], "merged");
        assert!(l.pending_contributions().is_empty());
        assert!(l.settle_contribution(a, ContributionState::Pending, None, None).is_err());
    }

    #[test]
    fn the_ledger_replays_to_the_same_state() {
        let path = std::env::temp_dir().join(format!("osc-bridge-ledger-{}.jsonl", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let (liar, rolled) = {
            let l = Ledger::open(Some(&path)).unwrap();
            let u: Vec<String> = (0..4).map(|_| l.issue_account().unwrap()).collect();
            for x in &u[..3] {
                l.add(report(x, None, Outcome::Works, Some("2.1"), true)).unwrap();
            }
            let (id, _) = l.add(report(&u[3], None, Outcome::Fails, Some("2.1"), false)).unwrap();
            (u[3].clone(), id)
        };
        let l = Ledger::open(Some(&path)).unwrap();
        assert!(l.knows(&liar));
        assert_eq!(overall(&l)["status"], "usage-verified");
        assert_eq!(l.report_status(rolled), Some(ReportStatus::RolledBack));
        assert_eq!(l.pending()["suspect_accounts"][0], liar.as_str());
        std::fs::remove_file(path).ok();
    }
}
