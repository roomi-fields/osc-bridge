# Privacy Policy — osc-bridge

_Last updated: 2026-10-23_

osc-bridge drives your music hardware and software from your own machine.
MIDI and OSC traffic to your devices never leaves your machine or local
network, except through the relay when you open it yourself (below).

## A copy installed on your machine (`osc-bridge mcp`)

By default it is linked to the SynthHub server
(`mcp.synthhub.roomi-fields.com`), so it stays up to date with the drivers
other users and agents fixed and confirmed:

- **At startup** it fetches the list of the server's driver files and the
  trust of each driver, then downloads the files that changed. The server
  records the time, an anonymous account id it issued (kept on your machine in
  `<config>/osc-bridge/user`) and the osc-bridge version.
- **When your agent reports or contributes** (`report_result`,
  `report_mapping`, `submit_driver`, `submit_fix`), what it sends goes to the
  server under that account, to improve the catalogue for everyone.
- **When your agent shares a preset** (`share_preset`, only a sound you
  made, after asking you), it is published under CC BY-SA 4.0 with a
  pseudonym for its author — never your account id. You can withdraw it
  (`withdraw_preset`).
- **Never sent:** your MIDI ports, your devices' traffic, your files, your
  address (beyond what any web server sees for a few days in its access log).

Sharing is on by default, and you choose what to keep to yourself in a
settings file, `<config>/osc-bridge/settings.toml` (`~/.config` on Linux,
`%APPDATA%` on Windows). It is created, with comments, the first time
osc-bridge starts; edit it with any text editor:

```toml
# true: do not contact the server at all (bundled catalogue, nothing sent).
offline = false

# Devices kept private: nothing about them is sent to the server.
private = [
  "korg/minilogue",   # one driver (and the other variants of that device)
  "minilab3",         # a device by its slug
  "elektron/*",       # a whole vendor
]
```

- `offline = true` cuts the link: nothing is sent, and the catalogue bundled
  with your copy is used as is. For a single run: `osc-bridge mcp --offline`
  or `OSC_BRIDGE_OFFLINE=1`.
- A private device: its reports, learned mappings, fixes, presets and trust
  lookups stay on your machine, while the catalogue and the trust fetched at startup
  stay up to date.

`osc-bridge privacy` shows the current settings and where the file is; it can
change them too (`--offline on|off`, `--private <device>`, `--public
<device>`).

## Your account on the site

On synthhub.roomi-fields.com you can sign in — with a passkey, a link sent by
e-mail, Google or Discord — to manage what you and your agents share. No
password is kept, and no e-mail address: for e-mail only a fingerprint of the
address (to recognise you next time; the link itself is sent and forgotten),
for Google and Discord only your account number with them (nothing else is
asked for), for a passkey its public key. A session cookie keeps you signed
in for 30 days. You choose a display name and a link shown with what you
share. Deleting your account removes all of this; what you shared stays
published under its licence, under your pseudonym, unless you withdraw it.

## The hosted server and the relay

Through the relay (the `/connect` page or `osc-bridge connect`), the identity
(model, firmware) of the devices your agent detects is recorded by default,
to improve the catalogue. Untick *Share the identity of my devices* (or
`osc-bridge connect --no-share`) to record none, or keep one device private —
the *private* box next to it on the page, `--private-port <name>` on the
command line. MIDI messages passing through the relay are never recorded,
except a patch your agent captures to share as a preset (`share_preset`,
after asking you) — never from a device kept private.

What the server records — accounts, tool calls, reports, device identities —
is described in the
[API for agents](https://synthhub.roomi-fields.com/api/) page.

**Contact:** https://github.com/roomi-fields/osc-bridge/issues
