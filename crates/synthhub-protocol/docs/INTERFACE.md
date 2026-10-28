# synthhub-protocol — interface

`synthhub-protocol` offers the contract between the SyntHub service and its clients as Rust types, one module per API version. The list below is the target of the core epic; once the crate exists, the part between the generated markers is drawn from its rustdoc comments.

## Overview

| Export | Shape | Carries |
|---|---|---|
| `v1::*` | request and response types of API version 1 | catalogue, trust, detection, sounds, librarian, contributions, account |
| `relay::Frame` | enum of relay frames | `hello`, `ports`, `midi_out`, `midi_in`, `confirm`, `confirm_reply`, `confirm_cancel` |
| `identity::Installation` | an installation id issued by the service | — |
| `identity::ClientVersion` | the client's name and version, sent with each call | — |
| `Fault` | typed fault at deserialisation | — |

The TypeScript types of the site's interface are generated from these types.

## The faults

- `Shape` — a value does not match its type.
- `UnknownVersion` — an API version this build does not define.

## The guard

- `cargo-semver-checks` on each release: a published version's types do not change.
- Requests recorded from published clients deserialise into the current types.
- The service and osc-bridge depend on this crate; neither defines the contract elsewhere.

## Consumers

- `osc-bridge`: `v1`, `relay`, `identity`, `Fault`
- the SyntHub service: `v1`, `relay`, `identity`, `Fault`
- the SyntHub site: `v1`, `relay`, `identity`

<!-- généré: début -->
<!-- généré: fin -->
