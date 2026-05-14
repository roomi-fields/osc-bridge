# VCV Rack — driver companion (passthrough via community OSC plugin)

VCV Rack 2.5 ships without OSC support. To pilot it via osc-bridge, install
**any** community OSC plugin (FormsAndShapes "OSC In/Out", Stoermelder
pack's `STRIP-OSC`-family modules, etc.) from the VCV Library and
configure its port to match the bridge's `transport.port`.

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
