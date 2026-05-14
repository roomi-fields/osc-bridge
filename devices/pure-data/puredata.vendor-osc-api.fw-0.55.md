# Pure Data — driver companion (passthrough)

Bridges `/pd/...` to a Pure Data patch via UDP. Pd is a freeform OSC host
— there's no standard surface, your patch defines the paths.

## Ports

| Direction | Port | Notes |
|---|---|---|
| Bridge → Pd | UDP 9000 | `[netreceive -u -b 9000]` in your patch |
| Pd → Bridge | UDP 9001 | `[netsend -u -b]` + `[connect 127.0.0.1 9001]` |

Both ports are arbitrary — change them in the JSON if your patch uses
different ones.

## Minimal Pd patch (receive side)

```
[netreceive -u -b 9000]
|
[oscparse]
|
[route /trigger /set_volume]
|...
```

From the bridge client:

```
/pd/trigger 60 127
```

→ bridge strips `/pd`, forwards `/trigger 60 127` as OSC to Pd's
`[netreceive]`. `[oscparse]` decodes it, `[route /trigger /set_volume]`
dispatches to your handler.

## Minimal Pd patch (send side)

```
[osc-bridge-out]   ← you wire your data here
|
[oscformat /beat]
|
[netsend -u -b]
| (connect 127.0.0.1 9001)
```

When Pd emits `/beat 1` via `[netsend]`, the bridge's clients see
`/pd/beat 1`.
