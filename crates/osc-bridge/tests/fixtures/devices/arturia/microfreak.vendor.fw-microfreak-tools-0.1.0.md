<!-- imported README: https://github.com/gbryant/microfreak-tools/blob/396fcf46090691f82560ddbe7deaa94d9bf5272f/README.md -->
> From the README of [gbryant/microfreak-tools](https://github.com/gbryant/microfreak-tools) at [396fcf4](https://github.com/gbryant/microfreak-tools/blob/396fcf46090691f82560ddbe7deaa94d9bf5272f/README.md), licence Apache-2.0.

# microfreak-tools

Command-line tools for the Arturia MicroFreak. Read and write presets, samples,
wavetables and settings over MIDI, and patch and flash firmware. Plugins add
custom firmware and the commands that use it, such as putting your own words
on the Speech engine.

Not affiliated with, authorised by or endorsed by Arturia. No Arturia code or
binaries are in this repository. Commands that need firmware take the path to
an image you already own.

## Install

    uv tool install microfreak-tools     # puts `mfreak` on PATH

With the plugins, add a `--with` for each:

    uv tool install microfreak-tools --with mfreak-speech --with mfreak-peek

To upgrade, `uv tool upgrade microfreak-tools`. With pipx, `pipx install
microfreak-tools`, then `pipx inject microfreak-tools mfreak-speech` for each
plugin.

From a clone, `uv run mfreak doctor` runs it without installing, with both
plugins.

Reading audio files needs `ffmpeg` (`brew install ffmpeg`). Recording and USB
support come with the package; on Linux, recording also needs the system
`libportaudio2`. `mfreak doctor` reports anything missing.

[Read the whole README on GitHub](https://github.com/gbryant/microfreak-tools/blob/396fcf46090691f82560ddbe7deaa94d9bf5272f/README.md)
