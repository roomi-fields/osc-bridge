//! The relay protocol: how a "door" — the `/connect` page in a browser (Web
//! MIDI), `osc-bridge connect` (`crate::connect`), a plugin, any program
//! speaking it — lets an agent reach the MIDI ports of a machine it does not
//! run on, through the SynthHub server.
//!
//! The door opens a WebSocket to the server (`crate::connect::DEFAULT_RELAY`).
//! The server gives the session a pairing code; the user hands that code to
//! their agent, whose MCP calls (`relay_*` tools) then travel through the
//! server to the door and on to the hardware. The server does all the work —
//! it turns named OSC addresses into MIDI bytes with the catalogue's drivers —
//! and the door only moves bytes between the WebSocket and its MIDI ports.
//!
//! Frames are JSON text.
//! - server → door: `{"type":"hello","code","user"}` once, then
//!   `{"type":"midi_out","port","data":[bytes]}` for each message to send,
//!   and `{"type":"confirm","id","port","messages":[[bytes]],"bytes","gap_ms"}`
//!   for a batch that may overwrite the device's memory (`needs_confirmation`):
//!   the door asks the person, sends the batch only if they accept (waiting
//!   `gap_ms` between messages), and answers
//!   `{"type":"confirm_reply","id","accepted":bool}`. A `confirm_cancel`
//!   `{"type":"confirm_cancel","id"}` withdraws an unanswered request. A door
//!   never sends such a message from a plain `midi_out`. `{"type":"paired"}`
//!   tells the door an agent now drives it (that agent only, until a new
//!   session gives a new code).
//! - door → server: `{"type":"ports","outputs":[names],"inputs":[names]}`
//!   (with `"known":{port: driver id}`, the devices the person recognised on
//!   the page, when there are) whenever its ports change, `{"type":"midi_in","port","data":[bytes]}`
//!   for each message a device sends, `confirm_reply`, and optionally
//!   `{"type":"share","enabled":bool,"private":[port names]}` (default:
//!   enabled, no port private): whether the device identities seen in the
//!   session may be recorded for the catalogue, and the ports whose devices
//!   the person keeps private (never recorded, whatever `enabled` says).
//!
//! The door connects with `?user=<id>` to keep its account; an id the server
//! did not issue gets a new account. A session lives as long as its socket.

/// The MIDI Identity Request, whatever device id it addresses.
pub fn is_identity_request(msg: &[u8]) -> bool {
    matches!(msg, [0xF0, 0x7E, _, 0x06, 0x01, 0xF7])
}

/// Whether a message may change what the device keeps in memory: every
/// SysEx (patch dumps, stores, settings) but the Identity Request. A door
/// sends these only once the person accepts.
pub fn needs_confirmation(msg: &[u8]) -> bool {
    msg.first() == Some(&0xF0) && !is_identity_request(msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_sysex_other_than_the_identity_request_needs_confirmation() {
        assert!(!needs_confirmation(&[0xF0, 0x7E, 0x7F, 0x06, 0x01, 0xF7]));
        assert!(!needs_confirmation(&[0xF0, 0x7E, 0x00, 0x06, 0x01, 0xF7]));
        assert!(needs_confirmation(&[0xF0, 0x42, 0x30, 0x00, 0x01, 0x2C, 0x4C, 0x00, 0xF7]));
        assert!(needs_confirmation(&[0xF0, 0x7E, 0x7F, 0x06, 0x02, 0xF7]));
        assert!(!needs_confirmation(&[0xB0, 74, 64]));
        assert!(!needs_confirmation(&[0xC0, 5]));
    }
}
