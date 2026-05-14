#!/usr/bin/env node
// Downloads the platform-native `osc-bridge` binary from the GitHub release
// matching this package's version, into vendor/. Runs automatically on
// `npm install`.
//
// Asset names must stay in sync with .github/workflows/ci.yml (the release job).
"use strict";

const { createWriteStream, mkdirSync, chmodSync, existsSync, unlinkSync } = require("node:fs");
const { join } = require("node:path");
const https = require("node:https");

const pkg = require("../package.json");
const VERSION = pkg.version;
const REPO = "roomi-fields/osc-bridge";

// `${process.platform}-${process.arch}` -> release asset file name.
const ASSETS = {
  "linux-x64": "osc-bridge-linux-x86_64",
  "win32-x64": "osc-bridge-windows-x86_64.exe",
  "darwin-arm64": "osc-bridge-macos-arm64",
  "darwin-x64": "osc-bridge-macos-x86_64",
};

function fail(msg) {
  console.error("[osc-bridge] " + msg);
  process.exit(1);
}

const key = process.platform + "-" + process.arch;
const asset = ASSETS[key];
if (!asset) {
  fail(
    "no prebuilt binary for platform '" +
      key +
      "'.\n" +
      "Covered: linux-x64, win32-x64, darwin-arm64, darwin-x64.\n" +
      "Build from source instead: https://github.com/" + REPO
  );
}

const url =
  "https://github.com/" + REPO + "/releases/download/v" + VERSION + "/" + asset;
const ext = process.platform === "win32" ? ".exe" : "";
const vendorDir = join(__dirname, "..", "vendor");
const dest = join(vendorDir, "osc-bridge" + ext);

if (existsSync(dest)) {
  // Already present (e.g. a repeat install) — keep it.
  process.exit(0);
}
mkdirSync(vendorDir, { recursive: true });

function download(u, redirectsLeft) {
  https
    .get(u, { headers: { "User-Agent": "osc-bridge-npm-postinstall" } }, (res) => {
      if (
        res.statusCode >= 300 &&
        res.statusCode < 400 &&
        res.headers.location
      ) {
        if (redirectsLeft <= 0) fail("too many redirects fetching " + u);
        res.resume();
        return download(res.headers.location, redirectsLeft - 1);
      }
      if (res.statusCode !== 200) {
        fail(
          "download failed (HTTP " +
            res.statusCode +
            ") for:\n  " +
            u +
            "\nIs release v" +
            VERSION +
            " published with the binary assets attached?"
        );
      }
      const out = createWriteStream(dest, { mode: 0o755 });
      res.pipe(out);
      out.on("finish", () => {
        out.close(() => {
          try {
            chmodSync(dest, 0o755);
          } catch (_) {
            /* best-effort; Windows ignores the mode */
          }
          console.log("[osc-bridge] installed " + asset + " -> " + dest);
        });
      });
      out.on("error", (e) => {
        try {
          unlinkSync(dest);
        } catch (_) {}
        fail("write error: " + e.message);
      });
    })
    .on("error", (e) => fail("network error: " + e.message));
}

download(url, 5);
