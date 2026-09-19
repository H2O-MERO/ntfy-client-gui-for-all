# ntfy pusher

A lightweight cross-platform ntfy desktop notification client built with Rust and Slint.

This repository is a ground-up replacement for `H2O-MERO/ntfy-pusher-Windows`. The legacy C# repository is used only as a read-only behavioral reference; all new implementation lives in this workspace.

## What works

- Independent background core and on-demand settings GUI.
- Multi-server and multi-topic subscriptions over WebSocket or HTTP JSON streaming.
- HTTP Basic authentication with secrets excluded from logs and debug output.
- Bounded message framing, event-id deduplication, cancellation, and exponential reconnect backoff with jitter.
- Native Windows Toast with copy and auto-copy behavior; native Linux/macOS notifications with capability-based action fallback.
- Authenticated local IPC: Windows Named Pipe or Unix Domain Socket plus a per-instance 256-bit token, with event-driven status pushes to the GUI.
- Daemon and GUI single-instance behavior; named isolated instances preserve the legacy multi-instance use case without sharing files or IPC names.
- Versioned, atomically-written per-user configuration and non-destructive import of legacy `settings.json`, `topics.json`, and `topics.txt`.
- Slint Fluent settings pages for overview, servers, topics, notifications, general settings, and updates.
- Light, dark, and system theme modes; simplified Chinese and English UI.
- Login autostart through platform-appropriate mechanisms.
- Windows tray and Linux KSNI status indicator with open, update check, and quit commands.
- GitHub release checks that recognize only artifacts accompanied by SHA-256 sidecars.

See [implementation status](docs/implementation-status.md) for verified and incomplete platform behavior. In particular, macOS menu-bar integration and automatic update installation are release gates, not silently omitted features.

## Requirements

- Rust 1.92 or newer.
- Windows 10/11, macOS 11+, or a Linux desktop with X11/Wayland.
- Linux build packages on Debian/Ubuntu:

  ```sh
  sudo apt install libx11-dev libx11-xcb-dev libxcursor-dev \
    libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev pkg-config
  ```

## Build and test

```sh
cargo build --workspace
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
```

The default GUI uses Winit + Slint Software Renderer:

```sh
cargo run -p ntfy-pusher-gui
```

Build with FemtoVG available:

```sh
cargo run -p ntfy-pusher-gui --no-default-features --features renderer-femtovg
```

When both are compiled, select explicitly with `SLINT_BACKEND=winit-software` or `SLINT_BACKEND=winit-femtovg`.

## Run

Normally launch `ntfy-pusher-gui`. It starts the daemon if needed. Closing the window exits only the GUI.

```sh
cargo run -p ntfy-pusher-gui
```

Run the daemon directly:

```sh
cargo run -p ntfy-pusher-daemon -- --start-in-tray
```

Compatibility and isolation options:

- `-h`, `--help`
- `-t`, `--start-in-tray` (accepted for compatibility; the daemon is always background-only)
- `-m`, `--allow-multiple-instances` (uses a PID-derived isolated instance)
- `--instance NAME` (stable isolated config and IPC namespace)
- `--legacy-dir PATH` (explicit source for non-destructive legacy import)
- `--config-dir PATH` (testing/portable override; normal installs use the OS user config directory)

## Configuration locations

The `directories` crate selects the per-user location for organization `H2O-MERO`, application `ntfy-pusher`. Each instance gets its own directory containing `config.json`, `ipc-token`, and on Unix the local socket. Normal logs never include passwords or the IPC token.

Legacy files are detected next to the old executable or through `--legacy-dir`. Migration writes the new config first and never overwrites or deletes the source files.

## Package and release

Tagging `vX.Y.Z` runs the release workflow for Windows x86_64, Linux x86_64, and macOS x86_64/aarch64. Every archive receives a `.sha256` sidecar. The generated macOS bundle is ad-hoc signed for CI validation only; public distribution still requires a Developer ID signature and notarization.

Detailed build, package, and signing notes are in [release.md](docs/release.md).

## Documentation

- [Legacy migration audit](docs/migration-audit.md)
- [Architecture](docs/architecture.md)
- [IPC security](docs/ipc.md)
- [Implementation and platform status](docs/implementation-status.md)
- [Performance measurements](docs/performance.md)
- [Renderer selection and validation](docs/rendering.md)
- [Release process](docs/release.md)

## License

MIT
