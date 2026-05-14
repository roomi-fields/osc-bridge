# Lua scripting — escape hatch

> **Use this only when the declarative JSON schema cannot express what you need.**
> 95% of devices don't require any Lua. CC, NRPN, SysEx templating, u7/u14
> splits, reply matching — all declarative. Reach for scripting only for exotic
> checksums, non-standard encodings, or conditional transforms that the schema
> can't describe.
>
> `osc-bridge lint <device.json>` warns on every use of `transform` or `script`.
> That's intentional — it's a signal, not a shame.

## Three levels

Three optional fields, all opt-in. Introduce them in order: `transform` first,
`script` only if the former isn't enough.

### 1. `transform` — inline expression on a param value

Applied on the way in (OSC → MIDI), before range-clamping. Receives a single
`value` (integer) and must `return` a number.

```json
{
  "osc": "/volume_scaled",
  "cc": 7,
  "range": [0, 127],
  "transform": "return math.floor(value * 127 / 1000)"
}
```

Available on `ParamEntry` (SysEx param table) and `CcParamEntry`.

A twin field `transform_reverse` is reserved for the MIDI → OSC direction
(reply decoding). Not wired on CC yet; use `reply.script` if you need it now.

### 2. `script` — block on a command or reply pattern

For transforms the value engine can't do: inserting a computed checksum,
conditional drops, payload rewriting.

The script receives a `ctx` table and must `return ctx` (or `return nil` to
drop the message entirely). The Lua call signature is:

```lua
local ctx = ...   -- first Lua vararg is the context
-- mutate ctx here
return ctx
```

Context fields:

| field       | type           | meaning                                                |
|-------------|----------------|--------------------------------------------------------|
| `payload`   | table of ints  | assembled frame bytes (header + body + footer)         |
| `args`      | table of ints  | OSC args decoded to integers                           |
| `checksum`  | int or nil     | scratch space — set it, or insert into `payload`       |
| `direction` | string         | `"osc_to_midi"` or `"midi_to_osc"`                     |
| `device`    | string         | device name                                            |
| `command`   | string         | the OSC path that matched                              |

Example — XOR checksum inserted before the F7:

```json
{
  "osc": "/weird_cmd",
  "args": [{"name":"a","type":"u7"},{"name":"b","type":"u7"}],
  "frame": [16, "{a}", "{b}"],
  "script": "local ctx = ...; local sum = 0; for i = 5, #ctx.payload - 1 do sum = sum ~ ctx.payload[i] end; table.insert(ctx.payload, #ctx.payload, sum & 0x7F); return ctx"
}
```

**Reminder:** Lua tables are **1-indexed**. `ctx.payload[1]` is the SysEx `F0`.

### 3. `codec` — whole-device custom codec

Reserved for devices that can't be expressed declaratively at all. Not
implemented in this release; open an issue if you hit a case.

## The `ob` helper library

Always available in both `transform` and `script`:

```lua
ob.u7_clamp(v)       -- clamp v into [0, 127]
ob.u14_lsb(v)        -- low 7 bits
ob.u14_msb(v)        -- high 7 bits
ob.checksum_xor(t)   -- XOR over a table of bytes, masked to 7 bits
ob.checksum_sum(t)   -- sum % 128
ob.log(msg)          -- debug print (do not ship to prod)
```

## Sandbox & limits

- **No** `os`, `io`, `package`, `require`, `dofile`, `loadfile`, `debug`. A
  script can't touch the filesystem, network, or processes.
- **Memory cap**: 1 MiB per Lua instance. Allocating past that aborts the
  script with an error.
- **Timeout**: 10 ms hard deadline. An infinite loop is killed and the message
  is dropped (the runtime logs a warning; it does not crash).
- **One instance per device**, initialized lazily the first time a script is
  actually needed. Devices without scripts pay zero overhead.

## Failure modes

A script that errors (syntax, runtime, timeout, memory) logs a warning and
the message is dropped. The bridge keeps running. This is deliberate — Lua
failures must never take down the MIDI bus for everyone else.

## Testing

Unit tests: `cargo test --test scripting`. Integration (real device JSON):
`cargo test --test scripting_integration`.

## See also

- `docs/CR-lua-scripting.md` — the design rationale and delivery order.
- `docs/DEVICE_JSON_SCHEMA.md` — everything you can express without touching Lua.
