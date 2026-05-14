#!/usr/bin/env bash
# Demo helper for docs/demo/osc-bridge-demo.tape — asks the osc-bridge MCP
# server for its tool list over stdio and prints it compactly.
# Assumes cwd is the repo root and `osc-bridge` is on PATH.
set -euo pipefail

echo '{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}' \
  | osc-bridge mcp --devices-dir devices 2>/dev/null \
  | python3 -c "import sys,json; [print('  - '+t['name']) for t in json.load(sys.stdin)['result']['tools']]"
