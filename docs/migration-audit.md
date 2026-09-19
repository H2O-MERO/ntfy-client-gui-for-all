# ntfy-pusher migration audit

Date: 2026-09-19  
Audited source: `H2O-MERO/ntfy-pusher-Windows`, `master`  
Audit method: direct review of all functional C# source, project/workflow manifests, WinForms designers, and localization resources. The legacy checkout is read-only and excluded from the new repository.

## Current architecture

The legacy application is a single Windows GUI process targeting `.NET 6` and `Windows 10.0.17763`. WinForms owns the application lifetime. Closing the main window hides it; the same process retains the tray icon, network subscriptions, notifications, settings, updater, and clipboard integration.

| Area | Legacy implementation | Dependencies / storage |
|---|---|---|
| Entry and instance check | `Program.cs` | Process-name counting; `-h`, `-t`, `-m` |
| Main lifetime and tray | `MainForm.cs`, `MainForm.Designer.cs` | WinForms `NotifyIcon` |
| ntfy subscriptions | `Notifications/NotificationListener.cs` | `HttpClient`, `ClientWebSocket`, Newtonsoft.Json |
| Wire models | `Notifications/NtfyEvent.cs`, `SubscribedTopic.cs` | JSON properties |
| Toast notifications | `MainForm.OnNotificationReceive` | Microsoft.Toolkit.Uwp.Notifications 7.1.3 |
| Custom popup | `NotificationDialog.cs` | WinForms, user32 animation, Windows sound registry |
| Topic editing | `SubscribeDialog.cs` | WinForms |
| Preferences | `SettingsDialog.cs`, `SettingsManager.cs`, `SettingsModel.cs` | `settings.json` next to executable |
| Topic storage | `MainForm.LoadTopics/SaveTopicsToFile` | `topics.json` next to executable |
| Localization | form `.resx`, `Properties/Resources*.resx` | zh-CN default and en-US |
| Autostart | `MainForm.UpdateAutoStart` | `HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run` |
| Update check/download | `Updater/*` | GitHub Releases API, ZIP, PowerShell replacement script |
| Release | `.github/workflows/release.yml` | Windows x64 framework-dependent ZIP |

## Functional acceptance inventory

The following behavior exists in code and must be represented in the replacement unless a documented platform capability prevents it.

- Subscribe to many unique `topic@server` pairs concurrently.
- Accept `http`, `https`, `ws`, and `wss` input URLs. The subscription type selector rewrites schemes and selects HTTP JSON streaming or WebSocket.
- Send HTTP Basic credentials when both username and password are present. Reject half-filled credentials in the UI.
- Parse newline-delimited ntfy events and only notify for `event == "message"`.
- Use explicit message title or fall back to `topic@server`; map priorities 1 through 5 to notification severity.
- Show Windows Toast with a copy action, or automatically copy the body. The legacy Toast expires after the configured timeout and priority 5 uses the alarm scenario.
- Offer a custom top-right popup with optional countdown bar, dark colors, Windows notification sound, and manual close.
- Add and remove subscriptions; copy a topic name or its full `topic@server` display string.
- Configure timeout, reconnect attempts, reconnect delay, native/custom notification mode, popup bar/theme/sound, auto-copy, autostart, silent autostart, and language.
- Hide the control window on user close while keeping subscriptions alive; expose show/check-update/exit tray commands.
- Silently check GitHub Releases at startup, manually check from the menu/tray, download a ZIP with progress, preserve launch arguments, replace files, and restart.
- Support simplified Chinese and English resource sets.
- Preserve `--help`, `--start-in-tray`, and `--allow-multiple-instances` compatibility aliases.

## Persisted data and compatibility

### `settings.json`

Location: executable directory. Current `Revision` is 4. JSON uses PascalCase names:

`Revision`, `Timeout`, `ReconnectAttempts`, `ReconnectAttemptDelay`, `NotificationsMethod`, `CustomTrayNotificationsShowTimeoutBar`, `CustomTrayNotificationsShowInDarkMode`, `CustomTrayNotificationsPlayDefaultWindowsSound`, `NativeNotificationsAutoCopyToClipboard`, `AutoStartEnabled`, `AutoStartSilent`, `Language`.

Legacy revision merging fills fields introduced in revisions 1 through 4 with defaults. It is not an atomic update and malformed JSON can terminate startup.

### `topics.json`

Location: executable directory. It is a JSON array of objects containing `TopicId`, `ServerUrl`, `Username`, and `Password`. `Runner` and `RunnerCanceller` are excluded. Credentials are stored in plaintext.

### `topics.txt`

One topic per line, implicitly on `https://ntfy.sh` with no credentials. The legacy migration writes `topics.json` and then deletes `topics.txt`. The Rust migration must preserve the source and write a new, versioned file atomically.

### New storage rules

