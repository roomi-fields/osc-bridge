---
title: Connect your synth to an AI agent — no install
description: >-
  Plug a synth into your computer, open this page in Chrome or Edge, and give
  the code to Claude or any MCP agent: it can detect the device, play it and
  set its parameters by name, through osc-bridge's hosted relay.
schema_type: TechArticle
hide:
  - toc
---

# Connect your synth to an AI agent

This page is a relay: it lets an AI agent reach the MIDI devices plugged into
this computer, with nothing to install. Your browser talks to your synth; the
osc-bridge server turns the agent's requests ("set the filter cutoff to 64",
"play middle C") into the right MIDI messages for your device, from a catalogue
of 963 drivers.

<div class="ob-connect">
  <div class="ob-connect-start">
    <button type="button" id="ob-connect-go" class="md-button md-button--primary">Connect my MIDI devices</button>
    <label class="ob-connect-share"><input type="checkbox" id="ob-connect-share" checked>
      Share the identity of my devices (model, firmware) to improve the catalogue</label>
  </div>
  <p class="ob-connect-status" id="ob-connect-status">Not connected. Works in Chrome, Edge, Opera and Firefox; not in Safari.</p>
  <div class="ob-connect-paired" id="ob-connect-paired" hidden>
    <p>Your code: <span class="ob-connect-code" id="ob-connect-code">····-····</span></p>
    <ol>
      <li>In your AI assistant, add a remote MCP server (in Claude: <em>Settings → Connectors → Add custom connector</em>) with this address, once:<br>
        <code id="ob-connect-url"></code> <button type="button" class="ob-connect-copy" data-copy="ob-connect-url">copy</button></li>
      <li>Tell the agent: <code id="ob-connect-say"></code> <button type="button" class="ob-connect-copy" data-copy="ob-connect-say">copy</button></li>
      <li>Keep this tab open while you play. Closing it ends the session.</li>
    </ol>
    <p><button type="button" id="ob-connect-new" class="md-button">New code</button>
      The first agent to use a code is the only one that can drive your devices; a new code ends the session and pairs again.</p>
    <p class="ob-connect-ports" id="ob-connect-ports"></p>
  </div>
  <div class="ob-connect-log" id="ob-connect-log" aria-live="polite"></div>
</div>

## What happens

- The agent asks the server; the server sends MIDI messages to this page, which
  plays them on your device. Every message appears in the list above as it
  passes.
- `relay_detect` asks each device who it is (the standard MIDI Identity Request,
  which changes nothing on it) and finds its driver and firmware.
- `relay_send` sets a parameter or plays a note by its name in the driver.
- `relay_listen` hears what your device sends: turn a knob and the agent sees
  which parameter it is.

## Privacy

The server gives this browser an account — a random id, kept in this browser so
you stay the same user next time; no sign-up, no personal data. Device
identities are recorded only if the box above is ticked. The server does not
keep your address; like any site, the web server in front of it keeps a short
access log for a few days. Details: [API for agents](api.md).

## Limits

- Safari and iOS do not give web pages access to MIDI devices; use Chrome or
  Edge, or [`osc-bridge connect`](getting-started.md) on your computer.
- Messages that store a patch or restore a memory dump overwrite your device;
  a well-behaved agent asks you first.
- Timing follows the network: fine for setting sounds and playing test notes,
  not for tightly timed sequences.
