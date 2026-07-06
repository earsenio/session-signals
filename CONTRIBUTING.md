# Contributing to Session Signals

Thanks for your interest in improving Session Signals! This is a small, fully-local
desktop app — contributions that keep it lightweight, private, and well-tested
are very welcome.

## Ground rules

- **Stay local.** Session Signals makes **no outbound network calls** and has no
  telemetry. If a change would add one, it won't be merged. The only network
  surface is the inbound loopback listener.
- **Engine is the source of truth.** State flows one way: the Rust engine derives
  state → emits to the webview → React renders. The UI never derives state
  independently.
- **TypeScript strict, no `any`.** Functional React, hooks-based.
- **Keep the Rust surface minimal and well-commented**; prefer putting logic in
  the engine so the UI stays a thin renderer.

See [CLAUDE.md](CLAUDE.md) for the full standing conventions and the locked
architectural decisions, and [docs/SPEC.md](docs/SPEC.md) for the requirements.

## Development setup

**Prerequisites:** Node.js 20.19+ (or 22.12+), the Rust toolchain via [rustup](https://rustup.rs),
and your platform's Tauri system dependencies (WebView, build tools) — see
[docs/BUILD.md](docs/BUILD.md).

```bash
npm install
npm run tauri dev      # runs the app: tray + floating widget, no dock icon
```

Then open the tray menu → **Install Claude Code hooks**, start a Claude Code
session, and watch the tray and widget react.

## Running the checks

Please run these before opening a PR:

```bash
npm run build                 # typecheck (tsc --strict) + bundle (vite)
cd src-tauri && cargo test    # engine, listener, hook-merge, install tests
cargo fmt --check             # Rust formatting
cargo clippy -- -D warnings   # Rust lints
```

The Rust engine/listener/hooks suites are the primary correctness gate — if you
touch state derivation, the hook contract, or settings-file merging, add or
update tests alongside your change.

## Commit & PR conventions

- Write clear, imperative commit subjects (e.g. "Fix widget restoring to
  ~0-height list on expand"). Keep one logical change per commit where practical.
- Reference the spec or an issue when relevant.
- Open a PR against `main`. CI (typecheck, lint, Rust tests, build matrix) should
  be green. Describe what changed and how you verified it.
- Small, focused PRs review fastest.

## Versioning

Session Signals uses SemVer, releases-only. **`package.json` is the single source of
truth** — never hand-edit the version in `tauri.conf.json` or `Cargo.toml`. Bump
via:

```bash
npm run release:patch   # or release:minor / release:major
```

Full details: [docs/VERSIONING.md](docs/VERSIONING.md). Note user-facing changes
in [CHANGELOG.md](CHANGELOG.md) under **Unreleased**.

## Releasing

Releases are automated by `.github/workflows/release.yml` (maintainers only).

1. Move the **Unreleased** notes in [CHANGELOG.md](CHANGELOG.md) under the new
   version heading, then commit.
2. From a clean working tree, bump + tag + push in one step:
   ```bash
   npm run release:patch   # or release:minor / release:major
   ```
   This bumps `package.json`, syncs `Cargo.toml`/`Cargo.lock`, creates the
   `vX.Y.Z` commit and tag, and pushes both (the `postversion` hook).
3. The pushed tag triggers the **Release** workflow. It builds the macOS
   (universal `.dmg`) and Windows (`.msi`/`.exe`) installers with
   [`tauri-action`](https://github.com/tauri-apps/tauri-action) and uploads them
   to a **draft** GitHub Release named `Session Signals vX.Y.Z`.
4. Review the attached installers, then **publish** the draft Release.

To (re)build an existing tag without re-tagging, dispatch the workflow manually:

```bash
gh workflow run release.yml --ref main -f tag=vX.Y.Z
```

> Heads-up (tauri-action v1): if a **non-draft** Release already exists for the
> tag, the workflow fails rather than editing it — delete or re-draft that
> Release before re-running.

**Signing.** Installers currently ship **unsigned** (users do a one-time
right-click → Open on macOS / "Run anyway" on Windows). To sign, add the cert
secrets and uncomment the relevant `env:` block in `release.yml` — see the
current Tauri docs for [macOS](https://v2.tauri.app/distribute/sign/macos/) and
[Windows](https://v2.tauri.app/distribute/sign/windows/).

## Reporting bugs & ideas

Open a GitHub issue with steps to reproduce, your OS, the Session Signals version, and
(for detection issues) which hook events you expected. For **security** issues,
do **not** open a public issue — see [SECURITY.md](SECURITY.md).

## Code of Conduct

By participating you agree to uphold our [Code of Conduct](CODE_OF_CONDUCT.md).
