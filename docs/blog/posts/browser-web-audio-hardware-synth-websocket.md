---
date: 2026-10-22
authors: [romain]
title: "From a Web Page to a Hardware Synth: OSC over WebSocket"
description: >-
  Start osc-bridge with --ws-bind and a web page can drive a hardware synth: binary WebSocket frames carry raw OSC that the bridge turns into MIDI, and back.
slug: browser-web-audio-hardware-synth-websocket
---

# From a Web Page to a Hardware Synth: OSC over WebSocket

A web page can control a hardware synth without the Web MIDI API: run osc-bridge with `--ws-bind 127.0.0.1:7890`, open a WebSocket to it, and send OSC packets as binary frames. The bridge treats each frame exactly like an OSC packet received over UDP, turns it into MIDI CC, NRPN or SysEx for the synth, and sends incoming MIDI (knob turns, notes, SysEx replies) back to the page as OSC. All it takes in the browser is an OSC encoder and decoder of about 60 lines, shown below.

<!-- more -->

## Why not use Web MIDI?

Web MIDI is the obvious route, and in Chrome, Edge and Firefox it works. Two things make it a poor base for a page meant for everyone:

- **Safari has no Web MIDI.** WebKit lists the Web MIDI API among the features it has "decided to not yet implement due to fingerprinting, security, and other concerns" ([WebKit tracking prevention](https://webkit.org/tracking-prevention/)). Most browsers on iOS run on WebKit, so that rules out most iPhones and iPads too.
- **Web MIDI gives you bytes, not parameters.** The page still has to know that the Subsequent 37's cutoff is NRPN 3/115, or how the MiniLab 3 builds a pad-colour SysEx frame.

A WebSocket works in every browser. osc-bridge holds the per-device knowledge in its driver files, so the page sends `/sub37/filter/cutoff 8192` and the bridge does the rest.

## How do you start the bridge with WebSocket?

Add `--ws-bind` to `osc-bridge run`:

```bash
osc-bridge run --device devices/arturia/minilab3.json \
  --out-port 4 --in-port 0 --ws-bind 127.0.0.1:7890
```

The log confirms it with a `WS IN    127.0.0.1:7890` line, then `WS +client …` and `WS -client …` as pages connect and leave. UDP on 7777 keeps working alongside.

With several devices, use the orchestrator and set `ws_bind` in the `[osc]` section of `bridge.toml`:

```toml
[osc]
bind = "127.0.0.1:7777"
ws_bind = "127.0.0.1:7890"
```

WebSocket is off unless you ask for it. It is in osc-bridge since v0.13.0.

## What goes over the wire?

One binary WebSocket frame carries one OSC packet, with the same bytes the UDP transport uses: no JSON wrapper, no extra framing. Text frames are ignored, with a warning in the log.

A connected WebSocket client behaves like an extra `--osc-client`. It receives everything the bridge broadcasts (decoded MIDI input, SysEx replies, `/bridge/status` answers), and every frame it sends goes through the same dispatch as UDP. The implementation is in `src/ws.rs`; `tests/ws_transport.rs` checks both directions, including a full round trip through a running bridge.

## How do you encode OSC in JavaScript?

OSC 1.0 has a small set of rules. The address and the type tag string (a comma followed by one letter per argument) are null-terminated ASCII strings padded to a multiple of 4 bytes. `int32` and `float32` arguments are 4 bytes, big-endian. `T` and `F` (true, false) have no payload.

```js
// osc.js: minimal OSC 1.0 encoder/decoder (int32, float32, string, true/false)
const enc = new TextEncoder();
const dec = new TextDecoder();

function oscString(s) {
  const bytes = enc.encode(s);
  const out = new Uint8Array((bytes.length + 4) & ~3); // null-terminated, padded to 4
  out.set(bytes);
  return out;
}

// args: array of [tag, value], tag in "i" | "f" | "s" | "T" | "F"
export function encodeOSC(address, args = []) {
  const parts = [oscString(address), oscString("," + args.map(([t]) => t).join(""))];
  for (const [t, v] of args) {
    if (t === "i" || t === "f") {
      const b = new Uint8Array(4);
      const dv = new DataView(b.buffer);
      if (t === "i") dv.setInt32(0, v, false);   // big-endian
      else dv.setFloat32(0, v, false);
      parts.push(b);
    } else if (t === "s") {
      parts.push(oscString(String(v)));
    } else if (t !== "T" && t !== "F") {
      throw new Error("unsupported OSC type " + t);
    }
  }
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let off = 0;
  for (const p of parts) { out.set(p, off); off += p.length; }
  return out;
}

function readString(dv, off) {
  let end = off;
  while (dv.getUint8(end) !== 0) end++;
  const s = dec.decode(new Uint8Array(dv.buffer, dv.byteOffset + off, end - off));
  return [s, (end + 4) & ~3]; // skip the null and the padding
}

// buf: ArrayBuffer holding one OSC message (the bridge sends messages, not bundles)
export function decodeOSC(buf) {
  const dv = new DataView(buf);
  let [address, off] = readString(dv, 0);
  let tags;
  [tags, off] = readString(dv, off);
  const args = [];
  for (const t of tags.slice(1)) {
    if (t === "i") { args.push(dv.getInt32(off, false)); off += 4; }
    else if (t === "f") { args.push(dv.getFloat32(off, false)); off += 4; }
    else if (t === "h") { args.push(Number(dv.getBigInt64(off, false))); off += 8; }
    else if (t === "d") { args.push(dv.getFloat64(off, false)); off += 8; }
    else if (t === "s") { let s; [s, off] = readString(dv, off); args.push(s); }
    else if (t === "T") args.push(true);
    else if (t === "F") args.push(false);
    else throw new Error("unsupported OSC type " + t);
  }
  return { address, args };
}
```

Types are explicit on purpose. JavaScript has one number type, so `120.0` and `120` are the same value; the tag decides whether it travels as an int or a float. For CC and NRPN parameters, send `"i"` with the device's raw value: the bridge truncates floats, so `0.5` becomes `0`.

As a check, this encoder produces the same bytes as the bridge's own `osc-bridge osc-send` for `/minilab3/pad/0/color 127 0 0`:

```
2f6d696e696c6162332f7061642f302f636f6c6f72000000  2c696969 00000000
0000007f 00000000 00000000
```

(spaces added: address padded to 24 bytes, `,iii` padded to 8, then three int32 values).

## How does the page talk to the synth?

Open the socket with `binaryType = "arraybuffer"`, send encoded packets, decode what comes back. This page lights a MiniLab 3 pad, writes to its screen, and uses the MiniLab's knob to drive a Web Audio filter in the page:

```html
<script type="module">
import { encodeOSC, decodeOSC } from "./osc.js";

const ws = new WebSocket("ws://127.0.0.1:7890");
ws.binaryType = "arraybuffer";

// Web Audio: an oscillator through a low-pass filter
const ctx = new AudioContext();
const filter = new BiquadFilterNode(ctx, { type: "lowpass", frequency: 800 });
const osc = new OscillatorNode(ctx, { type: "sawtooth", frequency: 110 });
osc.connect(filter).connect(ctx.destination);
document.body.addEventListener("click", () => { ctx.resume(); osc.start(); }, { once: true });

ws.onopen = () => {
  ws.send(encodeOSC("/minilab3/pad/0/color", [["i", 127], ["i", 0], ["i", 0]]));
  ws.send(encodeOSC("/minilab3/display/text", [["s", "Hello"], ["s", "browser"]]));
};

ws.onmessage = (e) => {
  if (typeof e.data === "string") return;
  const { address, args } = decodeOSC(e.data);
  // A knob turn arrives as /minilab3/cc/<number> <value> <channel>
  if (address === "/minilab3/cc/18") {
    const hz = 100 * Math.pow(80, args[0] / 127);   // 100 Hz .. 8 kHz
    filter.frequency.setTargetAtTime(hz, ctx.currentTime, 0.02);
  }
};
</script>
```

Change `18` to the CC your knob sends; the address format comes from the driver's `midi_in` block. To find out what arrives, log every `address` in `onmessage`.

Sending to another synth is the same call with that driver's addresses, for example `encodeOSC("/sub37/note/on", [["i", 48], ["i", 100]])` when the bridge runs `devices/moog/subsequent-37.json`.

## Is it safe to open a WebSocket on my machine?

Bind to `127.0.0.1`, as above, and other computers on the network cannot connect. Use `0.0.0.0` or a LAN address only if you mean to accept tablets or other machines, on a network you trust.

Binding to loopback does not protect you from other web pages. Browsers let any page open a WebSocket to `127.0.0.1`, and the bridge in this version does not check the page's origin and has no authentication. While `--ws-bind` is on, any site open in your browser could send OSC to your synths. Turn it on when you use it, not permanently.

The bridge does not do TLS, so the URL is `ws://`, not `wss://`. Serve your page from `http://localhost` during development; an `https://` page may be blocked from opening a `ws://` connection by the browser's mixed-content rules.

## What are the limits?

- **Lossy under load.** Each client has a queue of 256 outgoing frames; a slow page loses frames past that but stays connected. A client that stalls for 5 seconds is disconnected.
- **No timing.** Incoming OSC bundles are accepted, but time tags are ignored and their messages are sent at once.
- **One thread per connection.** Fine for a handful of pages on one machine; not designed as a public server.
- **The drivers decide what works.** Most of the 963 drivers come from documentation and have not been tested on the hardware. Check `_sources` in the driver, or the [catalogue](../../catalogue.md).
- **A local process is still required.** The page cannot reach MIDI by itself; someone has to run osc-bridge on the computer the synth is plugged into.

## FAQ

**Do I need an OSC library?**
No. The encoder and decoder above cover what the bridge sends and expects. Any OSC library that produces standard binary packets also works.

**Does this replace Web MIDI in Chrome?**
It does a different job. Web MIDI is enough when you only need raw notes and CCs in Chromium or Firefox; the bridge adds Safari, parameter names, NRPN and SysEx.

**Can the same page drive Ableton or Sonic Pi?**
Yes. Software targets are drivers like any other: with the Sonic Pi driver, `/sonicpi/...` messages are forwarded to Sonic Pi on port 4560 and its replies come back to the page.

**Where are the commands documented?**
The [getting started guide](../../getting-started.md) and the [device JSON schema](../../DEVICE_JSON_SCHEMA.md). Source: <https://github.com/roomi-fields/osc-bridge>.
