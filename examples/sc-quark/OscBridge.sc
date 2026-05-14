// OscBridge — SuperCollider convenience wrapper for the osc-bridge daemon.
//
// Usage:
//   ~mb = OscBridge("minilab3", NetAddr("127.0.0.1", 7777));
//   ~mb.display("Hello", "World");
//   ~mb.padColor(0, 127, 0, 0);
//   ~mb.param("/knob/3/cc_number", 100);
//
// Incoming MIDI events must be routed from osc-bridge to this SC instance via
// the `--osc-client 127.0.0.1:57120` flag on `osc-bridge run`.

OscBridge {
    var <prefix;       // e.g. "/minilab3"
    var <netAddr;
    var <>responders;  // dict of OSCdefs keyed by user tag

    *new { |deviceTag = "minilab3", netAddr|
        ^super.new.init(deviceTag, netAddr);
    }

    init { |tag, na|
        prefix = "/" ++ tag;
        netAddr = na ? NetAddr("127.0.0.1", 7777);
        responders = IdentityDictionary.new;
    }

    // --- send helpers ---

    send { |route ... args|
        netAddr.sendMsg(*([prefix ++ route] ++ args));
    }

    raw { |hexString|
        this.send("/raw/syx", hexString);
    }

    param { |route, value|
        this.send(route, value.asInteger);
    }

    display { |l1 = "", l2 = ""|
        this.send("/display/text", l1.asString, l2.asString);
    }

    icons { |p1 = \none, p2 = \none, l1 = "", l2 = ""|
        this.send("/display/icons", p1.asString, p2.asString, l1.asString, l2.asString);
    }

    padColor { |pad, r, g, b, mode = \user|
        this.send("/pad/" ++ pad ++ "/color",
            r.asInteger, g.asInteger, b.asInteger, mode.asString);
    }

    recallPreset { |id| this.send("/preset/recall", id.asInteger); }
    storePreset  { |id| this.send("/preset/store",  id.asInteger); }

    init_ { this.send("/init"); }

    // --- receive helpers ---
    // Attach a closure to any sub-path; returns the OSCdef for later removal.

    on { |subPath, func|
        var path = (prefix ++ subPath).asSymbol;
        var key = (prefix ++ subPath).asSymbol;
        ^OSCdef(key, func, path);
    }

    off { |subPath|
        OSCdef((prefix ++ subPath).asSymbol).free;
    }
}
