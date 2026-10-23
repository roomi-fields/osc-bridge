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

`osc-bridge mcp --offline` or `OSC_BRIDGE_OFFLINE=1` cuts the link: nothing is
sent, and the catalogue bundled with your copy is used as is.

## The hosted server and the relay

What they record — accounts, tool calls, reports, the device identities seen
through a relay when you allow it — is described in the
[API for agents](https://osc-bridge.roomi-fields.com/api/) page.

**Contact:** https://github.com/roomi-fields/osc-bridge/issues
