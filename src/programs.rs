//! Program recall: `/<prefix>/program/recall <bank> <number> [channel]`
//! sends what the driver's `programs` block says recalls a stored program —
//! its `before` messages, the bank select, the program change, its `after`
//! messages. The bank is named, or given by its place in `banks` (from 0);
//! the number is the one the device shows (from the bank's `first_number`).

use crate::device::{Bank, Device, FrameToken, Programs};
use crate::frame::{build_message, Bindings};
use rosc::OscType;

/// The route, after the device's prefix.
pub const RECALL: &str = "/program/recall";

/// A bank of `p` by its name (any case) or its place in the list.
pub fn find_bank<'a>(p: &'a Programs, bank: &str) -> Option<(usize, &'a Bank)> {
    p.banks.iter().enumerate().find(|(_, b)| b.name.eq_ignore_ascii_case(bank))
        .or_else(|| bank.parse::<usize>().ok().and_then(|i| p.banks.get(i).map(|b| (i, b))))
}

/// The MIDI messages that recall program `number` of `bank`, on `channel`
/// (0-15; else the bank's, else `midi_out.default_channel`).
pub fn recall(dev: &Device, bank: &str, number: i64, channel: Option<u8>) -> Result<Vec<Vec<u8>>, String> {
    let p = dev.programs.as_ref().ok_or("this driver says nothing of its programs")?;
    if p.banks.is_empty() {
        return Err("this driver lists no banks".into());
    }
    let (bank_index, b) = find_bank(p, bank).ok_or_else(|| {
        let names: Vec<&str> = p.banks.iter().map(|b| b.name.as_str()).collect();
        format!("no bank {bank:?}: {}", names.join(", "))
    })?;
    let first = b.first_number as i64;
    let last = first + b.size as i64 - 1;
    if !(first..=last).contains(&number) {
        return Err(format!("bank {} holds programs {first}..={last}", b.name));
    }
    let offset = (number - first) as u16;
    let ch = channel.or(b.channel).or(dev.midi_out.as_ref().map(|m| m.default_channel)).unwrap_or(0) & 0x0F;
    let pc = (b.first_pc as u16 + offset).min(127) as u8;
    let index = p.banks[..bank_index].iter().map(|b| b.size).sum::<u16>() + offset;
    let mut bind = Bindings::new();
    bind.set_u7("channel", ch);
    bind.set_u7("msb", b.msb.unwrap_or(0));
    bind.set_u7("lsb", b.lsb.unwrap_or(0));
    bind.set_u7("bank", bank_index as u8);
    bind.set_u7("program", pc);
    bind.set_u14("index", index);
    bind.set_u14("number", number as u16);
    if let Some(d) = dev.identity.as_ref().and_then(|i| i.device_id.as_ref()) {
        bind.set_u7("dev", d.default);
    }
    let frames = |list: &[Vec<FrameToken>]| -> Result<Vec<Vec<u8>>, String> {
        list.iter().map(|f| build_message(f, &bind).map_err(|e| e.to_string())).collect()
    };
    let mut out = frames(&p.before)?;
    if p.program_change {
        if let Some(msb) = b.msb { out.push(vec![0xB0 | ch, 0, msb & 0x7F]); }
        if let Some(lsb) = b.lsb { out.push(vec![0xB0 | ch, 32, lsb & 0x7F]); }
        out.push(vec![0xC0 | ch, pc & 0x7F]);
    }
    out.extend(frames(&p.after)?);
    Ok(out)
}

