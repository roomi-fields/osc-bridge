---
title: Device catalogue — 963 synths, drum machines and DAWs
description: >-
  Search the 963 hardware synthesizers, drum machines, effects and DAWs that
  osc-bridge drives over OSC, MIDI and SysEx: vendor, source, coverage, and a
  page per device with its CC, NRPN and OSC map.
schema_type: CollectionPage
hide:
  - toc
---

# Device catalogue

Every device osc-bridge can drive, one row per driver file. Open a device to
see its named OSC addresses, CC and NRPN numbers, drum sounds and the command
that runs it. The source column says how far to trust each driver: ✅ tested
on the device, 📘 manufacturer documentation, 📡 the software's OSC API,
🎛️ an Electra One community preset, 📦 [MIDI Guide](https://midi.guide)
community data (CC BY-SA 4.0).

<div class="ob-catalogue" data-index="../devices.json" data-pages="../devices/">
  <div class="ob-controls">
    <input id="ob-q" type="search" placeholder="Search vendor, device or author…" aria-label="Search devices">
    <select id="ob-vendor" aria-label="Vendor"><option value="">All vendors</option></select>
    <div class="ob-chips" id="ob-markers">
      <button type="button" class="ob-chip" data-m="✅">✅ verified</button>
      <button type="button" class="ob-chip" data-m="📘">📘 vendor doc</button>
      <button type="button" class="ob-chip" data-m="📡">📡 OSC API</button>
      <button type="button" class="ob-chip" data-m="🎛️">🎛️ Electra</button>
      <button type="button" class="ob-chip" data-m="📦">📦 MIDI Guide</button>
      <button type="button" class="ob-chip" data-m="🤖">🤖 agent</button>
    </div>
  </div>
  <p class="ob-stats" id="ob-stats">Loading the catalogue…</p>
  <table class="ob-table">
    <thead><tr><th></th><th>Vendor</th><th>Device</th><th>Coverage</th><th>Author</th></tr></thead>
    <tbody id="ob-rows"></tbody>
  </table>
</div>

The same data is available as JSON: [`devices.json`](../devices.json).
