# Building & packaging Session Signals

Session Signals is a Tauri 2 app: a Rust shell (tray, windows, hook listener) + a React/
Vite frontend. One codebase produces native installers for macOS and Windows.

## Prerequisites

- **Node** 20.19+ (or 22.12+, per Vite 7) and npm — `npm install` once in the repo root.
- **Rust** stable (via [rustup](https://rustup.rs)).
- **Platform toolchain:**
  - macOS: Xcode Command Line Tools (`xcode-select --install`).
  - Windows: the **Microsoft C++ Build Tools** and **WebView2** runtime (present
    on Windows 11; bundled by the installer on Windows 10).

## Develop

```bash
npm run tauri dev
```

Runs Vite + the Rust shell with hot reload. The tray icon and floating widget
appear; the settings window opens automatically on first run (when hooks aren't
installed yet).

## Build installers

```bash
npm run tauri build
```

This runs `npm run build` (`tsc && vite build`) then bundles. Output lands in
`src-tauri/target/release/bundle/`:

| Platform | Artifacts | Path |
|---|---|---|
| macOS | `.app`, `.dmg` | `bundle/macos/Session Signals.app`, `bundle/dmg/Session Signals_<ver>_<arch>.dmg` |
| Windows | `.msi`, `.exe` (NSIS) | `bundle/msi/Session Signals_<ver>_<arch>_en-US.msi`, `bundle/nsis/Session Signals_<ver>_<arch>-setup.exe` |

Build on each target OS for that OS's installers (no cross-compilation here).

To limit targets:

```bash
npm run tauri build -- --bundles dmg          # macOS dmg only
npm run tauri build -- --bundles nsis msi     # Windows installers only
```

## App identity

Set in `src-tauri/tauri.conf.json`:

- `productName: "Session Signals"`, `identifier: "com.beacon.cc"`, `version`.
- `bundle.icon` — app icons (`.icns` / `.ico` / PNGs in `src-tauri/icons/`).
- `bundle.category`, `shortDescription`, `longDescription`, `copyright`.

> The **tray** icon is *not* a bundled asset — it's rendered at runtime from the
> active theme's palette (see `src/themes/README.md`), so themes need no images.

## Code signing & notarization (checklist)

Unsigned builds run locally and are fine for development and side-loading. For
distribution, sign so the OS doesn't warn/block. None of this blocks
`npm run tauri build`.

### macOS

- [ ] Apple Developer ID Application certificate in the login keychain.
- [ ] Sign: set `APPLE_CERTIFICATE` / `APPLE_SIGNING_IDENTITY` (or configure
      `bundle.macOS.signingIdentity`) so Tauri signs the `.app`/`.dmg`.
- [ ] Notarize: provide `APPLE_ID`, `APPLE_PASSWORD` (app-specific), and
      `APPLE_TEAM_ID`; Tauri submits to Apple and staples the ticket.
- [ ] Verify: `spctl -a -vvv "Session Signals.app"` reports *accepted / Notarized*.
- [ ] Entitlements: Session Signals needs no special entitlements (loopback HTTP only).

### Windows

- [ ] Authenticode code-signing certificate (OV/EV; EV avoids SmartScreen warm-up).
- [ ] Configure `bundle.windows.certificateThumbprint` (+ `signCommand` /
      `timestampUrl`) or sign artifacts in CI with `signtool`.
- [ ] Verify: `signtool verify /pa "Session Signals_<ver>_x64-setup.exe"`.

### Linux (optional)

- [ ] Not a target platform for v1, but `appimage`/`deb` bundles build if you add
      Linux to `targets`. No signing required for local use.

## Notes

- Installer **`installMode`** is `currentUser` (NSIS) — no admin prompt.
- `minimumSystemVersion` for macOS is `10.15`.
- Listener binds `127.0.0.1` only; the app makes no outbound network calls.
