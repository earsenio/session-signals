<!-- Thanks for contributing to Beacon! Keep PRs small and focused. -->

## Summary

<!-- What does this change, and why? Link any related issue (e.g. "Closes #12"). -->

## How I tested

<!-- Commands run, manual steps, screenshots/GIFs for UI changes. -->

## Checklist

- [ ] `npm run lint`, `npm run format:check`, and `npm run typecheck` pass
- [ ] `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test` pass (in `src-tauri/`)
- [ ] Added/updated tests for the change where it makes sense
- [ ] Updated `CHANGELOG.md` (Unreleased) if the change is user-facing
- [ ] No new telemetry or outbound network calls — Beacon stays fully local
