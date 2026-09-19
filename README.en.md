<p align="center">
  <img src="assets/logo.svg" width="128" height="128" alt="ntfy-client-gui-for-all Logo">
</p>

<h1 align="center">ntfy-client-gui-for-all</h1>

<p align="center">
  A lightweight cross-platform ntfy desktop notification client built with Rust and Slint
</p>

<p align="center">
  <a href="README.md">简体中文</a> · <strong>English</strong>
</p>

## Features

- Independent daemon and on-demand settings GUI. Closing the GUI does not stop subscriptions.
- Multiple ntfy servers and topics over HTTP JSON streaming or WebSocket.
- HTTP Basic authentication with credentials excluded from logs and debug output.
- Titles, messages, priorities, tags, links, view actions, and attachment metadata.
- Bounded queues, message-id deduplication, `since` recovery, exponential backoff, and jitter.
- Native Windows Toast and system notifications on Linux and macOS.
- Copy, automatic copy, sound, and user-initiated HTTP(S) view actions.
- Authenticated local IPC over Windows Named Pipe or Unix Domain Socket.
- Single-instance daemon and GUI, plus isolated named instances.
- Fluent-style Slint settings UI with light, dark, and system themes.
- Simplified Chinese and English interfaces.
- Windows tray, Linux KSNI status indicator, and cross-platform login autostart.
- GitHub Release checks and SHA-256 artifact verification.

See [implementation status](docs/implementation-status.md) for verified and limited platform behavior.

## Requirements

- Rust 1.92 or newer.
- Windows 10/11, macOS 11+, or a Linux desktop using X11/Wayland.
- Debian/Ubuntu build packages:

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

Run with the default Winit + Slint Software Renderer:

```sh
cargo run -p ntfy-client-gui-for-all
```

Run with FemtoVG:

```sh
cargo run -p ntfy-client-gui-for-all --no-default-features --features renderer-femtovg
```

When both renderers are compiled, select one with `SLINT_BACKEND=winit-software` or
`SLINT_BACKEND=winit-femtovg`.

## Run

Normally start the GUI. It launches the daemon when required:

```sh
cargo run -p ntfy-client-gui-for-all
```

Start the daemon directly:

```sh
cargo run -p ntfy-client-gui-for-all-daemon -- --start-in-tray
```

Command-line options:

- `-h`, `--help`: show help.
- `-t`, `--start-in-tray`: run directly in background mode.
- `-m`, `--allow-multiple-instances`: create a PID-isolated instance.
- `--instance NAME`: select a stable configuration and IPC namespace.
- `--config-dir PATH`: override the user configuration directory for tests or portable use.
- `--import-dir PATH`: import a compatible configuration without modifying source files.

## Configuration

The application follows the operating system's per-user configuration-directory convention.
Each named instance owns its own `config.json`, `ipc-token`, and Unix socket. Configuration
is written through a temporary file and atomic replacement. Passwords and IPC tokens are never logged.

## Release

Pushing a `vX.Y.Z` tag builds Windows x86_64, Linux x86_64, macOS x86_64, and macOS arm64
packages. Every archive receives a `.sha256` sidecar.

macOS CI artifacts use ad-hoc signing only. Public distribution still requires Developer ID
signing and notarization. See [release documentation](docs/release.md).

## Documentation

- [Architecture](docs/architecture.md)
- [IPC and security](docs/ipc.md)
- [Implementation and platform status](docs/implementation-status.md)
- [Performance measurements](docs/performance.md)
- [Renderer selection and validation](docs/rendering.md)
- [Build and release](docs/release.md)

## License

MIT
