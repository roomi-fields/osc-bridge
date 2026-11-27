# Test drivers

The drivers the engine's tests load. The catalogue itself lives on the
SyntHub server; these are copies of a few of its drivers, kept as they were
when the tests were written, plus synthetic ones (`acme/`), all in OSD 0.7
(converted by `osd::convert_to`). Each keeps its `sources`, with their credit
and licence.

`../emitted_bytes.json` records, call by call, the frames each driver queued
before its conversion to 0.7: the converted drivers queue the same ones.
