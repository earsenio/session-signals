// Propagate the canonical app version (package.json) into the Rust crate.
//
// package.json is the single source of truth. Tauri reads the installer version
// straight from it (tauri.conf.json `version` points at "../package.json"), but
// Cargo.toml carries its own copy for the crate, so we keep that in sync here.
//
// Run automatically by the npm `version` lifecycle hook (see package.json), i.e.
// on every `npm version <patch|minor|major>` / `npm run release:*`. Can also be
// run by hand: `node scripts/sync-version.mjs`.

import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");

const version = JSON.parse(readFileSync(resolve(root, "package.json"), "utf8")).version;

// Releases-only policy: the version must be plain X.Y.Z. The Windows MSI/NSIS
// ProductVersion is numeric and silently strips pre-release tags, so a `-beta.N`
// here would mean the two platforms ship under different versions. Refuse early.
if (!/^\d+\.\d+\.\d+$/.test(version)) {
  console.error(
    `sync-version: package.json version "${version}" is not plain X.Y.Z.\n` +
      "Beacon is releases-only; pre-release/build tags are not supported.",
  );
  process.exit(1);
}

const cargoPath = resolve(root, "src-tauri/Cargo.toml");
const cargo = readFileSync(cargoPath, "utf8");

// Only the [package] version sits at column 0 (`version = "..."`); dependency
// versions are inline tables or `name = "x"`, never line-anchored like this.
const next = cargo.replace(/^version = ".*"$/m, `version = "${version}"`);
if (next === cargo) {
  if (cargo.includes(`version = "${version}"`)) {
    console.log(`sync-version: Cargo.toml already at ${version}`);
    process.exit(0);
  }
  console.error("sync-version: could not find a [package] version line in Cargo.toml");
  process.exit(1);
}

writeFileSync(cargoPath, next);
console.log(`sync-version: Cargo.toml → ${version}`);
