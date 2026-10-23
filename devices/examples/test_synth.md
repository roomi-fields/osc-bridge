# Test Synth (virtual)

Not a real instrument: the synth that `osc-bridge connect --test-synth` and
the [connect page](https://synthhub.roomi-fields.com/connect/) can simulate,
so the whole chain — relay, detection, sending, listening, reports, learned
mappings, contributions — can be tested without hardware.

- Answers the MIDI Identity Request as manufacturer `7D` (the ID MIDI reserves
  for non-commercial use), family 1, model 1, firmware `01 00 00 00`.
- Sends back every channel message it receives, as if the control had been
  moved on the device: `relay_send` then `relay_listen` sees it.
- Controls: `/filter/cutoff` (CC 74), `/filter/resonance` (CC 71),
  `/amp/attack` (CC 73), `/amp/release` (CC 72), `/osc/waveform` (CC 70,
  0-3), `/lfo/rate` (NRPN 0/1, 14-bit); drums `/drum/kick`, `/drum/snare`,
  `/drum/hat` (notes 36, 38, 42 on channel 10); notes and pitch bend.
