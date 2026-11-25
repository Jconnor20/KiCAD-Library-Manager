# Contributing

## Prerequisites

- Windows (the app and its tests are Windows-only).
- Rust stable (`rustup` default toolchain, with `rustfmt` and `clippy`).
- KiCad 9 or 10 on the machine if you want the `kicad-cli`-backed tests to run. Without it they skip themselves.

## Build and run

```
cargo build --release
cargo run --release -p klm-gui      # the GUI (klm.exe)
cargo run --release -p klm-cli            # with no arguments, prints every command
scripts\package.ps1                        # dist\klm-pcm-<version>.zip, the KiCad PCM package
```

## Test

```
cargo test --workspace
```

The GUI tests render offscreen (egui_kittest on wgpu), so they need no window. Tests that call live distributor APIs are `#[ignore]`d and need your own keys:

```
cargo test -p klm-core --test live_api -- --ignored live
```

### Keep tests away from your real library

Set `KLM_CONFIG_DIR` (or pass `--config <dir>` to `klm` / `klm-cli`) and everything KLM writes, including KiCad's library tables and stored API keys, goes into that folder instead of `%APPDATA%`. Most tests make their own temporary sandbox; setting it yourself before running them, or while trying a change by hand, makes sure nothing touches your real library.

```
$env:KLM_CONFIG_DIR = "$env:TEMP\klm-sandbox"; cargo test --workspace
```

## Style

- `cargo fmt --all` (see `rustfmt.toml`, 160 columns).
- `cargo clippy --workspace --all-targets -- -D warnings` must pass; thresholds are in `clippy.toml`.
- Comments explain why, not what.

## Pull requests

- Add tests for new logic.
- CI (fmt, clippy, tests, `cargo deny`) must be green.
- Keep a PR to one change; say what you could not test.

## Cutting a release

1. Bump `version` under `[workspace.package]` in `Cargo.toml` and merge it to `master`.
2. Tag and push: `git tag v<version>` then `git push origin v<version>`.

The Release workflow checks that the tag matches `Cargo.toml`, builds the KiCad PCM package and a portable zip, publishes a GitHub Release, and commits the new version to the PCM repository in `pcm/` (`scripts/update-pcm-repo.ps1`).
