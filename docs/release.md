# Build, package, and release

## Local release build

```sh
cargo test --locked --workspace --all-targets
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo build --locked --release --workspace
```

The two distributable binaries are `ntfy-client-gui-for-all-daemon` and `ntfy-client-gui-for-all`. Keep them in the same directory so each can launch the other.

## Windows

The workflow builds `x86_64-pc-windows-msvc` and creates `ntfy-client-gui-for-all-windows-x86_64.zip` with its SHA-256 sidecar. A production installer must create a Start Menu shortcut with AppUserModelID `io.github.h2omero.ntfy-client-gui-for-all`; that identity is required for correct Toast attribution and activation. Code signing and an installer are not configured yet.

Portable verified automatic installation is intentionally disabled until a separate updater helper can replace both binaries atomically, restore a backup on failure, and verify publisher identity in addition to SHA-256.

## Linux

The workflow emits `ntfy-client-gui-for-all-linux-x86_64.tar.gz`, a desktop entry, and icon. Distribution maintainers should package the files into their native package format and own updates through that package manager. The application reports new versions but does not overwrite package-managed files. KSNI provides the preferred status indicator; desktops without compatible indicator support need a documented launcher/CLI exit fallback.

## macOS

The workflow creates x86_64 and arm64 `.app` bundles and applies only an ad-hoc signature for CI artifact integrity. Public distribution requires:

1. A valid Apple Developer ID Application certificate.
2. Hardened runtime signing for the bundle and both executables.
3. Notarization with `notarytool` and stapling.
4. Runtime validation of notification authorization and login item behavior.
5. A main-thread AppKit status item implementation.

Do not present the CI ad-hoc bundle as notarized.

## Tag release

Update the workspace version and macOS `CFBundleShortVersionString`, commit, then:

```sh
git tag v0.1.0
git push origin v0.1.0
```

The release workflow builds all configured targets and attaches archives plus `.sha256` files. The in-app checker targets this repository and only reports `verified_asset_available` when both names match the active OS/architecture and the sidecar exists.
