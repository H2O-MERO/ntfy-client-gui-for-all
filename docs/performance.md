# Performance measurements

Measurements below are actual samples, not targets or estimates.

## 2026-09-19 Windows smoke

- OS: Windows host provided by the development environment.
- Build: Rust Debug profile, `rustc 1.97.1`; Slint 1.18, Winit + Software Renderer.
- Configuration: empty isolated instance, no server or topic.
- Sampling: PowerShell `Get-Process` after approximately three seconds. `WorkingSet64` is the process working set; `PrivateMemorySize64` is private committed bytes.

| Scenario | Working set | Private bytes | Cumulative CPU |
|---|---:|---:|---:|
| Daemon only, empty config | 13,766,656 B (13.13 MiB) | 3,092,480 B (2.95 MiB) | 0.03125 s |
| Daemon while GUI open | 13,983,744 B (13.34 MiB) | 3,125,248 B (2.98 MiB) | 0.05 s |
| GUI open on overview | 38,334,464 B (36.56 MiB) | 7,540,736 B (7.19 MiB) | 0.66 s |
| Daemon after tray/event-stream integration, empty config | 16,121,856 B (15.38 MiB) | 3,379,200 B (3.22 MiB) | 0.015625 s |

The same host was then sampled with the optimized Release profile after approximately three to four seconds:

| Scenario | Working set | Private bytes | Cumulative CPU | Handles |
|---|---:|---:|---:|---:|
| Daemon only, empty config, tray/event stream enabled | 12,357,632 B (11.79 MiB) | 3,121,152 B (2.98 MiB) | 0.015625 s | 156 |
| Daemon while Release GUI open | 12,537,856 B (11.96 MiB) | 3,170,304 B (3.02 MiB) | 0.015625 s | 157 |
| Release GUI open on overview | 28,778,496 B (27.45 MiB) | 7,688,192 B (7.33 MiB) | 0.234375 s | 236 |

The Release GUI was forcibly stopped as part of the harness and the daemon remained alive, directly verifying process separation for that run. The daemon and the isolated temporary configuration were then removed by the harness.

The GUI process and daemon were terminated after sampling, and the isolated test configuration was removed.

The final daemon sample includes the native tray and IPC broadcast infrastructure; the earlier daemon samples predate that integration. These Debug figures do not establish long-running stability or provide a fair comparison with unrelated builds. Release-profile samples, real subscriptions, disconnect/reconnect, notification bursts, and 24-hour growth tests remain required. Linux and macOS have no runtime measurement yet.

## Required benchmark matrix

For each release candidate, record OS/build/renderer and sample at steady state for:

1. Daemon idle with no topics.
2. One server and one topic.
3. Multiple servers and topics.
4. GUI open and idle.
5. GUI closed, confirming process removal.
6. Network disconnect and successful reconnect.
7. Sustained notification delivery followed by a quiet period.

Report working set/private bytes, CPU time or idle percentage, open handles/file descriptors, and GPU allocation where available. Do not compare different OS counters as if they were identical.
