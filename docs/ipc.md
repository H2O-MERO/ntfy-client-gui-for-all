# Local IPC and security

## Transport

- Windows: Tokio Named Pipe under `\\.\pipe\ntfy-pusher-<instance>`, with remote clients rejected.
- Linux/macOS: Unix Domain Socket inside the instance configuration directory. The directory is mode `0700` and the socket mode `0600`.

Binding the daemon endpoint is also the daemon single-instance lock. The GUI uses a second `gui-<instance>` endpoint; a second GUI sends `OpenGui` and exits.

## Authentication and framing

The configuration store creates 32 random bytes and writes their lowercase hexadecimal representation to `ipc-token`. Every request contains this 256-bit token and a UUID request id. The daemon compares tokens without content-dependent early exit. Transport permissions remain the primary restriction; the token is defense in depth.

Messages are newline-delimited JSON with a hard 2 MiB frame limit. Command connections carry one request and one response, which avoids buffered multi-frame ambiguity and makes disconnect cleanup immediate. `WatchEvents` upgrades one authenticated connection to a bounded broadcast stream; the daemon pushes snapshots on connection-state and configuration changes. A lagging GUI receives a fresh snapshot instead of an unbounded backlog, and reconnects with capped exponential delay after a broken stream. Invalid JSON, oversized frames, missing authentication, and unexpected disconnects are returned as typed faults or transport errors.

Supported commands include snapshot retrieval, live event watching, atomic configuration replacement, connect/disconnect/reconnect, settings activation, update check, and daemon shutdown.

## Remaining hardening

Windows Named Pipe creation rejects remote clients but does not yet install an explicit current-user-only DACL. The random token and per-user token file prevent unauthenticated control, but a release-grade Windows build should add a user SID DACL and an impersonation check. Credentials necessarily cross IPC when the GUI edits configuration; they are never logged.
