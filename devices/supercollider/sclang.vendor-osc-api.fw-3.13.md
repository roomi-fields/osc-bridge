# SuperCollider sclang — driver companion (passthrough)

Bridges `/sclang/...` to the SuperCollider language interpreter's OSC port
(57120). The SC user code declares which paths it cares about via `OSCdef` /
`OSCFunc` — the bridge does not interpret paths.

## Ports

| Direction | Port | Notes |
|---|---|---|
| Bridge → sclang | UDP 57120 | sclang's default OSC port |
| sclang → Bridge | UDP 57130 | Have your SC code emit replies via `NetAddr("127.0.0.1", 57130).sendMsg(...)` |

If you want to drive **scsynth** (the audio server, port 57110) directly,
copy this file as `scsynth.vendor-osc-api.fw-3.13.json`, change `osc_prefix`
to `/scsynth`, `port` to 57110, and a reply_port of your choice.

## Usage

In SuperCollider:

```supercollider
~bridge = NetAddr("127.0.0.1", 57130);
OSCdef(\trigger, { |msg|
    [\got, msg].postln;
    Synth(\default, [\freq, msg[1].midicps]);
    ~bridge.sendMsg("/ack", msg[1]);
}, "/trigger");
```

From the bridge client:

```
/sclang/trigger 60
```

→ bridge strips `/sclang`, forwards `/trigger 60` to sclang on 57120.
SC's OSCdef fires, posts the message, plays a synth, sends `/ack 60` back
to port 57130. The bridge's clients see `/sclang/ack 60`.
