# Privacy Policy — osc-bridge

_Last updated: 2026-10-23_

osc-bridge drives your music hardware and software from your own machine.
MIDI and OSC traffic to your devices never leaves your machine or local
network, except through the relay when you open it yourself (below).

## A copy installed on your machine (`osc-bridge mcp`)

By default it is linked to the osc-bridge server
(`mcp.osc-bridge.roomi-fields.com`), so it stays up to date with the drivers
other users and agents fixed and confirmed:

- **At startup** it fetches the list of the server's driver files and the
  trust of each driver, then downloads the files that changed. The server
  records the time, an anonymous account id it issued (kept on your machine in
  `<config>/osc-bridge/user`) and the osc-bridge version.
- **When your agent reports or contributes** (`report_result`,
  `report_mapping`, `submit_driver`, `submit_fix`), what it sends goes to the
  server under that account, to improve the catalogue for everyone.
- **Never sent:** your MIDI ports, your devices' traffic, your files, your
  address (beyond what any web server sees for a few days in its access log).

Sharing is on by default, and you choose what to keep to yourself with
`osc-bridge privacy` (it shows the current state when run alone):

- `osc-bridge privacy --offline on` cuts the link for good: nothing is sent,
  and the catalogue bundled with your copy is used as is (`--offline off`
  links it again). For a single run: `osc-bridge mcp --offline` or
  `OSC_BRIDGE_OFFLINE=1`.
- `osc-bridge privacy --private <device>` keeps one device private: the
  reports, learned mappings and fixes about it stay on your machine, while
  the catalogue and its trust stay up to date. `--public <device>` shares it
  again.

The choices are kept in `<config>/osc-bridge/settings.toml`.

## The hosted server and the relay

Through the relay (the `/connect` page or `osc-bridge connect`), the identity
(model, firmware) of the devices your agent detects is recorded by default,
to improve the catalogue. Untick *Share the identity of my devices* (or
`osc-bridge connect --no-share`) to record none, or keep one device private —
the *private* box next to it on the page, `--private-port <name>` on the
command line. MIDI messages passing through the relay are never recorded.

What the server records — accounts, tool calls, reports, device identities —
is described in the
[API for agents](https://osc-bridge.roomi-fields.com/api/) page.

**Contact:** https://github.com/roomi-fields/osc-bridge/issues
