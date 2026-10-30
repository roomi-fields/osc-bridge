# Privacy Policy — osc-bridge

_Last updated: 2026-10-27_

osc-bridge drives your music hardware and software from your own machine.
MIDI and OSC traffic to your devices never leaves your machine or local
network, except through the relay when you open it yourself (below).

## A copy installed on your machine (`osc-bridge mcp`)

By default it is linked to the SynthHub server
(`mcp.synthhub.roomi-fields.com`), which holds the catalogue of drivers, as
other users and agents fixed and confirmed them:

- **At startup** it fetches the trust of each driver.
- **When a driver is needed** (to run a device, read its routes or notes,
  report on it), it fetches that one driver, by its id or slug, and keeps it in
  a cache on your machine (`<cache>/osc-bridge/drivers`: `~/.cache` on Linux,
  `%LOCALAPPDATA%` on Windows). **Detecting your hardware**
  (`osc-bridge detect`, `detect_hardware`) first asks the server which
  identity requests to send, then sends it the names of your MIDI ports and
  what your devices answered them (maker, model, firmware, and any other
  SysEx a device sent while being asked), so the server names the matching
  drivers; **listing devices** sends the search words and filters. These
  requests carry no account. Of detection and of driver fetches the server
  keeps counters only, with no visitor, account, address or time of day
  attached: per driver, how many times it was fetched each day; per device
  name asked for, how many times it was found or missing; per MIDI port
  name (lower case, without its numbers), how many times it was seen,
  whether the device identified itself, and which driver came first, or
  none. It learns which drivers are used and which devices are missing,
  never who uses what. Its general request log names only the kind of
  request (`/api/v1/devices/…`, `/v1/devices/…`, `/v1/detect`, a search),
  never the driver, device or ports. A search is different: it is kept with
  its words and filters, tied to the visitor, never to your address. What
  the server keeps of the calls your copy forwards, and for how long, is
  written under *Visits and activity* below.
- **An anonymous account** is issued by the server the first time your agent
  reports or contributes, and kept on your machine in `<config>/osc-bridge/user`.
- **When your agent reports or contributes** (`report_result`,
  `report_mapping`, `submit_driver`, `submit_fix`), what it sends goes to the
  server under that account, to improve the catalogue for everyone.
- **When your agent shares a preset** (`share_preset`, only a sound you
  made, after asking you), it is published under CC BY-SA 4.0 with a
  pseudonym for its author — never your account id. You can withdraw it
  (`withdraw_preset`).
- **Never sent:** your MIDI ports, your devices' traffic, your files. Your
  address is never kept: the server records a fingerprint of it instead
  (see *Visits and activity* below).

Sharing is on by default, and you choose what to keep to yourself in a
settings file, `<config>/osc-bridge/settings.toml` (`~/.config` on Linux,
`%APPDATA%` on Windows). It is created, with comments, the first time
osc-bridge starts; edit it with any text editor:

```toml
# true: do not contact the server at all (drivers already cached only, nothing sent).
offline = false

# Devices kept private: nothing about them is sent to the server.
private = [
  "korg/minilogue",   # one driver (and the other variants of that device)
  "minilab3",         # a device by its slug
  "elektron/*",       # a whole vendor
]
```

- `offline = true` cuts the link: nothing is sent, and only the drivers
  already in the cache are used. For a single run: `osc-bridge mcp --offline`
  or `OSC_BRIDGE_OFFLINE=1`.
- A private device: its reports, learned mappings, fixes, presets and trust
  lookups stay on your machine. A device named as the list names it is not
  fetched by that name either; its driver comes from the cache (or a file of
  yours) — fetching it again means asking the server for it, without your
  account.

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
in for 30 days. You choose what your author page shows — a display name, a
link, a few words, a picture — also shown with what you share. Deleting your account removes all of this; what you shared stays
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

## Visits and activity

To know how SynthHub is used and to notice problems, the server records each
request it answers, each page of the site viewed, each sign-in, each relay
opened and closed, and each tool call: the time, what was asked, the
answer's status, the browser or program's name, and — for page views — the
page, the site you came from (its address only), your language and screen
size. A tool call your copy forwards (a tool call of your agent) is kept
with its outcome, its duration and, when it failed, the error message,
shortened to its first characters; a search is kept with its words and
filters. This holds on every surface, with or without an account: a copy
that sends no account is recorded too. Each of these records is tied to the
visitor — the fingerprint below — and never to your address. You appear in
it only as a **fingerprint of your address**, computed
with a secret key kept on the server: the same address always gives the same
fingerprint, so visits that come back can be counted, but the address itself
is never written and cannot be read back from it. An IPv6 address is
fingerprinted by its network (its first half), which a home's devices share. The site sets no cookie for
this and keeps nothing in your browser. These records stay on the server and
are read only by its administrator.

The server keeps this detail (each call, search and visit, with its visitor)
for 6 months, then deletes it. Beyond that only each day's totals remain,
with no visitor and no account, the words of the searches included.

What the server records — accounts, tool calls, reports, device identities —
is described in the
[API for agents](https://synthhub.roomi-fields.com/api/) page.

**Contact:** https://github.com/roomi-fields/osc-bridge/issues
