//! The tools that set and read a control by its role (`filter/cutoff`): `list_roles`,
//! `set_by_role`, `get_by_role`. A role is the OSD path of a control; it resolves to the control's
//! address through `osd`, and the value an agent gives, from 0 to 1, is scaled on the range and
//! steps `osd` gives for that control of that driver.

use super::{content_structured, send_osc_message, DeviceIndex, SEND_ANSWER_GUARD};
use serde_json::{json, Value};
use std::net::{SocketAddr, UdpSocket};

/// One control that carries a role, as `osd` reads it.
struct Control {
    role: String,
    address: String,
    range: [i64; 2],
    default: Option<i64>,
    step_count: Option<i64>,
    priority: u8,
    /// The `pr p c r` numbers of a SysEx parameter: its message writes the device's memory.
    sysex: Option<[u8; 4]>,
    /// Whether the driver declares a read request and a reply pattern for this control.
    reads: bool,
}

/// The text of a role: its parts (the number of a part as a segment after it), then its setting.
fn role_text(r: &osd::Role) -> String {
    let mut out: Vec<String> = Vec::new();
    for p in &r.parts {
        out.push(p.part.clone());
        if let Some(i) = p.index { out.push(i.to_string()); }
    }
    out.push(r.setting.clone());
    out.join("/")
}

/// Every control of the driver that carries a role, most important first (priority 1 is the most
/// important; a control without priority comes last; equal priorities keep the driver's order).
fn controls(dev: &osd::Device) -> Vec<Control> {
    let prefix = &dev.device.osc_prefix;
    let mut out = Vec::new();
    for e in dev.cc_params.iter().flat_map(|t| &t.entries) {
        let Some(role) = &e.role else { continue };
        out.push(Control {
            role: role_text(role), address: format!("{prefix}{}", e.osc), range: e.range.unwrap_or([0, 127]),
            default: e.default, step_count: e.step_count, priority: e.priority.unwrap_or(u8::MAX),
            sysex: None, reads: false,
        });
    }
    if let Some(t) = &dev.params {
        for e in &t.entries {
            let Some(role) = &e.role else { continue };
            let address = format!("{prefix}{}", e.osc);
            let reads = !t.get_frame.is_empty()
                && dev.replies.iter().flatten().any(|r| crate::device::address_text(&r.emit_osc) == address);
            out.push(Control {
                role: role_text(role), address, range: e.range.unwrap_or([0, 127]),
                default: e.default, step_count: e.step_count, priority: e.priority.unwrap_or(u8::MAX),
                sysex: Some([e.pr.unwrap_or(0), e.p.unwrap_or(0), e.c.unwrap_or(0), e.r.unwrap_or(0)]),
                reads,
            });
        }
    }
    out.sort_by_key(|c| c.priority);
    out
}

/// The id of the driver an agent named, its OSC prefix and its controls with a role.
fn device_controls(index: &DeviceIndex, args: &Value) -> Result<(String, String, Vec<Control>), String> {
    let wanted = args.get("device").and_then(Value::as_str).ok_or("missing device")?;
    let entry = index.find(wanted).ok_or_else(|| format!("no driver '{wanted}' (see list_devices)"))?;
    let dev = crate::device::load(&entry.path).map_err(|e| format!("load {}: {e}", entry.path.display()))?;
    Ok((entry.id.clone(), dev.device.osc_prefix.clone(), controls(&dev)))
}

fn control_json(c: &Control) -> Value {
    let mut o = json!({ "role": c.role, "address": c.address, "range": c.range, "priority": c.priority });
    if let Some(d) = c.default { o["default"] = json!(d); }
    if let Some(s) = c.step_count { o["step_count"] = json!(s); }
    o
}

pub(super) fn list_roles(index: &DeviceIndex, args: &Value) -> Result<Value, String> {
    let (id, _, all) = device_controls(index, args)?;
    let limit = args.get("limit").and_then(Value::as_u64).map_or(usize::MAX, |l| l as usize);
    let roles: Vec<Value> = all.iter().take(limit).map(control_json).collect();
    let structured = json!({ "device": id, "roles": roles });
    let text = if roles.is_empty() {
        format!("{id} declares no role.")
    } else {
        serde_json::to_string_pretty(&structured).unwrap_or_default()
    };
    Ok(content_structured(&text, structured))
}

