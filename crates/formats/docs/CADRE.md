# formats — frame

## Role

- **R1** `formats` writes a resolved device in other formats and reads other formats into OSD: MIDNAM and OSCQuery first, then Electra One presets, Ctrlr panels and instrument definition files.

## Receives

- **R2** A resolved device from `osd`, and the target format; or the text of a file in another format.

## Returns

- **R3** The text of the exported file, with each source's credit and licence; or an OSD device descriptor, with what the source format could not say listed.

## Knows

- **R4** The `osd` types, and each format's specification.

## Does not know

- **R5** Sessions, ports, the network, the service, where files are stored.

## Refuses

- **R6** Exporting a device whose sources forbid sharing in that format: a typed fault naming the source.
- **R7** A file that is not valid in its format.

## Invariants

- **R8** Every export is valid against its format's own schema or DTD.
- **R9** What an import produces is a valid OSD descriptor; a round trip (OSD → format → OSD) keeps every field the format can hold.

## Cost

- **R10** Under 10 ms per device and format.
