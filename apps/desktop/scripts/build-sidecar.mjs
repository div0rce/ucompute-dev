#!/usr/bin/env node
// Builds the Common daemon from THIS checkout, stages it as the Tauri sidecar, and checks that it accepts
// the exact arguments the app starts it with (src-tauri/daemon-args.txt).
//
// `pnpm tauri build` runs this first (tauri.conf.json > build.beforeBuildCommand), so a bundle can never
// ship a daemon left over from an earlier commit. That happened once: an app built after
// `--exit-with-parent` was added bundled a daemon built before it, which exited with a usage error.
// Node, not sh, so the same step runs on macOS, Linux and Windows.
//
// Tauri sets TAURI_ENV_TARGET_TRIPLE for the target being bundled; without it the host triple is used.
import { execFileSync } from "node:child_process";
import { chmodSync, copyFileSync, mkdirSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const desktop = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const root = resolve(desktop, "../..");
const fail = (msg) => {
  console.error(`build-sidecar: ${msg}`);
  process.exit(1);
};

const host = execFileSync("rustc", ["-vV"], { encoding: "utf8" }).match(/^host: (.+)$/m)?.[1]?.trim();
if (!host) fail("cannot read the host triple from `rustc -vV`");
const triple = process.env.TAURI_ENV_TARGET_TRIPLE || host;
const exe = triple.includes("windows") ? ".exe" : "";

const cargo = ["build", "--release", "--locked", "-p", "common-cli", "--manifest-path", join(root, "Cargo.toml")];
if (triple !== host) cargo.push("--target", triple);
execFileSync("cargo", cargo, { stdio: "inherit" });

const built = join(root, "target", ...(triple === host ? [] : [triple]), "release", `common${exe}`);
const staged = join(desktop, "src-tauri", "binaries", `common-${triple}${exe}`);
mkdirSync(dirname(staged), { recursive: true });
copyFileSync(built, staged);
if (!exe) chmodSync(staged, 0o755);
if (!readFileSync(built).equals(readFileSync(staged))) fail(`${staged} differs from the binary just built (${built})`);
console.log(`build-sidecar: staged ${staged}`);

const args = readFileSync(join(desktop, "src-tauri", "daemon-args.txt"), "utf8").trim().split(/\s+/);
if (triple !== host) {
  console.log(`build-sidecar: cross target ${triple}; argument check skipped (cannot run it here)`);
} else {
  // clap rejects an unknown argument before it reaches --help, so this fails exactly when the app's
  // arguments would make the daemon exit with a usage error at launch.
  try {
    execFileSync(staged, [...args, "--help"], { stdio: "ignore" });
  } catch {
    fail(`the staged daemon rejects the app's arguments: ${args.join(" ")}`);
  }
  console.log(`build-sidecar: staged daemon accepts: ${args.join(" ")}`);
}
