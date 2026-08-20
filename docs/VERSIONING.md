# Versioning & Releases

Session Signals uses [Semantic Versioning](https://semver.org) — `MAJOR.MINOR.PATCH` —
and ships **releases only** (plain numeric versions, no `-beta` / pre-release
tags). One version number drives both the macOS (`.app` / `.dmg`) and Windows
(NSIS `.exe`) installers.

## When to bump what

| Bump      | Command                             | Use for |
|-----------|-------------------------------------|---------|
| **PATCH** | `npm run release:prepare -- patch`  | Backward-compatible bug fixes and polish. |
| **MINOR** | `npm run release:prepare -- minor`  | New backward-compatible features — a new surface, theme, or setting. |
| **MAJOR** | `npm run release:prepare -- major`  | Breaking changes to a user-facing contract: the [hook contract / listener protocol](../CLAUDE.md#hook-contract), the persisted settings/store schema (`src/state/config.ts` `version`), or removing a feature. |

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

A release is **two steps**, because the version bump goes through a pull request
like every other change to `main`.

### 1. Prepare the bump

```bash
npm run release:prepare -- patch   # or minor / major
```

From a clean, up-to-date `main`, this:

1. creates a `release/vX.Y.Z` branch,
2. bumps `package.json` (+ `package-lock.json`),
3. runs `scripts/sync-version.mjs` → updates `Cargo.toml` + `Cargo.lock`,
4. commits as `chore(release): vX.Y.Z` and pushes the branch.

Then stamp `CHANGELOG.md` — move the `[Unreleased]` entries under
`## [X.Y.Z] - <date>`, add the compare link at the bottom — commit that to the
same branch, and open a PR:

```bash
gh pr create --base main --head release/vX.Y.Z --title "chore(release): vX.Y.Z"
```

Merge it once the four required checks pass.

### 2. Tag the merged commit

```bash
git checkout main && git pull
npm run release:tag
```

This tags `main`'s current HEAD as `vX.Y.Z` and pushes the tag. It refuses if the
tag already exists locally or on the remote, or if `main` isn't clean and current.

> **Why not tag in step 1?** `main` squash-merges, so the merge produces a *new*
> commit. A tag created on the release branch would point at a commit that never
> lands on `main`.

Pushing the tag triggers `.github/workflows/release.yml`, which builds the macOS
(universal) and Windows bundles and attaches them to a **draft** GitHub Release
named `Session Signals vX.Y.Z`. Review the assets — install the bundle and check
that notifications actually fire, which only works from a bundled app — then
publish.

> **Never push a version commit straight to `main`.** The `Main Protection`
> ruleset requires a PR and four green checks; a direct push from an account with
> the repository-role bypass would skip both silently. That is exactly what the
> old one-command flow did, and why this is now two steps.

## Code signing (later)

CI builds are currently **unsigned**, so macOS Gatekeeper and Windows SmartScreen
will warn on first launch. To ship signed builds, add these repo secrets and
uncomment the matching `env:` block in `.github/workflows/release.yml`:

- **macOS:** `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`,
  `APPLE_SIGNING_IDENTITY`, `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID`
  (notarization).
- **Windows:** an Authenticode certificate, referenced via
  `bundle.windows.certificateThumbprint` in `tauri.conf.json`.
