// Relay door: carries MIDI between this browser's devices and the hosted
// SynthHub server, so an agent can reach them. Runs only on the page that
// holds the .ob-connect block. Protocol: see src/relay_protocol.rs.
(function () {
  const root = document.querySelector(".ob-connect");
  if (!root) return;
  const RELAY = "wss://mcp.synthhub.roomi-fields.com/relay";
  const MCP = "https://mcp.synthhub.roomi-fields.com";
  const $ = id => document.getElementById(id);
  const store = {
    get(k) { try { return localStorage.getItem(k); } catch (e) { return null; } },
    set(k, v) { try { localStorage.setItem(k, v); } catch (e) { /* private mode */ } },
  };
  const hex = data => Array.from(data, b => b.toString(16).toUpperCase().padStart(2, "0")).join(" ");
  let access = null;
  let ws = null;
  // Virtual test synth (devices/examples/test_synth): answers the Identity
  // Request as manufacturer 7D, echoes channel messages. Same as
  // osc-bridge connect --test-synth.
  const TEST_SYNTH = "osc-bridge Test Synth";
  const IDENTITY_REQUEST = [0xF0, 0x7E, 0x7F, 0x06, 0x01, 0xF7];
  const IDENTITY_REPLY = [0xF0, 0x7E, 0x00, 0x06, 0x02, 0x7D, 0x01, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x00, 0xF7];
  const testSynth = () => $("ob-connect-test").checked;
  // Every SysEx but the Identity Request may overwrite what the device keeps
  // (patches, settings): the server sends those as a "confirm" request, and
  // they go out only once the person accepts. Never from a plain midi_out.
  const isIdentityRequest = d => d.length === 6 && d[0] === 0xF0 && d[1] === 0x7E && d[3] === 0x06 && d[4] === 0x01 && d[5] === 0xF7;
  const needsConfirmation = d => d[0] === 0xF0 && !isIdentityRequest(d);
  const asking = [];
  // The test synth keeps one sound (cutoff, resonance, attack, release,
  // waveform, set by their CC), dumps it when asked and takes a patch, like
  // a real synth's edit buffer. Same as TestSynth in src/connect.rs.
  const DUMP_REQUEST = [0xF0, 0x7D, 0x01, 0x10, 0xF7];
  const PATCH = [0xF0, 0x7D, 0x01, 0x20];
  const SOUND_CC = [74, 71, 73, 72, 70];
  const sound = [64, 0, 0, 40, 0];
  const same = (a, b) => a.length === b.length && a.every((x, i) => x === b[i]);
  function testSynthAnswer(data) {
    if (same(Array.from(data), IDENTITY_REQUEST)) return IDENTITY_REPLY;
    if (same(Array.from(data), DUMP_REQUEST)) return PATCH.concat(sound, [0xF7]);
    if (data.length === 10 && same(Array.from(data.slice(0, 4)), PATCH) && data[9] === 0xF7) {
      for (let i = 0; i < 5; i++) sound[i] = data[4 + i];
      return null;
    }
    if (data[0] >= 0xB0 && data[0] <= 0xBF && data.length === 3) {
      const i = SOUND_CC.indexOf(data[1]);
      if (i >= 0) sound[i] = data[2];
    }
    if (data[0] >= 0x80 && data[0] <= 0xEF) return data;
    return null;
  }

  function status(text) { $("ob-connect-status").textContent = text; }
  function log(dir, port, data) {
    const box = $("ob-connect-log");
    const line = document.createElement("div");
    line.textContent = new Date().toLocaleTimeString() + "  " + dir + "  " + port + "  " + hex(data);
    box.prepend(line);
    while (box.childElementCount > 40) box.lastChild.remove();
  }
  function names(map) {
    const list = map ? Array.from(map.values(), p => p.name) : [];
    if (testSynth()) list.push(TEST_SYNTH);
    return list;
  }
  function send(obj) { if (ws && ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify(obj)); }
  // Ports whose device the person keeps private (identity never recorded),
  // remembered in this browser by port name. Shared unless ticked.
  function privatePorts() {
    try { return JSON.parse(store.get("osc-bridge-private") || "[]"); } catch (e) { return []; }
  }
  let shown = [];
  function sendShare() {
    const mine = privatePorts();
    send({ type: "share", enabled: $("ob-connect-share").checked, private: shown.filter(n => mine.includes(n)) });
  }
  function sendPorts() {
    const outputs = names(access && access.outputs), inputs = names(access && access.inputs);
    send({ type: "ports", outputs, inputs });
    shown = Array.from(new Set(outputs.concat(inputs)));
    const box = $("ob-connect-ports");
    box.textContent = shown.length ? "Devices (tick to keep one private: its identity is never recorded):" :
      "No MIDI device found: plug one in, it will appear here.";
    const mine = privatePorts();
    shown.forEach(name => {
      const label = document.createElement("label");
      label.className = "ob-connect-device";
      const tick = document.createElement("input");
      tick.type = "checkbox";
      tick.checked = mine.includes(name);
      tick.addEventListener("change", () => {
        const now = privatePorts().filter(n => n !== name);
        if (tick.checked) now.push(name);
        store.set("osc-bridge-private", JSON.stringify(now));
        sendShare();
      });
      label.append(tick, " " + name + " — private");
      box.append(label);
    });
    sendShare();
  }
  function listen() {
    if (!access) return;
    access.inputs.forEach(input => {
      input.onmidimessage = e => {
        if (e.data[0] === 0xF8 || e.data[0] === 0xFE) return; // clock, active sensing
        send({ type: "midi_in", port: input.name, data: Array.from(e.data) });
        log("←", input.name, e.data);
      };
    });
  }

  // Play one message on a port (a real output or the test synth).
  function deliver(port, data) {
    if (port === TEST_SYNTH) {
      log("→", port, data);
      const reply = testSynthAnswer(data);
      if (reply) setTimeout(() => {
        send({ type: "midi_in", port: TEST_SYNTH, data: Array.from(reply) });
        log("←", TEST_SYNTH, reply);
      }, 5);
      return;
    }
    const out = access ? Array.from(access.outputs.values()).find(o => o.name === port) : null;
    if (!out) return log("✕", port, data);
    try { out.send(data); log("→", port, data); }
    catch (e) { log("✕ " + e.message, port, data); }
  }
  function answer(req, accepted) {
    if (accepted) {
      const step = i => {
        if (i >= req.messages.length) return send({ type: "confirm_reply", id: req.id, accepted: true });
        deliver(req.port, req.messages[i]);
        setTimeout(() => step(i + 1), i + 1 < req.messages.length ? req.gap_ms || 0 : 0);
      };
      step(0);
    } else {
      send({ type: "confirm_reply", id: req.id, accepted: false });
      log("✕ refused", req.port, []);
    }
  }
  function ask() {
    const box = $("ob-connect-confirm");
    const req = asking[0];
    if (!req) { box.hidden = true; return; }
    if ($("ob-connect-always").checked) { asking.shift(); answer(req, true); return ask(); }
    const n = req.messages.length;
    const what = n + " SysEx message" + (n > 1 ? "s" : "") + " (" + req.bytes + " bytes) to " + req.port;
    $("ob-connect-confirm-text").textContent = req.why
      ? "Your agent wants to " + req.why + ": " + what + ". Accept only if you expect it: this page cannot check what they do."
      : "Your agent wants to send " + what + ". This can replace sounds or settings stored in the device.";
    box.hidden = false;
  }
  function decide(accepted) {
    const req = asking.shift();
    if (req) answer(req, accepted);
    ask();
  }

  function open() {
    const user = store.get("osc-bridge-user");
    ws = new WebSocket(RELAY + (user ? "?user=" + encodeURIComponent(user) : ""));
    status("Connecting to the relay…");
    ws.onmessage = ev => {
      let msg;
      try { msg = JSON.parse(ev.data); } catch (e) { return; }
      if (msg.type === "hello") {
        store.set("osc-bridge-user", msg.user);
        $("ob-connect-code").textContent = msg.code;
        $("ob-connect-url").textContent = MCP + "/u/" + msg.user + "/mcp";
        $("ob-connect-say").textContent = "My osc-bridge relay code is " + msg.code;
        $("ob-connect-paired").hidden = false;
        status("Connected. Waiting for your agent.");
        sendPorts();
      } else if (msg.type === "midi_out") {
        if (needsConfirmation(msg.data)) return log("✕ not confirmed", msg.port, msg.data);
        deliver(msg.port, msg.data);
      } else if (msg.type === "confirm") {
        asking.push(msg);
        if (asking.length === 1) ask();
      } else if (msg.type === "confirm_cancel") {
        const i = asking.findIndex(r => r.id === msg.id);
        if (i >= 0) { asking.splice(i, 1); if (i === 0) ask(); }
      } else if (msg.type === "paired") {
        status("An agent is connected. Only it can use this code; press New code to change agent.");
      } else if (msg.type === "error") {
        status("Relay error: " + msg.error);
      }
    };
    ws.onclose = () => {
      asking.length = 0;
      ask();
      status("Disconnected. Press the button to start a new session (the code will change).");
      $("ob-connect-paired").hidden = true;
      $("ob-connect-go").disabled = false;
    };
  }

  $("ob-connect-go").addEventListener("click", async () => {
    $("ob-connect-go").disabled = true;
    if (!navigator.requestMIDIAccess) {
      if (!testSynth()) {
        status("This browser gives no MIDI access to web pages (Safari, iOS). Use Chrome or Edge, or osc-bridge connect — or tick the virtual test synth.");
        $("ob-connect-go").disabled = false;
        return;
      }
      open();
      return;
    }
    try {
      access = await navigator.requestMIDIAccess({ sysex: true });
    } catch (e) {
      try {
        access = await navigator.requestMIDIAccess();
        log("!", "SysEx refused", []);
      } catch (e2) {
        status("MIDI access refused: allow it in the browser's site settings, then try again.");
        $("ob-connect-go").disabled = false;
        return;
      }
    }
    access.onstatechange = () => { listen(); sendPorts(); };
    listen();
    open();
  });
  $("ob-connect-new").addEventListener("click", () => {
    if (!ws) return;
    ws.onclose = () => open();
    ws.close();
  });
  $("ob-connect-accept").addEventListener("click", () => decide(true));
  $("ob-connect-refuse").addEventListener("click", () => decide(false));
  $("ob-connect-test").addEventListener("change", () => { if (ws) sendPorts(); });
  $("ob-connect-share").addEventListener("change", sendShare);
  root.querySelectorAll(".ob-connect-copy").forEach(b => b.addEventListener("click", () => {
    const text = $(b.dataset.copy).textContent;
    if (navigator.clipboard) navigator.clipboard.writeText(text).then(() => { b.textContent = "copied"; });
  }));
})();
