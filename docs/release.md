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

Linux release artifacts are not currently published. Distribution maintainers may build from source and should package the files using their native package format.

## macOS

macOS release artifacts are not currently published. Public distribution requires:

1. A valid Apple Developer ID Application certificate.
2. Hardened runtime signing for the bundle and both executables.
3. Notarization with `notarytool` and stapling.
4. Runtime validation of notification authorization and login item behavior.
5. A main-thread AppKit status item implementation.

Do not present the CI ad-hoc bundle as notarized.

## Tag release

Update the workspace version and macOS `CFBundleShortVersionString`, commit, then:

```sh
git tag vX.Y.Z
git push origin vX.Y.Z
```

The release workflow builds Windows x86_64 and attaches the ZIP archive plus its `.sha256` file. The in-app checker targets this repository and only reports `verified_asset_available` when both names match the active OS/architecture and the sidecar exists.
