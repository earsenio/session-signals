# Versioning & Releases

Session Signals uses [Semantic Versioning](https://semver.org) — `MAJOR.MINOR.PATCH` —
and ships **releases only** (plain numeric versions, no `-beta` / pre-release
tags). One version number drives both the macOS (`.app` / `.dmg`) and Windows
(NSIS `.exe`) installers.

## When to bump what

| Bump      | Command                  | Use for |
|-----------|--------------------------|---------|
| **PATCH** | `npm run release:patch`  | Backward-compatible bug fixes and polish. |
| **MINOR** | `npm run release:minor`  | New backward-compatible features — a new surface, theme, or setting. |
| **MAJOR** | `npm run release:major`  | Breaking changes to a user-facing contract: the [hook contract / listener protocol](../CLAUDE.md#hook-contract), the persisted settings/store schema (`src/state/config.ts` `version`), or removing a feature. |

**Pre-1.0 caveat:** while at `0.y.z`, a MINOR bump may carry breaking changes
(standard SemVer pre-1.0 allowance). Cut `1.0.0` once the hook contract and the
store schema are considered stable.

> Don't confuse the app version with `src/state/config.ts` `version: 1` — that is
> the **settings-schema** version (bumped only when the persisted store shape
> changes, to drive migrations). It is independent of the app version.

## Single source of truth

`package.json` `version` is canonical:

- **macOS / Windows installers** — `src-tauri/tauri.conf.json` sets
  `"version": "../package.json"`, so Tauri stamps both bundles from package.json.
- **Rust crate** — `scripts/sync-version.mjs` copies the version into
  `src-tauri/Cargo.toml` (and refreshes `Cargo.lock`). It runs automatically via
  npm's `version` lifecycle hook, and rejects any non-`X.Y.Z` version.

Never edit the version in `tauri.conf.json` or `Cargo.toml` by hand — bump
`package.json` through the release commands below and the rest follow.

## Cutting a release

```bash
npm run release:patch   # or release:minor / release:major
```

This single command:

1. bumps `package.json`,
2. runs `scripts/sync-version.mjs` → updates `Cargo.toml` + `Cargo.lock`,
3. creates the version commit and the `vX.Y.Z` git tag,
4. pushes the commit and tag (`postversion` hook).

Pushing the tag triggers `.github/workflows/release.yml`, which builds the macOS
(universal) and Windows bundles and attaches them to a **draft** GitHub Release
named `Session Signals vX.Y.Z`. Review the assets, then publish the release.

> Requires a clean working tree (`npm version` refuses otherwise). Commit or
> stash first.

## Code signing (later)

CI builds are currently **unsigned**, so macOS Gatekeeper and Windows SmartScreen
will warn on first launch. To ship signed builds, add these repo secrets and
uncomment the matching `env:` block in `.github/workflows/release.yml`:

- **macOS:** `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`,
  `APPLE_SIGNING_IDENTITY`, `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID`
  (notarization).
- **Windows:** an Authenticode certificate, referenced via
  `bundle.windows.certificateThumbprint` in `tauri.conf.json`.