The replacement uses the platform per-user configuration directory and an instance-specific subdirectory. Writes use a same-directory temporary file, sync, and rename. Unix directories/files are restricted to the current user. IPC uses a random 256-bit token stored alongside configuration. Passwords are redacted from `Debug` and logs through `SecretString`; moving credentials to OS credential stores remains a release-gating security item because Linux Secret Service is not universally available.

## Network behavior and gaps

The legacy listener starts one async task per topic, but not one OS thread per topic. HTTP creates a new `HttpClient` on every reconnect while attempting to reuse the same `HttpRequestMessage`, which is invalid on modern .NET after the first send. A fresh 8 KiB buffer is allocated for every read and the accumulated string is repeatedly split/copied.

WebSocket handling does not check message type, close frames, or `EndOfMessage`. Both transports reset failure count immediately after connection, have no stable-connection threshold, use fixed delay without jitter, and do not use ntfy `since`/event ids to resume. An HTTP EOF read of zero bytes is not treated explicitly and may spin. Authentication failure detection for WebSocket is acknowledged as unreliable in a source TODO.

Only `id`, `time`, `event`, `topic`, `message`, `title`, and `priority` are modeled. ntfy `tags`, `click`, `icon`, `actions`, and `attachment` are absent despite the broader protocol expectations in the migration brief. No duplicate suppression is implemented.

The replacement therefore adds bounded NDJSON framing, full currently required message fields, bounded event-id deduplication, reused HTTP clients, close/EOF handling, cancellation, and exponential backoff with jitter. Resume semantics need server-compatible `since` integration before claiming lossless reconnect.

## Process, IPC, and resource findings

- There is no process separation or IPC.
- Single instance is a process-name count race and cannot activate the existing window.
- `--allow-multiple-instances` shares the same configuration files and registry value, so writes can conflict.
- Subscription tasks are cancelled only when individual topics are removed; normal process exit relies on process teardown.
- The Toast copy lookup dictionary removes an entry only if its button is activated, so ignored notifications accumulate for the life of the process.
- Auto-copy creates one background STA thread per message.
- Connection failures display modal UI from listener callbacks, coupling network tasks to WinForms.
- There is no structured production logging; many failures are swallowed or only visible under `DEBUG`.

The replacement uses a daemon-owned configuration and subscription coordinator, bounded channels, native IPC, explicit shutdown, and a separate Slint process. GUI exit cannot cancel daemon subscriptions.

## Notification and platform compatibility

Windows currently has the richest behavior: Toast title/body/copy and automatic copy. The custom popup is not an OS notification and will not be reproduced as an always-resident Slint window; native notifications are the cross-platform default. Linux notification actions depend on the active freedesktop notification server. macOS actions require a packaged application identity and user authorization. Missing action support must degrade to title/body/click and retain auto-copy when configured.

Windows Toast identity, macOS bundle/signing/notarization, and Linux desktop service availability must be tested on their actual target systems. Compilation alone is not runtime verification.

## Update and release findings

The existing updater trusts the first ZIP (or EXE) asset returned by the GitHub release and validates only byte count. It extracts an untrusted ZIP and copies its contents over the installation with an execution-policy-bypass PowerShell script. There is no checksum/signature, ZIP path validation statement, rollback, install-scope handling, or package-manager awareness.

The new release design must publish per-target artifacts and SHA-256 checksums. Windows may support verified self-update when installed portably; packaged installations must delegate to their installer. Linux package-managed installations and macOS signed application bundles default to version notification/manual update until an atomic, signature-preserving updater is implemented.

## README mismatches

- “Lightweight” has no recorded measurement methodology or result.
- “Private topic” means Basic Auth only; token/Bearer auth is not present.
- “Multi-server” is implemented indirectly by independent topic records, not a reusable server model.
- The README does not disclose plaintext credentials, installation-directory writes, fixed reconnect behavior, or the lack of update authenticity checks.
- The README presents auto-update as complete, but its release workflow publishes only a framework-dependent Windows x64 ZIP requiring .NET 6.

## Migration risks and gates

1. Non-destructive recognition of all legacy paths, including portable installations where files sit next to the old executable.
2. Credential migration without logging or silent loss; OS credential-store rollout needs explicit fallback behavior.
3. Windows Toast registration and action activation across portable and installed builds.
4. macOS notifications, menu bar, login item, signing, hardened runtime, and notarization.
5. Linux notification action and tray availability across GNOME/KDE and Wayland/X11.
6. Correct replay/deduplication after suspend, network transition, and reconnect.
7. Local IPC access control on Windows beyond remote-client rejection; token authentication is defense in depth, not a substitute for a current-user DACL.
8. Software renderer limitations for CJK text and visual effects. FemtoVG must remain available and be evaluated on each target.
9. Authenticated update metadata, archive traversal protection, rollback, and package-manager ownership.

## Acceptance tracking

Implementation status is tracked in `docs/implementation-status.md`. A feature is marked verified only with an automated test, a local build/run record, or a named CI target. Unsupported or untested target behavior is kept explicit rather than inferred from shared source.
