//! A driver as the engine holds it: the `osd::Device` that `osd` resolves from
//! a catalogue folder, plus what only the engine knows (the companion note, the
//! rules its blocks must meet to be used). The format, its types and the
//! resolution of shared MIDI and maker profiles belong to `osd`.

use anyhow::{anyhow, bail, Context, Result};
use osd::{Device, FramePart, FrameToken, IdentityPart};
use std::path::{Path, PathBuf};

/// The key under which the companion note travels in a device's remarks
/// (`_` keys are notes for people that `osd` keeps as written).
const DOCS_KEY: &str = "_docs";

/// A catalogue folder read by `osd`: the file of id `maker/name` is
/// `<root>/maker/name.json`, a profile `maker/family` the file
/// `<root>/maker/_profile.family.json`.
pub struct Folder(pub PathBuf);

impl osd::Files for Folder {
    fn get(&self, id: &str) -> Option<String> {
        let (maker, name) = id.split_once('/')?;
        let ok = |s: &str| !s.is_empty() && !s.starts_with('.') && !s.contains(['/', '\\']);
        if !ok(maker) || !ok(name) {
            return None;
        }
        std::fs::read_to_string(self.0.join(maker).join(format!("{name}.json"))).ok()
    }
}

/// Where a driver file sits in its catalogue: the folder holding the maker
/// folders, and the file's catalogue id (`maker/name`).
fn place(path: &Path) -> Result<(PathBuf, String)> {
    let abs = std::path::absolute(path).with_context(|| format!("{}", path.display()))?;
    let stem = abs.file_stem().and_then(|s| s.to_str()).ok_or_else(|| anyhow!("{}: not a driver file", path.display()))?;
    let maker_dir = abs.parent().ok_or_else(|| anyhow!("{}: no catalogue folder", path.display()))?;
    let maker = maker_dir.file_name().and_then(|s| s.to_str()).ok_or_else(|| anyhow!("{}: no catalogue folder", path.display()))?;
    let root = maker_dir.parent().ok_or_else(|| anyhow!("{}: no catalogue folder", path.display()))?;
    Ok((root.to_path_buf(), format!("{maker}/{stem}")))
}

/// The driver at `path` as its firmware runs it: `osd` reads it, follows its
/// shared MIDI and completes its blocks from the maker's profiles, in the
/// catalogue folder around it. A driver `osd` refuses fails with `osd`'s
/// fault; one the engine cannot use fails naming the defect.
pub fn load<P: AsRef<Path>>(path: P) -> Result<Device> {
    let p = path.as_ref();
    let (root, id) = place(p)?;
    if !p.is_file() {
        bail!("reading {}: no such file", p.display());
    }
    let resolved = osd::resolve(&id, &Folder(root)).map_err(anyhow::Error::from).with_context(|| p.display().to_string())?;
    check_blocks(&resolved.device).map_err(|e| anyhow!("{}: {e}", p.display()))?;
    Ok(resolved.device)
}

/// The driver file at `path` as written, none of its links followed: what the
/// file says of itself. None when `osd` does not read it as a driver.
pub fn read_own<P: AsRef<Path>>(path: P) -> Option<Device> {
    let text = std::fs::read_to_string(path).ok()?;
    match osd::read(&text, osd::Kind::Device).ok()? {
        osd::Descriptor::Device(d) => Some(*d),
        _ => None,
    }
}

/// The word a device kind is written with (`hardware` when none is stated).
pub fn kind_name(kind: Option<osd::DeviceKind>) -> &'static str {
    match kind {
        None | Some(osd::DeviceKind::Hardware) => "hardware",
        Some(osd::DeviceKind::Software) => "software",
    }
}

/// The word a command argument's type is written with.
pub fn arg_type_name(ty: osd::ArgType) -> &'static str {
    use osd::ArgType::*;
    match ty {
        U7 => "u7",
        U14 => "u14",
        S32 => "s32",
        Int => "int",
        Float => "float",
        Bool => "bool",
        String => "string",
        Enum => "enum",
    }
}

/// The word a program dump kind is written with.
pub fn dump_kind_name(kind: osd::DumpKind) -> &'static str {
    match kind {
        osd::DumpKind::None => "none",
        osd::DumpKind::Request => "request",
        osd::DumpKind::EditBuffer => "edit_buffer",
        osd::DumpKind::Manual => "manual",
    }
}

/// The word a trigger's channel mode is written with (`fixed` when none is stated).
pub fn channel_mode_name(mode: Option<osd::ChannelMode>) -> &'static str {
    match mode {
        None | Some(osd::ChannelMode::Fixed) => "fixed",
        Some(osd::ChannelMode::Basic) => "basic",
        Some(osd::ChannelMode::Omni) => "omni",
        Some(osd::ChannelMode::Auto) => "auto",
    }
}