/// The recall an OSC message asks for, or None when it is not this
/// device's recall route.
pub fn recall_osc(dev: &Device, addr: &str, args: &[OscType]) -> Option<Result<Vec<Vec<u8>>, String>> {
    dev.programs.as_ref()?;
    if addr != dev.full_osc_path(RECALL) {
        return None;
    }
    let int = |v: &OscType| match v {
        OscType::Int(i) => Some(*i as i64),
        OscType::Long(i) => Some(*i),
        OscType::Float(f) => Some(*f as i64),
        OscType::Double(f) => Some(*f as i64),
        _ => None,
    };
    let bank = match args.first() {
        Some(OscType::String(s)) => s.clone(),
        Some(v) => match int(v) { Some(i) => i.to_string(), None => return Some(Err("bank: a name or an index".into())) },
        None => return Some(Err(format!("{RECALL} <bank> <number> [channel]"))),
    };
    let Some(number) = args.get(1).and_then(int) else { return Some(Err("number: the program's number".into())) };
    let channel = args.get(2).and_then(int).map(|c| c.clamp(0, 15) as u8);
    Some(recall(dev, &bank, number, channel))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(programs: &str) -> Device {
        serde_json::from_str(&format!(r#"{{"device":{{"name":"Box","vendor":"Acme","osc_prefix":"/box"}},
            "midi_out":{{"default_channel":2}},"identity":{{"device_id":{{"default":16}},"reply":[240,126]}},
            "programs":{programs}}}"#)).unwrap()
    }

    #[test]
    fn a_bank_and_a_number_become_the_bank_select_and_the_program_change() {
        let d = dev(r#"{"banks":[{"name":"User","msb":81,"lsb":0,"size":128},
            {"name":"Preset A","msb":81,"lsb":64,"size":128,"display":"001-128"},
            {"name":"B","lsb":1,"first_pc":1,"size":72,"first_number":129}]}"#);
        assert_eq!(recall(&d, "preset a", 1, None).unwrap(), vec![vec![0xB2, 0, 81], vec![0xB2, 32, 64], vec![0xC2, 0]]);
        assert_eq!(recall(&d, "1", 128, Some(0)).unwrap(), vec![vec![0xB0, 0, 81], vec![0xB0, 32, 64], vec![0xC0, 127]]);
        assert_eq!(recall(&d, "B", 130, None).unwrap(), vec![vec![0xB2, 32, 1], vec![0xC2, 2]], "CC 32 only, PC from 1");
        assert!(recall(&d, "B", 128, None).unwrap_err().contains("129..=200"));
        assert!(recall(&d, "Card", 1, None).unwrap_err().contains("User, Preset A, B"));
    }

    #[test]
    fn a_recall_sequence_wraps_the_program_change_or_replaces_it() {
        let d = dev(r#"{"before":[[240,66,"{channel:u7+0x30}",117,78,2,247]],"after":[[240,65,"{dev}","{index:u14_lsb}","{index:u14_msb}",247]],
            "banks":[{"name":"A","size":100},{"name":"B","size":100}]}"#);
        let m = recall(&d, "B", 100, None).unwrap();
        assert_eq!(m, vec![vec![0xF0, 0x42, 0x32, 0x75, 0x4E, 2, 0xF7], vec![0xC2, 99], vec![0xF0, 0x41, 16, 199 & 0x7F, 1, 0xF7]]);
        let song = dev(r#"{"program_change":false,"before":[[243,"{program}"]],"banks":[{"name":"Songs","size":100,"first_number":0}]}"#);
        assert_eq!(recall(&song, "Songs", 42, None).unwrap(), vec![vec![0xF3, 42]], "Song Select, no program change");
    }

    #[test]
    fn the_route_takes_a_bank_name_or_index() {
        let d = dev(r#"{"banks":[{"name":"A","size":16},{"name":"B","first_pc":16,"size":16}]}"#);
        let call = |args: Vec<OscType>| recall_osc(&d, "/box/program/recall", &args);
        assert_eq!(call(vec![OscType::String("B".into()), OscType::Int(3)]).unwrap().unwrap(), vec![vec![0xC2, 18]]);
        assert_eq!(call(vec![OscType::Int(1), OscType::Int(3), OscType::Int(9)]).unwrap().unwrap(), vec![vec![0xC9, 18]]);
        assert!(call(vec![OscType::String("B".into())]).unwrap().is_err());
        assert!(recall_osc(&d, "/box/program_change", &[]).is_none());
    }
}
