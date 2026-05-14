# TouchDesigner — driver companion (passthrough)

Bridges `/td/...` to TouchDesigner via UDP OSC. TouchDesigner exposes OSC
through a family of operators (OSC In CHOP, OSC In DAT, OSC Out CHOP, OSC
Out DAT); the project author chooses what messages are accepted and emitted.

## Ports

| Direction | Port | Notes |
|---|---|---|
| Bridge → TD | UDP 7000 | OSC In CHOP / DAT operator listens on 7000 |
| TD → Bridge | UDP 7001 | OSC Out CHOP / DAT operator targets 127.0.0.1:7001 |

Both are arbitrary — match what your TD project configures.

## Usage

In TouchDesigner, add an **OSC In CHOP** with `Local Port = 7000`. Any
incoming `/some/path val1 val2 ...` produces channels named after the
path. The bridge forwards `/td/some/path val1 val2` from clients as
`/some/path val1 val2` to TouchDesigner.

For replies, add an **OSC Out CHOP** with `Network Address = 127.0.0.1`
and `Network Port = 7001`. Outgoing TD channels become OSC messages that
the bridge re-emits as `/td/<original_addr>` to its clients.

Use cases : AV mapping, ML/MIDI/OSC hybrids, generative visuals driven
by your hardware or DAW via the same `/td/*` OSC surface as everything
else in your bridge.
