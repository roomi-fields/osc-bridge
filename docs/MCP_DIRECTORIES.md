# MCP directory submissions — osc-bridge

State tracker for getting osc-bridge discoverable across the MCP ecosystem.
Per the PLAYBOOK, the growth engine is **MCP directories + Google SEO**, not
social. This file tracks the directory half.

**Package**: `@roomi-fields/osc-bridge` (npm) · `io.github.roomi-fields/osc-bridge` (registry)
**Positioning everywhere**: *OSC ↔ MIDI bridge for 981 synths and DAWs — a MIDI MCP and OSC MCP.*
**Hosted endpoint first** (since 2026-10-23): every listing leads with the hosted server,
`https://mcp.synthhub.roomi-fields.com/mcp` (SynthHub), the local copy (`npx`) second. No
copies hosted by third parties: agents should reach the hosted server itself.
**Last sync**: 2026-10-23

Legend: ✅ live · ⚠️ submitted, pending · 🔄 auto-indexing in progress · ❌ to do · 🚫 out of scope

---

## Tier 0 — the roots (everything else indexes off these)

| Channel | Status | Notes |
|---|---|---|
| npm — `@roomi-fields/osc-bridge` | ✅ | v0.16.0 (2026-10-24), catalogue bundled, README leads with the hosted endpoint; published by npm trusted publishing from `ci.yml` (no token) |
| Official MCP Registry — `io.github.roomi-fields/osc-bridge` | ✅ | v0.16.0 published 2026-10-24 by the release job (OIDC), remote `mcp.synthhub.roomi-fields.com/mcp` |
| GitHub repo metadata | ✅ | public; About description + homepage (Pages site) set to the 3-strata framing; topics include `mcp`, `mcp-server`, `model-context-protocol`, `midi`, `osc`, `claude`, `ableton`, `bitwig`, `reaper`, `daw`, `live-coding` (20-topic cap reached — dropped the long-tail vendor topics) |
| `roomi-fields/claude-plugins` marketplace | ✅ | entry added 2026-05-14 |
| Site — synthhub.roomi-fields.com | ✅ | built from SynthHub since 2026-10-23: a page per device, 10 articles, JSON-LD, llms.txt; GitHub Pages is a pointer page |

## Tier 1 — auto-indexed (verify, fix metadata if wrong)

Most of these crawl npm + the Official Registry. Action = confirm we appear
and that the description/keywords landed correctly.

| Directory | Status | Notes |
|---|---|---|
| Glama.ai | ⚠️ | listed, no quality score ("cannot be deployed"), which blocks punkpeye. A Glama-hosted copy is not pursued: agents should reach the hosted server itself. `glama.json` (maintainers only, 2026-10-23) claims the listing to keep its text right; if their build stalls, clear the Python version they infer |
| PulseMCP | 🔄 | auto-indexed from the registry |
| mcpservers.org | 🔄 | auto-indexed |
| LobeHub | 🔄 | auto-indexed |
| MCPMarket.com | 🔄 | auto-indexed |

## Tier 2 — manual, high value

| Directory | Status | Action |
|---|---|---|
| Smithery.ai | ⚠️ | `roomifields/osc-bridge` published 2026-05-14 as a stdio MCPB bundle (quality score 100%). To do: add the hosted endpoint as the listing's remote URL, ahead of the bundle |
| awesome-mcp-servers (punkpeye, ~80k★) | ❌ | PR #6342 closed for inactivity — only missing a Glama quality score. Reopen once Glama has scored the server |
| awesome-mcp-servers (TensorBlock, ~680★) | ✅ | PR #543 merged 2026-05-18 |
| Cline Marketplace | ⚠️ | issue #1561 open, no answer since May |
| ChatForest review (music MCP servers) | ⚠️ | issue thunderrabbit/chatforest.com#2 opened 2026-10-23 |
| awesome-mcp-servers (wong2, ~4k★) | 🚫 | no PR / no issue — submit via web form at mcpservers.org/submit |
| awesome-mcp-servers (appcypher, ~5.5k★) | 🚫 | submissions closed — PRs **and** issues both disabled, maintainer-only list |
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
3. ~~PR-able awesome lists~~ — ✅ punkpeye #6342 + TensorBlock #543 done. wong2
   and appcypher have no PR/issue path (web form / closed) — not actionable.
4. **Hosted endpoint first in every listing** — our own pages done 2026-10-23 (README,
   npm README, MCP guide, site); Smithery: remote URL to add; web forms below: submit
   the hosted URL, not the npm command.
5. **Verify Tier 1 auto-indexers** picked us up correctly (description, keywords) —
   give them a few days after the registry publish, then check.
6. **Web forms (need a human)**: wong2 (mcpservers.org/submit), Cursor Directory,
   FindMCP.dev, MCPIndex.net, MCPList.ai, Windsurf.
7. **Track PR #6342 (punkpeye) + PR #543 (TensorBlock) + issue #1561 (Cline)** —
   respond to maintainer feedback if any.

## Monitoring cadence (PLAYBOOK §6.2)

- **Weekly**: GitHub Insights → Traffic (referrers = ground truth).
- **Weekly**: npm downloads (npm-stat.com).
- **Monthly**: star history.
- Ignore clone counts — CI runners inflate them.
