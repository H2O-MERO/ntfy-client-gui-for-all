<p align="center">
  <img src="assets/logo.svg" width="128" height="128" alt="ntfy-client-gui-for-all Logo">
</p>

<h1 align="center">ntfy-client-gui-for-all</h1>

<p align="center">
  使用 Rust 与 Slint 构建的轻量级跨平台 ntfy 桌面通知客户端
</p>

<p align="center">
  <strong>简体中文</strong> · <a href="README.en.md">English</a>
</p>

## 功能

- 后台核心与设置界面采用独立进程，关闭 GUI 不会中断通知订阅。
- 支持多个 ntfy 服务器、多个话题、HTTP JSON Stream 与 WebSocket。
- 支持 HTTP Basic 身份认证，凭据不会写入日志或调试输出。
- 支持消息标题、正文、优先级、标签、链接、查看动作与附件元数据。
- 使用有界队列、消息 ID 去重、`since` 断线恢复、指数退避与随机抖动。
- Windows 使用原生 Toast；Linux 和 macOS 使用系统通知服务。
- 支持复制、自动复制、通知声音以及用户确认后的 HTTP(S) 查看动作。
- Windows Named Pipe 或 Unix Domain Socket 本地 IPC，并使用实例级 256 位令牌认证。
- 后台与 GUI 单实例运行，支持相互独立的命名实例。
- Fluent 风格 Slint 设置界面，支持浅色、深色及跟随系统主题。
- 支持简体中文和英文界面。
- 支持 Windows 托盘、Linux KSNI 状态指示器和跨平台登录自启动。
- 支持 GitHub Release 版本检查及 SHA-256 更新包校验。

各平台已经验证及仍受限制的功能见[实现状态](docs/implementation-status.md)。

## 环境要求

- Rust 1.92 或更高版本。
- Windows 10/11、macOS 11+，或使用 X11/Wayland 的 Linux 桌面。
- Debian/Ubuntu 构建依赖：

  ```sh
  sudo apt install libx11-dev libx11-xcb-dev libxcursor-dev \
    libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev pkg-config
  ```

## 构建与测试

```sh
cargo build --workspace
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
```

默认 GUI 使用 Winit + Slint Software Renderer：

```sh
cargo run -p ntfy-client-gui-for-all
```

使用 FemtoVG：

```sh
cargo run -p ntfy-client-gui-for-all --no-default-features --features renderer-femtovg
```

同时编译两个渲染器时，可使用 `SLINT_BACKEND=winit-software` 或
`SLINT_BACKEND=winit-femtovg` 显式选择。

## 运行

通常只需启动 GUI；GUI 会在需要时启动后台：

```sh
cargo run -p ntfy-client-gui-for-all
```

直接启动后台：

```sh
cargo run -p ntfy-client-gui-for-all-daemon -- --start-in-tray
```

命令行选项：

- `-h`、`--help`：显示帮助。
- `-t`、`--start-in-tray`：直接以后台模式运行。
- `-m`、`--allow-multiple-instances`：创建按 PID 隔离的实例。
- `--instance NAME`：选择稳定的配置与 IPC 命名空间。
- `--config-dir PATH`：覆盖用户配置目录，适合测试或便携运行。
- `--import-dir PATH`：从指定目录导入兼容格式的配置，源文件不会被修改。

## 配置

应用使用操作系统约定的用户配置目录。每个命名实例拥有独立的 `config.json`、
`ipc-token`，以及 Unix 平台上的本地 Socket。配置通过临时文件和原子替换写入，
日志不会包含密码或 IPC 令牌。

## 发布

推送 `vX.Y.Z` 标签后，发布工作流将构建 Windows x86_64、Linux x86_64、
macOS x86_64 和 macOS arm64 包，并为每个压缩包生成 `.sha256` 文件。

macOS CI 产物只使用临时签名。公开分发仍需要 Developer ID 签名与公证。
详细说明见[发布文档](docs/release.md)。

## 文档

- [架构](docs/architecture.md)
- [IPC 与安全](docs/ipc.md)
- [实现及平台状态](docs/implementation-status.md)
- [性能测量](docs/performance.md)
- [渲染器选择与验证](docs/rendering.md)
- [构建与发布](docs/release.md)

## 许可证

MIT
