<!--
Use this template when promoting a device from 📄 (doc-derived) to ✅
(hardware-verified). Open the PR with ?template=promote_to_verified.md
or paste this block manually.
-->

## Promote `<Vendor> <Device>` → hardware-verified

### Test environment

- Firmware / OS version on the device:
- Host OS + osc-bridge version:
- MIDI interface (USB direct / DIN via interface / …):

### Routes exercised

<!--
Paste a log of OSC messages you sent and the MIDI bytes / OSC replies
you observed. A short text log is fine — what matters is that every
command, param, and reply in the JSON was actually hit.
-->

```
<osc-send transcript>
```

### Corrections applied

<!--
List anything in the doc-derived JSON that was wrong and you fixed:
- wrong CC number
- inverted range
- typo in OSC name
- missing reply pattern
- opcode that silently did nothing
-->

### Still not tested

<!--
Anything you couldn't realistically exercise (e.g. "bitmap upload —
we built the 1216-byte frame but didn't visually confirm the pixels").
Being explicit here keeps the ✅ marker honest.
-->

### Checklist

- [ ] Every command / param / reply in the JSON was sent and the observed behaviour matches the spec (or the JSON was corrected).
- [ ] `README.md` status marker flipped to ✅.
- [ ] `docs/SUPPORTED_DEVICES.md` section updated (source line amended to *Hardware-verified by @you on <date>*).
- [ ] `cargo test --release` passes.
