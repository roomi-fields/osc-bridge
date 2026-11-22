#!/usr/bin/env bash
# The preparation of the CI of main (npm run ci:preparer, played by the frame's ci.yml after `npm ci`
# and before the binaries of the languages): what the tests need beyond the frame's template.
# Replayed at every job, so idempotent; the path goes to the next steps through $GITHUB_PATH.
set -euo pipefail

# The ALSA headers the MIDI backend builds against: GitHub's machines only, a runner of the project
# has them installed.
if [ "${RUNNER_ENVIRONMENT:-}" = github-hosted ]; then
  sudo apt-get update
  sudo apt-get install -y libasound2-dev
fi

# The Rust toolchain of rust-toolchain.toml (stable, clippy, rustfmt); ~/.cargo/bin on the PATH of
# the next steps.
if ! command -v rustup >/dev/null; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain none
fi
export PATH="$HOME/.cargo/bin:$PATH"
rustup show active-toolchain >/dev/null 2>&1 || rustup toolchain install --profile minimal
[ -z "${GITHUB_PATH:-}" ] || echo "$HOME/.cargo/bin" >> "$GITHUB_PATH"
