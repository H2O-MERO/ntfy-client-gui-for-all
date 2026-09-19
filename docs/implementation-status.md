# Implementation status

Status date: 2026-09-19

| Capability | Windows | Linux | macOS |
|---|---|---|---|
| Daemon without Slint | Built and runtime-smoked | CI configured, not runtime-tested | CI configured, not runtime-tested |
| Slint settings GUI | Software renderer visually checked on Windows 11 | CI configured, not runtime-tested | CI configured, not runtime-tested |
| FemtoVG build option | Local compile check passed | CI configured, not runtime-tested | CI configured, not runtime-tested |
| HTTP JSON stream | Implemented; unit-level URL/decoder tests | Same shared implementation | Same shared implementation |
| WebSocket | Implemented | Same shared implementation | Same shared implementation |
| Basic Auth | Implemented | Implemented | Implemented |
| Message title/body/priority/tags/click/actions/attachment parsing | Implemented | Implemented | Implemented |
| Reconnect gap recovery with ntfy `since` | In-memory cursor implemented | Shared implementation | Shared implementation |
| Cursor persistence across daemon restart | Pending | Pending | Pending |
| Native notification | WinRT Toast compiled; copy/auto-copy implemented | notify-rust adapter; runtime pending | notify-rust adapter; permission/runtime pending |
| Tray/status entry | Native tray thread implemented | KSNI implemented; DE fallback pending | Not implemented: AppKit main-thread integration required |
| Login autostart | Current-user registry through `auto-launch` | XDG autostart through `auto-launch` | LaunchAgent through `auto-launch`; runtime pending |
| Authenticated IPC | Named Pipe + token | Unix Socket + token | Unix Socket + token |
| Event-driven GUI status updates | Bounded broadcast stream implemented | Shared implementation | Shared implementation |
| Explicit current-user pipe DACL | Protected owner/System DACL implemented | N/A | N/A |
| Compatible configuration import | Automated tests pass | Import option available | Import option available |
| Version check | Implemented against new repository | Implemented | Implemented |
| SHA-256 verified download primitive | Implemented | Implemented | Implemented |
| Automatic update installation/rollback | Pending | Manual/package-manager path | Manual signed bundle path |
| Packaging | ZIP workflow | tar.gz workflow | `.app` tar workflow, ad-hoc CI signing |
| Production signing/notarization | Signing pending | Distribution signing pending | Developer ID and notarization pending |

## Release blockers

The codebase is a functional development foundation, but the following prevent calling the project fully production-complete: persistence of reconnect cursors across daemon restarts, OS credential-store integration, macOS status item main-loop work, production signing/notarization, safe automatic installation with rollback, and runtime tests on Linux/macOS. These are retained as visible release gates.
