# VCV Rack — driver companion (passthrough via community OSC plugin)

VCV Rack 2.x ships without OSC support. To pilot it via osc-bridge, install
an OSC plugin that opens an OSC server inside Rack and configure its port to
match the bridge's `transport.port`.

## Recommended plugin: `vcv-osc` (full patch control)

The **[vcv-osc](https://github.com/roomi-fields/vcv-osc)** plugin's *OSC
Controller* module is purpose-built for this bridge. It listens on **7770**
and replies on **7771** by default (already this JSON's `port` / `reply_port`),
so no configuration is needed. Unlike CV-only OSC modules, it controls the
whole patch — every parameter, cables, module lifecycle, presets and full
state dump:

| Send (bridge prefixes `/vcv`) | Effect |
|---|---|
| `/vcv/param/set <mod> <param> <value>` | Set any knob/button/switch |
| `/vcv/param/get <mod> <param>` | → `/vcv/param/value <mod> <param> <v>` |
| `/vcv/param/<mod>/<param> <value>` | Same, RESTful per-address form |
| `/vcv/cable/add <outMod> <outPort> <inMod> <inPort>` | Patch a cable |
| `/vcv/cable/remove <inMod> <inPort>` | Unpatch a cable |
| `/vcv/module/add <plugin> <model> [x y]` | Instantiate a module |
| `/vcv/module/remove <mod>` | Delete a module (and its cables) |
| `/vcv/module/preset_save <mod> <path>` · `/vcv/module/preset_load <mod> <path>` | Save / load a preset file |
| `/vcv/state/dump [1]` | → `/vcv/state/module…`, `/vcv/state/param…`, `/vcv/state/input…`, `/vcv/state/output…`, `/vcv/state/cable…`, `/vcv/state/done` |
| `/vcv/param/watch <mod> <param> [1]` | Stream a param's changes back |
| `/vcv/state/watch [1]` | Stream topology changes → `/vcv/event/module_add\|module_remove\|cable_add\|cable_remove` |
| `/vcv/registry/dump` | List every installed module (slug, name, description) |

**Addressing by name.** Every `<mod>` / `<param>` / `<port>` accepts a numeric
id **or** a name string — `/vcv/param/set "Fundamental/VCO" "Frequency" 0.5`,
`/vcv/module/add "Audible Instruments" "Braids"`. Names come from
`/vcv/state/dump`, so a mapping survives a patch reload. Full address reference:
the vcv-osc README.

**OSCQuery (outside this bridge).** vcv-osc also serves an OSCQuery description
of the live patch on **HTTP 7772**, so TouchOSC / Open Stage Control auto-build a
labelled, ranged control surface. That is a direct HTTP link to the plugin, not
an OSC passthrough — point the controller straight at `http://<rack-host>:7772/`.

## Alternative: CV-only OSC modules

Any community OSC plugin (FormsAndShapes "OSC In/Out", Stoermelder pack, etc.)
also works for simple CV in the passthrough model; consult that plugin's own
path conventions.

## Ports

| Direction | Port | Notes |
|---|---|---|
| Bridge → VCV | UDP 7770 | the chosen module listens on this port |
| VCV → Bridge | UDP 7771 | the chosen output module targets 127.0.0.1:7771 |

Both are arbitrary — match whatever the installed module exposes.

## Usage

Add an OSC In / OSC Out / OSC bridge module to your VCV Rack patch.
Configure it to listen on 7770 (or whichever port your driver JSON
declares). Wire its CV outputs to your modulation targets. Then from the
bridge's client:

```
/vcv/cutoff 0.45
/vcv/note 60
```

→ bridge strips `/vcv`, forwards `/cutoff 0.45` and `/note 60` to UDP
7770. The VCV OSC plugin parses them and routes to the corresponding
CV outputs.

For module-specific path conventions, consult the documentation of the
OSC plugin you installed. The bridge does not constrain the path layout.
