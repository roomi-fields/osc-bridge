// Relay door: carries MIDI between this browser's devices and the hosted
// osc-bridge server, so an agent can reach them. Runs only on the page that
// holds the .ob-connect block. Protocol: see src/relay.rs.
(function () {
  const root = document.querySelector(".ob-connect");
  if (!root) return;
  const RELAY = "wss://mcp.osc-bridge.roomi-fields.com/relay";
  const MCP = "https://mcp.osc-bridge.roomi-fields.com";
  const $ = id => document.getElementById(id);
  const store = {
    get(k) { try { return localStorage.getItem(k); } catch (e) { return null; } },
    set(k, v) { try { localStorage.setItem(k, v); } catch (e) { /* private mode */ } },
  };
  const hex = data => Array.from(data, b => b.toString(16).toUpperCase().padStart(2, "0")).join(" ");
  let access = null;
  let ws = null;

  function status(text) { $("ob-connect-status").textContent = text; }
  function log(dir, port, data) {
    const box = $("ob-connect-log");
    const line = document.createElement("div");
    line.textContent = new Date().toLocaleTimeString() + "  " + dir + "  " + port + "  " + hex(data);
    box.prepend(line);
    while (box.childElementCount > 40) box.lastChild.remove();
  }
  function names(map) { return Array.from(map.values(), p => p.name); }
  function send(obj) { if (ws && ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify(obj)); }
  function sendPorts() {
    const outputs = names(access.outputs), inputs = names(access.inputs);
    send({ type: "ports", outputs, inputs });
    $("ob-connect-ports").textContent = outputs.length || inputs.length
      ? "Devices: " + Array.from(new Set(outputs.concat(inputs))).join(", ")
      : "No MIDI device found: plug one in, it will appear here.";
  }
  function listen() {
    access.inputs.forEach(input => {
      input.onmidimessage = e => {
        if (e.data[0] === 0xF8 || e.data[0] === 0xFE) return; // clock, active sensing
        send({ type: "midi_in", port: input.name, data: Array.from(e.data) });
        log("←", input.name, e.data);
      };
    });
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
        send({ type: "share", enabled: $("ob-connect-share").checked });
        sendPorts();
      } else if (msg.type === "midi_out") {
        const out = Array.from(access.outputs.values()).find(o => o.name === msg.port);
        if (!out) return log("✕", msg.port, msg.data);
        try { out.send(msg.data); log("→", msg.port, msg.data); }
        catch (e) { log("✕ " + e.message, msg.port, msg.data); }
      } else if (msg.type === "error") {
        status("Relay error: " + msg.error);
      }
    };
    ws.onclose = () => {
      status("Disconnected. Press the button to start a new session (the code will change).");
      $("ob-connect-paired").hidden = true;
      $("ob-connect-go").disabled = false;
    };
  }

  $("ob-connect-go").addEventListener("click", async () => {
    if (!navigator.requestMIDIAccess) {
      status("This browser gives no MIDI access to web pages (Safari, iOS). Use Chrome or Edge, or osc-bridge connect.");
      return;
    }
    $("ob-connect-go").disabled = true;
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
  $("ob-connect-share").addEventListener("change", e => send({ type: "share", enabled: e.target.checked }));
  root.querySelectorAll(".ob-connect-copy").forEach(b => b.addEventListener("click", () => {
    const text = $(b.dataset.copy).textContent;
    if (navigator.clipboard) navigator.clipboard.writeText(text).then(() => { b.textContent = "copied"; });
  }));
})();