/// What a reply is reported with, as one text: the address, then each
/// argument (`{name}` for an argument of the message or a capture).
pub fn emit_text(r: &osd::Reply) -> String {
    let mut out = address_text(&r.emit_osc);
    for a in r.emit_args.iter().flatten() {
        out.push(' ');
        match a {
            osd::Scalar::Arg(n) | osd::Scalar::Capture(n) => out.push_str(&format!("{{{n}}}")),
            osd::Scalar::Number(n) => out.push_str(&n.to_string()),
            osd::Scalar::Bool(b) => out.push_str(&b.to_string()),
            osd::Scalar::String(s) => out.push_str(s),
        }
    }
    out
}

/// The note `<stem>.md` beside the driver file, when there is one: an
/// integration guide for clients and agents, never read by `osd`.
pub fn docs_of<P: AsRef<Path>>(path: P) -> Option<String> {
    std::fs::read_to_string(path.as_ref().with_extension("md")).ok()
}

/// The driver at `path` carrying its companion note in its remarks, for the
/// runtime that serves it.
pub fn load_with_docs<P: AsRef<Path>>(path: P) -> Result<Device> {
    let mut dev = load(path.as_ref())?;
    if let Some(docs) = docs_of(path.as_ref()) {
        dev.remarks.insert(DOCS_KEY.into(), serde_json::Value::String(docs));
    }
    Ok(dev)
}

/// The companion note a device was loaded with.
pub fn attached_docs(dev: &Device) -> Option<String> {
    dev.remarks.get(DOCS_KEY).and_then(|v| v.as_str()).map(str::to_string)
}

/// Prepend the device `osc_prefix` to a relative OSC path (starting with `/`).
/// If the path already starts with the prefix it is returned unchanged.
pub fn full_osc_path(dev: &Device, rel: &str) -> String {
    let prefix = &dev.device.osc_prefix;
    if rel.starts_with(prefix.as_str()) {
        rel.to_string()
    } else if rel.starts_with('/') {
        format!("{prefix}{rel}")
    } else {
        format!("{prefix}/{rel}")
    }
}

/// The text of an OSC address that holds no capture; a capture is written
/// `{name}`.
pub fn address_text(a: &osd::Address) -> String {
    match a {
        osd::Address::Text(t) => t.clone(),
        osd::Address::Segments(s) => s
            .iter()
            .map(|p| match p {
                osd::Segment::Text(t) => t.clone(),
                osd::Segment::Capture(n) => format!("{{{n}}}"),
            })
            .collect(),
    }
}

/// A thing a driver names that the engine cannot use: the driver, the
/// field, and the cause (the text of the error that refused it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceFault {
    pub device: String,
    pub field: String,
    pub cause: String,
}

/// The faults `dev` carries from loading: an `identity.request` that
/// `probe_of` cannot build. Nothing refuses the driver for them; `lint` and
/// `inspect` list them, and detection says when it fell back on the universal request.
pub fn faults(dev: &Device) -> Vec<DeviceFault> {
    dev.identity.as_ref().and_then(|i| crate::detect::probe_of(i).err())
        .map(|e| DeviceFault { device: dev.device.name.clone(), field: "identity.request".into(), cause: e.to_string() })
        .into_iter().collect()
}

/// The value a block key holds once resolved: `osd` reads an optional key of
/// `identity` or `programs` as absent, `null` (a key of the maker profile
/// dropped) or a value; the engine uses a value, so absent and `null` are one.
pub fn kept<T>(key: &Option<Option<T>>) -> Option<&T> {
    key.as_ref().and_then(Option::as_ref)
}

/// `kept` for a key that holds a list, read as a slice.
pub fn kept_list<T>(key: &Option<Option<Vec<T>>>) -> &[T] {
    kept(key).map_or(&[], Vec::as_slice)
}

/// The checksums the engine cannot compute, as `(place, problem)`: the place
/// is the OSC address of a command or parameter, or the block (`programs`,
/// `identity.request`). Nothing refuses a driver for them; `lint` and
/// `inspect` warn, the build of that frame fails.
pub fn checksum_problems(dev: &Device) -> Vec<(String, String)> {
    use crate::frame::checksum_problems as check;
    let mut out = Vec::new();
    let mut add = |place: String, frame: &[FrameToken], base: usize| {
        out.extend(check(frame, base).into_iter().map(|p| (place.clone(), p)));
    };
    // Commands and parameters sit after the header; identity and programs frames are whole messages.
    let head = dev.sysex.as_ref().and_then(|s| s.header.as_ref()).map_or(0, Vec::len);
    for c in dev.commands.iter().flatten() {
        add(full_osc_path(dev, &address_text(&c.osc)), c.frame.as_deref().unwrap_or_default(), head);
    }
    if let Some(pt) = &dev.params {
        let set = pt.entries.first().map(|e| full_osc_path(dev, &e.osc)).unwrap_or_else(|| "param set".into());
        add(set, pt.set_frame.as_deref().unwrap_or_default(), head);
        add(full_osc_path(dev, "/param/get"), pt.get_frame.as_deref().unwrap_or_default(), head);
    }
    if let Some(req) = dev.identity.as_ref().and_then(|i| kept(&i.request)) { add("identity.request".into(), req, 0); }
    if let Some(p) = &dev.programs {
        for f in program_frames(p) {
            add("programs".into(), f, 0);
        }
    }
    out
}

