# blofeld_sound_dump.hex

One Waldorf Blofeld Sound Dump (392 bytes, `F0 3E 13 00 10 00 00 … 3E F7`), written as hex.

- Source: the first message of `Waldorf_Blofeld_Blo Factory_2008.syx`, the Blofeld factory sounds as dumped.
- Origin: KnobKraft Orm, `adaptations/testData/` (https://github.com/christofmuc/KnobKraft-orm).
- Author: Christof Ruch. Licence: AGPL-3.0, compatible with osc-bridge (AGPL-3.0-or-later).
- Checksum: byte 390 = 0x3E = (sum of bytes 7..389) & 0x7F, the "sum of SDATA" of the Blofeld SysEx
  documentation v1.04 (https://www.deepsonic.ch/deep/docs_manuals/waldorf_blofeld_sysex_documentation_v.1.04.txt).