/// The control of `role`: the one, or an error result that names the role and its nearest roles
/// (the longest common path first, then the priority), or the two addresses that carry it.
fn find<'a>(id: &str, all: &'a [Control], role: &str) -> Result<&'a Control, Result<Value, String>> {
    let hits: Vec<&Control> = all.iter().filter(|c| c.role == role).collect();
    match hits.as_slice() {
        [one] => Ok(one),
        [] => {
            let wanted: Vec<&str> = role.split('/').collect();
            let common = |c: &Control| c.role.split('/').zip(&wanted).take_while(|(a, b)| a == *b).count();
            let mut near: Vec<&Control> = all.iter().collect();
            near.sort_by_key(|c| std::cmp::Reverse(common(c)));
            let nearest: Vec<&str> = near.iter().take(8).map(|c| c.role.as_str()).collect();
            let text = format!("{id} has no role '{role}'. Nearest roles: {}.", nearest.join(", "));
            let mut res = content_structured(&text, json!({ "role": role, "nearest": nearest }));
            res["isError"] = json!(true);
            Err(Ok(res))
        }
        many => Err(Err(format!("{id}: the role '{role}' is carried by {} controls: {}. Nothing sent.",
            many.len(), many.iter().map(|c| c.address.as_str()).collect::<Vec<_>>().join(" and ")))),
    }
}

/// The raw value of `value` (0 to 1) on the range of the control, snapped to its steps.
fn scale(c: &Control, value: f64) -> i64 {
    let [lo, hi] = c.range;
    let span = (hi - lo) as f64;
    match c.step_count {
        Some(n) if n >= 2 => {
            let k = (value * (n - 1) as f64).round();
            lo + (k * span / (n - 1) as f64).round() as i64
        }
        _ => lo + (value * span).round() as i64,
    }
}

pub(super) fn set_by_role(index: &DeviceIndex, args: &Value, target: SocketAddr) -> Result<Value, String> {
    let role = args.get("role").and_then(Value::as_str).ok_or("missing role")?;
    let value = args.get("value").and_then(Value::as_f64).ok_or("missing value (a number from 0 to 1)")?;
    if !(0.0..=1.0).contains(&value) {
        return Err(format!("value {value} is outside 0 to 1: give the value normalised on the range of the role."));
    }
    let (id, _, all) = device_controls(index, args)?;
    let c = match find(&id, &all, role) { Ok(c) => c, Err(done) => return done };
    if c.sysex.is_some() {
        return Err(format!("the role '{role}' of {id} resolves to {}, a message that writes the device's memory: \
            set_by_role never does. Use restore_presets, which asks the person's consent (confirm_overwrite).", c.address));
    }
    send_osc_message(&c.address, &json!([scale(c, value)]), target)
}

pub(super) fn get_by_role(index: &DeviceIndex, args: &Value, target: SocketAddr) -> Result<Value, String> {
    let role = args.get("role").and_then(Value::as_str).ok_or("missing role")?;
    let (id, prefix, all) = device_controls(index, args)?;
    let c = match find(&id, &all, role) { Ok(c) => c, Err(done) => return done };
    let Some(numbers) = c.sysex.filter(|_| c.reads) else {
        return Err(format!("{id} does not report the value of '{role}': its driver declares no read request \
            and reply pattern for it."));
    };
    let request = format!("{prefix}/param/get");
    use rosc::{decoder, encoder, OscMessage, OscPacket, OscType};
    let msg = OscPacket::Message(OscMessage { addr: request.clone(),
        args: numbers.iter().map(|n| OscType::Int(*n as i32)).collect() });
    let bytes = encoder::encode(&msg).map_err(|e| format!("osc encode: {e:?}"))?;
    let sock = UdpSocket::bind("0.0.0.0:0").map_err(|e| e.to_string())?;
    sock.send_to(&bytes, target).map_err(|e| e.to_string())?;
    let deadline = std::time::Instant::now() + SEND_ANSWER_GUARD;
    let mut buf = [0u8; 65536];
    loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        let silent = || format!("{id} did not report '{role}' ({} asked at {target}).", request);
        if left.is_zero() { return Err(silent()); }
        sock.set_read_timeout(Some(left)).map_err(|e| e.to_string())?;
        let Ok((n, _)) = sock.recv_from(&mut buf) else { return Err(silent()); };
        let Ok((_, pkt)) = decoder::decode_udp(&buf[..n]) else { continue };
        let mut got = Vec::new();
        super::flatten_messages(pkt, &mut got);
        for m in got.into_iter().filter(|m| m.addr == c.address) {
            let raw = m.args.iter().find_map(|a| match a {
                OscType::Int(i) => Some(*i as f64),
                OscType::Long(i) => Some(*i as f64),
                OscType::Float(f) => Some(*f as f64),
                OscType::Double(d) => Some(*d),
                _ => None,
            });
            let Some(raw) = raw else { continue };
            let [lo, hi] = c.range;
            let value = if hi == lo { 0.0 } else { ((raw - lo as f64) / (hi - lo) as f64).clamp(0.0, 1.0) };
            let structured = json!({ "device": id, "role": role, "address": c.address, "value": value, "raw": raw });
            return Ok(content_structured(&format!("{role} of {id} is {value:.4} (raw {raw})."), structured));
        }
    }
}
