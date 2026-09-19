# Slint rendering backends

The GUI is deliberately isolated from the daemon, so choosing a renderer changes only the on-demand settings process. The daemon does not link Slint, Winit, FemtoVG, or any graphics stack.

## Build-time selection

The default feature is Winit + Software Renderer:

```sh
cargo run -p ntfy-pusher-gui
```

FemtoVG remains an explicit alternative:

```sh
cargo run -p ntfy-pusher-gui --no-default-features --features renderer-femtovg
```

When a build contains both renderers, `SLINT_BACKEND=winit-software` and `SLINT_BACKEND=winit-femtovg` select one at runtime. Release packages currently use the default software renderer so their behavior is deterministic.

## What was actually verified

On the Windows development host, the software renderer displayed the dark Fluent layout, rounded cards, controls, and simplified Chinese text correctly at the host's display scale. The measured Debug-process values are in [performance.md](performance.md), and the captured window is in [gui-smoke.png](images/gui-smoke.png).

FemtoVG is compile-checked locally and in CI, but has not been run or measured in the current environment. Linux and macOS renderer behavior is likewise CI-build coverage only until native runtime testing is completed. No GPU-memory, launch-time, or animation comparison is claimed without measurements.

## Release decision

Software rendering is the current default because the settings UI is mostly static, it rendered all required effects in the available Windows test, and it avoids depending on a working GPU driver. This is not an assumption that software rendering always consumes less total memory. A release candidate should measure both backends on representative integrated and discrete GPU systems before changing the default.

The UI intentionally avoids continuous decorative animations, large raster images, and blur-heavy surfaces. Theme, layout, and HiDPI handling use Slint's component and layout systems instead of hard-coded pixel scaling.
