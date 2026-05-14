# MCP directory submissions — osc-bridge

State tracker for getting osc-bridge discoverable across the MCP ecosystem.
Per the PLAYBOOK, the growth engine is **MCP directories + Google SEO**, not
social. This file tracks the directory half.

**Package**: `@roomi-fields/osc-bridge` (npm) · `io.github.roomi-fields/osc-bridge` (registry)
**Positioning everywhere**: *OSC ↔ MIDI bridge for 849 synths and DAWs — a MIDI MCP and OSC MCP.*
**Last sync**: 2026-05-14

Legend: ✅ live · ⚠️ submitted, pending · 🔄 auto-indexing in progress · ❌ to do · 🚫 out of scope

---

## Tier 0 — the roots (everything else indexes off these)

| Channel | Status | Notes |
|---|---|---|
| npm — `@roomi-fields/osc-bridge` | ✅ | v0.10.0 published 2026-05-14 |
| Official MCP Registry — `io.github.roomi-fields/osc-bridge` | ✅ | published 2026-05-14, status `active` |
| GitHub repo metadata | ✅ | public; About description + homepage (Pages site) set to the 3-strata framing; topics include `mcp`, `mcp-server`, `model-context-protocol`, `midi`, `osc`, `claude`, `ableton`, `bitwig`, `reaper`, `daw`, `live-coding` (20-topic cap reached — dropped the long-tail vendor topics) |
| `roomi-fields/claude-plugins` marketplace | ✅ | entry added 2026-05-14 |
| Pages site (GitHub Pages) | ✅ | device browser + SEO/JSON-LD live |

## Tier 1 — auto-indexed (verify, fix metadata if wrong)

Most of these crawl npm + the Official Registry. Action = confirm we appear
and that the description/keywords landed correctly.

| Directory | Status | Notes |
|---|---|---|
| Glama.ai | 🔄 | auto-discovers from the registry; verify the page + the A/A/A quality badges build |
| PulseMCP | 🔄 | auto-indexed from the registry |
| mcpservers.org | 🔄 | auto-indexed |
| LobeHub | 🔄 | auto-indexed |
| MCPMarket.com | 🔄 | auto-indexed |

## Tier 2 — manual, high value

| Directory | Status | Action |
|---|---|---|
| Smithery.ai | ✅ | `roomifields/osc-bridge` published 2026-05-14 as a stdio MCPB bundle (`npx -y @roomi-fields/osc-bridge mcp`); 5 tools + `default_target` config indexed; description + icon set via the registry API |
| awesome-mcp-servers (punkpeye, ~80k★) | ⚠️ | PR #6342 open — entry under "Multimedia Process" |
| Cline Marketplace | ⚠️ | issue #1561 open on cline/mcp-marketplace |
| awesome-mcp-servers (wong2) | ❌ | PR |
| awesome-mcp-servers (appcypher) | ❌ | PR |
| Cursor Directory | ❌ | web form (~2 min) |
| FindMCP.dev | ❌ | web form |

## Tier 3 — manual, low effort (long tail)

| Directory | Status |
|---|---|
| mcp.directory | ✅ submitted by maintainer |
| mcp.so | 🔄 in progress (maintainer) |
| MCPIndex.net | ❌ |
| MCPList.ai | ❌ |
| Windsurf Directory | ❌ |

## To evaluate

| Channel | Note |
|---|---|
| Docker MCP Catalog | Would need a published Docker image — out of scope unless we ship one. |

---

## Priority order

1. ~~Add GitHub topics~~ — ✅ done 2026-05-14.
2. ~~Smithery~~ — ✅ done 2026-05-14.
3. **Verify Tier 1 auto-indexers** picked us up correctly (description, keywords) —
   give them a few days after the registry publish, then check.
4. **Cursor Directory + FindMCP** — quick web forms.
5. **Track PR #6342 (punkpeye) + issue #1561 (Cline)** — respond to maintainer
   feedback if any; the music/audio positioning needs a clear category pitch
   since we're not the typical "knowledge" or "dev-tools" MCP.
6. **awesome-mcp-servers PRs (wong2, appcypher)** — remaining high-traffic lists.

## Monitoring cadence (PLAYBOOK §6.2)

- **Weekly**: GitHub Insights → Traffic (referrers = ground truth).
- **Weekly**: npm downloads (npm-stat.com).
- **Monthly**: star history.
- Ignore clone counts — CI runners inflate them.
