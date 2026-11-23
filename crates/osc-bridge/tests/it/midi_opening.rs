//! Opening the MIDI system is guarded: a failure, or a panic inside the MIDI library (midir
//! panics when ALSA cannot allocate a queue), comes back as an error naming the MIDI system and
//! the cause, never as a panic. Nothing here opens the machine's MIDI system.

use osc_bridge::midi_port::guarded_open;

#[test]
fn an_opening_that_panics_comes_back_as_an_error_naming_the_midi_system_and_the_cause() {
    let e = guarded_open(|| -> anyhow::Result<u8> {
        panic!("ALSA function 'snd_seq_alloc_named_queue' failed with error 'Out of memory (12)'")
    })
    .unwrap_err()
    .to_string();
    assert!(e.contains("MIDI"), "{e}");
    assert!(e.contains("Out of memory (12)"), "{e}");
}

#[test]
fn an_opening_that_fails_keeps_its_cause_and_names_the_midi_system() {
    let e = guarded_open(|| -> anyhow::Result<u8> { Err(anyhow::anyhow!("no sequencer")) }).unwrap_err().to_string();
    assert!(e.contains("MIDI"), "{e}");
    assert!(e.contains("no sequencer"), "{e}");
}

#[test]
fn an_opening_that_succeeds_gives_its_value() {
    assert_eq!(guarded_open(|| Ok(7u8)).unwrap(), 7);
}
