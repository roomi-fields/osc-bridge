// SuperCollider demo — drive MiniLab 3 via osc-bridge
//
// Run first (in a terminal):
//   osc-bridge run --device devices/arturia/minilab3.json \
//     --out-port <Minilab3 MIDI out> --in-port <Minilab3 MIDI in> \
//     --bind 127.0.0.1:7777 --osc-client 127.0.0.1:57120
//
// Then evaluate this file in SuperCollider.

~mb = NetAddr("127.0.0.1", 7777);

// --- Display ---
~mb.sendMsg("/minilab3/display/text", "Hello", "SuperCollider");

// --- Pad colors ---
(
  8.do { |i|
    var r = 127 * (i / 7);
    var g = 127 * ((7 - i) / 7);
    ~mb.sendMsg("/minilab3/pad/%/color".format(i), r.asInteger, g.asInteger, 64);
  };
)

// --- Rainbow animation ---
(
  ~cycle = Task({
    var t = 0;
    loop {
      8.do { |i|
        var h = (t * 0.02 + i / 8.0) % 1.0;
        var r = (127 * (((h * 6 - 2).abs - 1).clip(0,1))).asInteger;
        var g = (127 * (((h * 6 - 0).abs - 1).clip(0,1))).asInteger;
        var b = (127 * (((h * 6 - 4).abs - 1).clip(0,1))).asInteger;
        ~mb.sendMsg("/minilab3/pad/%/color".format(i), r, g, b);
      };
      t = t + 1;
      0.05.wait;  // ~20 fps
    }
  }).play;
)
// ~cycle.stop;

// --- Read inbound knobs (OSC from bridge must target this SC instance on port 57120) ---
(
  OSCdef(\mb_cc, { |msg, time, addr, recvPort|
    var num = msg[0].asString.split($/).last.asInteger;
    var val = msg[1];
    var chan = msg[2];
    ("MiniLab CC" + num + "=" + val + "ch" + chan).postln;
  }, '/minilab3/cc/18'); // adapt CC number to the knob you want to listen to
)

// --- Reconfigure device params live ---
~mb.sendMsg("/minilab3/knob/3/cc_number", 100);
~mb.sendMsg("/minilab3/global/backlight", 1);

// --- Request and log current value of a param ---
(
  OSCdef(\mb_value, { |msg| ("param value:" + msg.asString).postln; },
    '/minilab3/param/value');
)
~mb.sendMsg("/minilab3/param/get", 7, 0, 0, 0);   // backlight
