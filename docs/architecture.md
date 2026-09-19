# Architecture

## Process model

`ntfy-pusher` is the background owner of configuration, subscriptions, reconnect state, native notifications, tray/status entry, autostart, and update checks. It does not depend on Slint.

`ntfy-pusher-gui` is an on-demand Slint process. It obtains a snapshot over local IPC, submits complete validated configuration replacements, and asks the daemon to reconnect topics. Closing the window destroys the GUI process and does not affect the daemon.

```text
native tray/status ─┐
ntfy servers ───────┼── ntfy-pusher daemon ── native notifications
                    │          │
                    │    authenticated IPC
                    │          │
                    └── ntfy-pusher-gui (on demand)
```

## Workspace modules

| Crate | Responsibility | Slint dependency |
|---|---|---|
| `ntfy-pusher-protocol` | ntfy wire types, bounded NDJSON decoder, bounded deduplication | No |
| `ntfy-pusher-config` | versioned model, validation, atomic persistence, legacy migration | No |
| `ntfy-pusher-ipc` | messages, snapshots, framing, Named Pipe/Unix Socket transport | No |
| `ntfy-pusher-core` | HTTP/WebSocket subscriptions, reconnect, update verification | No |
| `ntfy-pusher-platform` | native notifications, clipboard, tray, autostart | No |
| `ntfy-pusher-daemon` | background process composition and lifecycle | No |
| `ntfy-pusher-gui` | Fluent settings UI and IPC client | Yes |

## Subscription lifecycle

One Tokio task is used per enabled topic; no operating-system thread is allocated per subscription. All HTTP subscriptions share a `reqwest::Client` and TLS connection pool. Each task owns only its bounded decoder and 1,024-id duplicate window. The daemon event queue is bounded to 256 items, applying backpressure rather than growing without limit.

Reconnect delay doubles from the configured initial delay to the configured maximum and adds 80–120% jitter. Cancellation interrupts sleep, network streams, and WebSocket reads. Authentication failures become terminal until configuration changes or the user requests reconnect.

The last delivered message id is reused as ntfy's `since` value when either HTTP or WebSocket reconnects, and the bounded deduplication window suppresses replay overlap. This recovers messages still present in the server cache during an in-process network interruption. The cursor is not persisted across daemon restarts, so the project does not claim zero message loss across long outages or restarts.

## Configuration ownership

The daemon is the sole writer. The GUI sends a complete replacement, the daemon validates and atomically persists it, updates autostart, then restarts the subscription set. This keeps ordering and failure handling local to one module.

Named instances isolate configuration directories, local endpoints, tokens, and subscriptions. The compatibility `--allow-multiple-instances` option derives an instance name from the PID when no explicit name is supplied.

## Rendering

The GUI uses Slint's Fluent widget style with Winit. `renderer-software` is the default feature and `renderer-femtovg` can be selected at build time. The daemon never imports either renderer. Theme changes write `Palette.color-scheme`, with `Unknown` delegating to the system.
