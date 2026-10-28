# formats — interface

`formats` offers two operations per format: export a resolved device, import a file. The list below is the target; once the crate exists, the part between the generated markers is drawn from its rustdoc comments.

## Overview

| Export | Shape | Returns | Refuses |
|---|---|---|---|
| `export` | `fn export(device: &osd::Device, format: Format) -> Result<String, Fault>` | the file's text | sharing forbidden by a source, device without what the format needs |
| `import` | `fn import(text: &str, format: Format) -> Result<Imported, Fault>` | an OSD descriptor and what could not be carried | invalid file |
| `Format` | closed enum: `Midnam`, `OscQuery`, `ElectraOne`, `Ctrlr`, `Instrument` | — | — |
| `Imported`, `Fault` | typed values | — | — |

## The faults

- `SharingForbidden` — a source's licence forbids this export; names the source.
- `NotApplicable` — the device lacks what the format describes (an OSC-only device has no MIDNAM).
- `Invalid` — the imported file is not valid in its format.

## The guard

- Each export validated against its format's schema or DTD, over the shared drivers.
- Round trips per format.

## Consumers

- `osc-bridge`: `export`, `import`, `Format`, `Fault`
- the SyntHub service: `export`, `Format`, `Fault`
- the SyntHub tools: `import`, `Format`, `Imported`, `Fault`

<!-- généré: début -->
<!-- généré: fin -->