/// The frames a `programs` block sends: `before`, `after` and the dump request.
pub fn program_frames(p: &osd::Programs) -> impl Iterator<Item = &Vec<FrameToken>> {
    kept_list(&p.before).iter()
        .chain(kept_list(&p.after))
        .chain(kept(&p.dump).and_then(|d| d.request.as_ref()))
}

/// Refuse an `identity` or `programs` block the engine could not use: what
/// depends on its own notions (the bytes it builds, the program changes it
/// sends). What the OSD specification says is `osd`'s refusal.
pub fn check_blocks(dev: &Device) -> Result<()> {
    let names = |frame: &[FrameToken]| -> Vec<String> {
        frame
            .iter()
            .filter_map(|t| match t {
                FrameToken::Part(FramePart::Arg { name, .. }) => Some(name.clone()),
                _ => None,
            })
            .collect()
    };
    let has = |frame: &[FrameToken], f: fn(&FramePart) -> bool| frame.iter().any(|t| matches!(t, FrameToken::Part(p) if f(p)));
    let uses_dev = |frame: &[FrameToken]| has(frame, |p| matches!(p, FramePart::DeviceId));
    let uses_family = |frame: &[FrameToken]| has(frame, |p| matches!(p, FramePart::Identity { part: IdentityPart::Family, .. }));
    let uses_member = |frame: &[FrameToken]| has(frame, |p| matches!(p, FramePart::Identity { part: IdentityPart::Member, .. }));
    if let Some(id) = &dev.identity {
        let ports = !kept_list(&id.ports).is_empty();
        if kept(&id.to) == Some(&osd::IdentityTo::NamedPorts) && !ports {
            bail!("identity.to named_ports needs ports");
        }
        if let Some(req) = kept(&id.request) {
            if req.first() != Some(&FrameToken::Byte(0xF0)) || req.last() != Some(&FrameToken::Byte(0xF7)) {
                bail!("identity.request runs from F0 to F7");
            }
            if let Some(n) = names(req).into_iter().next() {
                bail!("identity.request names the argument {n}: only device_id, identity and checksum elements");
            }
            if uses_dev(req) && kept(&id.device_id).is_none() {
                bail!("identity.request holds {{\"device_id\": true}}: give identity.device_id");
            }
        }
        if let Some(fw) = kept(&id.firmware) {
            if fw.decode != "lua" {
                let want = match fw.decode.as_str() {
                    "korg_minor_major" => Some(4),
                    "yamaha_tenths" | "ascii" | "json" => Some(1),
                    _ => None,
                };
                if fw.at.is_empty() || want.is_some_and(|n| fw.at.len() != n) {
                    bail!("identity.firmware.at: wrong number of offsets for {}", fw.decode);
                }
            }
            if kept_list(&id.reply).is_empty() && kept(&id.script).is_none() {
                bail!("identity.firmware without a reply");
            }
        }
        if kept_list(&id.reply).is_empty() && kept(&id.script).is_none() && !ports {
            bail!("identity: a reply, a script or ports");
        }
        let frames: Vec<FrameToken> = kept_list(&id.reply).iter().chain(kept_list(&id.request)).cloned().collect();
        let family_empty = kept_list(&id.family).is_empty();
        let member_empty = kept_list(&id.member).is_empty();
        if uses_family(&frames) && family_empty {
            bail!("identity names the family: give identity.family");
        }
        if uses_member(&frames) && member_empty {
            bail!("identity names the member: give identity.member");
        }
    }
    if let Some(p) = &dev.programs {
        let identity = dev.identity.as_ref();
        for f in program_frames(p) {
            if uses_dev(f) && identity.and_then(|i| kept(&i.device_id)).is_none() {
                bail!("programs: a frame holds {{\"device_id\": true}}: give identity.device_id");
            }
            if uses_family(f) && identity.is_none_or(|i| kept_list(&i.family).is_empty()) {
                bail!("programs: a frame names the family: give identity.family");
            }
            if let Some(n) = names(f).into_iter().find(|n| !crate::programs::PLACEHOLDERS.contains(&n.as_str())) {
                bail!("programs: a frame names the argument {n}: one of {:?}", crate::programs::PLACEHOLDERS);
            }
        }
        let program_change = kept(&p.program_change).copied().unwrap_or(true);
        let no_message = kept_list(&p.before).is_empty() && kept_list(&p.after).is_empty();
        if !program_change && no_message && kept(&p.recall_script).is_none() {
            bail!("programs: no program change and no message to recall");
        }
        let mut seen = std::collections::HashSet::new();
        for b in kept_list(&p.banks) {
            if !seen.insert(b.name.as_str()) {
                bail!("programs: bank {:?} twice", b.name);
            }
            if program_change && kept(&p.recall_script).is_none() && b.first_pc.unwrap_or(0) as i64 + b.size > 128 {
                bail!("programs: bank {:?} runs past program change 127", b.name);
            }
        }
    }
    Ok(())
}
