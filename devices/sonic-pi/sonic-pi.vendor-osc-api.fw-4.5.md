# Sonic Pi — driver companion (passthrough)

This driver bridges OSC under `/sonicpi/...` to Sonic Pi's built-in OSC server.
It's a **passthrough** driver: the bridge does not interpret the OSC paths,
it just forwards them verbatim. Your Sonic Pi project decides what the paths
mean by writing `sync :osc:/<path>` or `live_loop` patterns.

## Ports

| Direction | Port | Notes |
|---|---|---|
| Bridge → Sonic Pi | UDP 4560 | Sonic Pi's default OSC input |
| Sonic Pi → Bridge | UDP 4570 | This driver's reply_port — Sonic Pi sends back here with `osc_send "127.0.0.1", 4570, "/path", arg…` |

Change either via `orchestrate.toml` if your Sonic Pi runs elsewhere or on a
different port.

## Usage

In Sonic Pi, accept OSC from external clients (one-time, in any buffer):

```ruby
use_osc_logging false
set_sched_ahead_time! 0.5
```

Match incoming OSC anywhere in your code:

```ruby
live_loop :hit do
  b = sync "/osc*/trigger/kick"
  sample :bd_haus, amp: b[0]
end
```

From the bridge's client, send :

```
/sonicpi/trigger/kick 0.9
```

Bridge strips `/sonicpi`, forwards `/trigger/kick 0.9` to port 4560 on
Sonic Pi. Sonic Pi's `sync "/osc*/trigger/kick"` fires.

For replies (Sonic Pi → bridge clients), have Sonic Pi push back:

```ruby
osc_send "127.0.0.1", 4570, "/beat", 1
```

The bridge's clients see `/sonicpi/beat 1`.

## Why passthrough

Sonic Pi doesn't expose a fixed OSC surface — the meaning of paths is
defined inside each user's Sonic Pi project. A declarative JSON driver
with `commands[]` would constrain that freedom for no benefit. Passthrough
lets you keep the `/sonicpi` namespace as the integration boundary while
your Sonic Pi code stays in charge of the semantics.
