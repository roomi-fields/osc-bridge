#!/usr/bin/env node
// Thin shim: exec the platform-native `osc-bridge` binary that
// scripts/postinstall.js downloaded into vendor/, forwarding argv + stdio.
//
// The MCP server is the `mcp` subcommand:
//   npx -y @roomi-fields/osc-bridge mcp
"use strict";

const { spawnSync } = require("node:child_process");
const { existsSync } = require("node:fs");
const { join } = require("node:path");

const ext = process.platform === "win32" ? ".exe" : "";
const bin = join(__dirname, "..", "vendor", "osc-bridge" + ext);

if (!existsSync(bin)) {
  console.error(
    "[osc-bridge] native binary not found at:\n  " +
      bin +
      "\n" +
      "The postinstall download likely failed. Re-run `npm install`, or grab\n" +
      "the binary from https://github.com/roomi-fields/osc-bridge/releases"
  );
  process.exit(1);
}

const res = spawnSync(bin, process.argv.slice(2), { stdio: "inherit" });
if (res.error) {
  console.error("[osc-bridge] failed to launch native binary: " + res.error.message);
  process.exit(1);
}
process.exit(res.status === null ? 1 : res.status);
