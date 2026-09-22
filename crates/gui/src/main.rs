#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use anyhow::{Context, Result};
use ntfy_client_config::{
    AppConfig, BasicCredentials, ConfigStore, ServerConfig, SubscriptionProtocol, ThemeMode,
    TopicConfig,
};
use ntfy_client_ipc::{
    ConnectionState, DaemonSnapshot, Endpoint, Event, Request, RequestEnvelope, Response,
    ResponseEnvelope, TopicStatus, connect, read_frame, request, write_frame,
};
use secrecy::SecretString;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::{
    collections::HashMap,
    env,
    path::Path,
    process::Command,
    rc::Rc,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};
use tokio::runtime::Runtime;
use url::Url;
use uuid::Uuid;

slint::include_modules!();

fn main() -> Result<()> {
    let options = parse_options()?;
    let instance = options.instance;
    let runtime = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("ntfy-gui-ipc")
            .build()?,
    );
    let store = options
        .config_dir
        .as_ref()
        .map(|root| ConfigStore::at(root.join(&instance)))
        .map(Ok)
        .unwrap_or_else(|| ConfigStore::discover(&instance))?;
    let token = store.load_or_create_ipc_token()?;
    let daemon_endpoint = Endpoint::for_instance(store.root(), &instance);
    let gui_endpoint = Endpoint::for_instance(store.root(), &format!("gui-{instance}"));

    let gui_server =
        match runtime.block_on(ntfy_client_ipc::LocalServer::bind(gui_endpoint.clone())) {
            Ok(server) => server,
            Err(_) => {
                let response = runtime.block_on(request(&gui_endpoint, &token, Request::OpenGui));
                if response.is_ok() {
                    return Ok(());
                }
                anyhow::bail!("another GUI instance exists but could not be activated");
            }
        };

    let snapshot = ensure_daemon(
        &runtime,
        &daemon_endpoint,
        &token,
        &instance,
        options.config_dir.as_deref(),
    )?;
    let ui = AppWindow::new()?;
    let state = Arc::new(Mutex::new(snapshot.config.clone()));
    apply_snapshot(&ui, snapshot);
    ui.invoke_apply_theme(ui.get_theme_mode());

    start_activation_server(runtime.clone(), gui_server, token.clone(), ui.as_weak());
    start_event_watcher(
        runtime.clone(),
        daemon_endpoint.clone(),
        token.clone(),
        state.clone(),
        ui.as_weak(),
    );
    wire_callbacks(&ui, runtime, daemon_endpoint, token, state);
    ui.run()?;
    Ok(())
}

fn start_event_watcher(
    runtime: Arc<Runtime>,
    endpoint: Endpoint,
    token: String,
    state: Arc<Mutex<AppConfig>>,
    weak: slint::Weak<AppWindow>,
) {
    runtime.spawn(async move {
        let mut retry = Duration::from_millis(250);
        loop {
            let result = async {
                let mut stream = connect(&endpoint).await?;
                let request = RequestEnvelope::new(token.clone(), Request::WatchEvents);
                write_frame(&mut stream, &request).await?;
                let response: ResponseEnvelope = read_frame(&mut stream).await?;
                match response.result {
                    Ok(Response::Accepted) => {}
                    Ok(_) => return Err(anyhow::anyhow!("unexpected event-stream response")),
                    Err(fault) => return Err(anyhow::anyhow!(fault.message)),
                }
                retry = Duration::from_millis(250);
                loop {
                    match read_frame::<Event>(&mut stream).await? {
                        Event::SnapshotChanged(snapshot) => {
                            *state.lock().expect("config mutex poisoned") = snapshot.config.clone();
                            let open =
                                weak.upgrade_in_event_loop(move |ui| apply_snapshot(&ui, snapshot));
                            if open.is_err() {
                                return Ok::<bool, anyhow::Error>(false);
                            }
                        }
                        Event::ActivateWindow => {
                            let open = weak.upgrade_in_event_loop(|ui| {
                                let _ = ui.show();
                                ui.window().request_redraw();
                            });
                            if open.is_err() {
                                return Ok(false);
                            }
                        }
                        Event::UpdateAvailable { .. } => {}
                    }
                }
            }
            .await;

            if matches!(result, Ok(false)) || weak.upgrade().is_none() {
                return;
            }
            if let Err(error) = result {
                set_error(&weak, format!("status stream disconnected: {error}"));
            }
            tokio::time::sleep(retry).await;
            retry = (retry * 2).min(Duration::from_secs(5));
        }
    });
}

struct Options {
    instance: String,
    config_dir: Option<std::path::PathBuf>,
}

fn parse_options() -> Result<Options> {
    let mut args = env::args_os().skip(1);
    let mut instance = "default".to_owned();
    let mut config_dir = None;
    while let Some(arg) = args.next() {
        match arg.to_string_lossy().as_ref() {
            "--instance" => {
                instance = args
                    .next()
                    .context("--instance requires a value")?
                    .to_string_lossy()
                    .into_owned()
            }
            "--config-dir" => {
                config_dir = Some(args.next().context("--config-dir requires a path")?.into());
            }
            "-h" | "--help" => {
                println!(
                    "ntfy-client-gui-for-all {}\n\nUsage: ntfy-client-gui-for-all [--instance NAME]",
                    env!("CARGO_PKG_VERSION")
                );
                std::process::exit(0);
            }
            unknown => anyhow::bail!("unknown option {unknown}"),
        }
    }
    Ok(Options {
        instance,
        config_dir,
    })
}

fn ensure_daemon(
    runtime: &Runtime,
    endpoint: &Endpoint,
    token: &str,
    instance: &str,
    config_dir: Option<&Path>,
) -> Result<DaemonSnapshot> {
    if let Ok(ResponseEnvelope {
        result: Ok(Response::Snapshot(snapshot)),
        ..
    }) = runtime.block_on(request(endpoint, token, Request::GetSnapshot))
    {
        return Ok(snapshot);
    }
    start_daemon(instance, config_dir)?;
    for _ in 0..40 {
        thread::sleep(Duration::from_millis(100));
        if let Ok(ResponseEnvelope {
            result: Ok(Response::Snapshot(snapshot)),
            ..
        }) = runtime.block_on(request(endpoint, token, Request::GetSnapshot))
        {
            return Ok(snapshot);
        }
    }
    anyhow::bail!("background core did not become ready within four seconds")
}

fn start_daemon(instance: &str, config_dir: Option<&Path>) -> Result<()> {
    let mut executable = env::current_exe()?;
    executable.set_file_name(if cfg!(windows) {
        "ntfy-client-gui-for-all-daemon.exe"
    } else {
        "ntfy-client-gui-for-all-daemon"
    });
    let mut command = Command::new(executable);
    command.args(["--instance", instance, "--start-in-tray"]);
    if let Some(config_dir) = config_dir {
        command.arg("--config-dir").arg(config_dir);
    }
    command.spawn()?;
    Ok(())
}

fn start_activation_server(
    runtime: Arc<Runtime>,
    mut server: ntfy_client_ipc::LocalServer,
    token: String,
    weak: slint::Weak<AppWindow>,
) {
    runtime.spawn(async move {
        loop {
            let Ok(mut stream) = server.accept().await else {
                break;
            };
            let Ok(envelope) = read_frame::<RequestEnvelope>(&mut stream).await else {
                continue;
            };
            let accepted = envelope.token == token && matches!(envelope.request, Request::OpenGui);
            if accepted {
                let activate = weak.clone();
                let _ = activate.upgrade_in_event_loop(|ui| {
                    let _ = ui.show();
                    ui.window().request_redraw();
                });
            }
            let result = if accepted {
                Ok(Response::Accepted)
            } else {
                Err(ntfy_client_ipc::IpcFault::unauthorized())
            };
            let _ = write_frame(
                &mut stream,
                &ResponseEnvelope {
                    request_id: envelope.request_id,
                    result,
                },
            )
            .await;
        }
    });
}

fn wire_callbacks(
    ui: &AppWindow,
    runtime: Arc<Runtime>,
    endpoint: Endpoint,
    token: String,
    state: Arc<Mutex<AppConfig>>,
) {
    {
        let runtime = runtime.clone();
        let endpoint = endpoint.clone();
        let token = token.clone();
        let state = state.clone();
        let weak = ui.as_weak();
        ui.on_refresh(move || {
            refresh(
                &runtime,
                endpoint.clone(),
                token.clone(),
                state.clone(),
                weak.clone(),
            )
        });
    }
    {
        let runtime = runtime.clone();
        let endpoint = endpoint.clone();
        let token = token.clone();
        let state = state.clone();
        let weak = ui.as_weak();
        ui.on_add_server(move |name, url, username, password| {
            let result = (|| -> Result<AppConfig> {
                if name.trim().is_empty() {
                    anyhow::bail!("server name is required");
                }
                let mut base_url = Url::parse(url.trim()).context("invalid server URL")?;
                if !matches!(base_url.scheme(), "http" | "https") {
                    anyhow::bail!("server URL must use http or https");
                }
                base_url.set_fragment(None);
                base_url.set_query(None);
                let has_user = !username.trim().is_empty();
                let has_password = !password.is_empty();
                if has_user != has_password {
                    anyhow::bail!("username and password must be supplied together");
                }
                let mut config = state.lock().expect("config mutex poisoned").clone();
                if config
                    .servers
                    .iter()
                    .any(|server| server.name.eq_ignore_ascii_case(name.trim()))
                {
                    anyhow::bail!("server name already exists");
                }
                config.servers.push(ServerConfig {
                    id: Uuid::new_v4(),
                    name: name.trim().into(),
                    base_url,
                    credentials: has_user.then(|| BasicCredentials {
                        username: username.trim().into(),
                        password: SecretString::from(password.to_string()),
                    }),
                });
                config.validate()?;
                Ok(config)
            })();
            match result {
                Ok(config) => persist(
                    &runtime,
                    endpoint.clone(),
                    token.clone(),
                    state.clone(),
                    weak.clone(),
                    config,
                ),
                Err(error) => set_error(&weak, error.to_string()),
            }
        });
    }
    {
        let runtime = runtime.clone();
        let endpoint = endpoint.clone();
        let token = token.clone();
        let state = state.clone();
        let weak = ui.as_weak();
        ui.on_delete_server(move |id| {
            let Ok(id) = Uuid::parse_str(id.as_str()) else {
                set_error(&weak, "invalid server id".into());
                return;
            };
            let mut config = state.lock().expect("config mutex poisoned").clone();
            config.servers.retain(|server| server.id != id);
            config.topics.retain(|topic| topic.server_id != id);
            persist(
                &runtime,
                endpoint.clone(),
                token.clone(),
                state.clone(),
                weak.clone(),
                config,
            );
        });
    }
    {
        let runtime = runtime.clone();
        let endpoint = endpoint.clone();
        let token = token.clone();
        let state = state.clone();
        let weak = ui.as_weak();
        ui.on_add_topic(move |name, server_key, protocol| {
            let result = (|| -> Result<AppConfig> {
                if name.trim().is_empty() || name.contains('/') {
                    anyhow::bail!("topic must be a non-empty path segment");
                }
                let mut config = state.lock().expect("config mutex poisoned").clone();
                let server_id = config
                    .servers
                    .iter()
                    .find(|server| {
                        server.name.eq_ignore_ascii_case(server_key.trim())
                            || server.id.to_string() == server_key.as_str()
                    })
                    .map(|server| server.id)
                    .context("server was not found")?;
                if config
                    .topics
                    .iter()
                    .any(|topic| topic.server_id == server_id && topic.name == name.trim())
                {
                    anyhow::bail!("topic is already subscribed on this server");
                }
                config.topics.push(TopicConfig {
                    id: Uuid::new_v4(),
                    server_id,
                    name: name.trim().into(),
                    protocol: if protocol.as_str() == "HTTP Stream" {
                        SubscriptionProtocol::HttpStream
                    } else {
                        SubscriptionProtocol::WebSocket
                    },
                    enabled: true,
                });
                config.validate()?;
                Ok(config)
            })();
            match result {
                Ok(config) => persist(
                    &runtime,
                    endpoint.clone(),
                    token.clone(),
                    state.clone(),
                    weak.clone(),
                    config,
                ),
                Err(error) => set_error(&weak, error.to_string()),
            }
        });
    }
    {
        let runtime = runtime.clone();
        let endpoint = endpoint.clone();
        let token = token.clone();
        let state = state.clone();
        let weak = ui.as_weak();
        ui.on_delete_topic(move |id| {
            let Ok(id) = Uuid::parse_str(id.as_str()) else {
                set_error(&weak, "invalid topic id".into());
                return;
            };
            let mut config = state.lock().expect("config mutex poisoned").clone();
            config.topics.retain(|topic| topic.id != id);
            persist(
                &runtime,
                endpoint.clone(),
                token.clone(),
                state.clone(),
                weak.clone(),
                config,
            );
        });
    }
    {
        let runtime = runtime.clone();
        let endpoint = endpoint.clone();
        let token = token.clone();
        let weak = ui.as_weak();
        ui.on_reconnect_topic(move |id| {
            let Ok(id) = Uuid::parse_str(id.as_str()) else {
                set_error(&weak, "invalid topic id".into());
                return;
            };
            send_simple(
                &runtime,
                endpoint.clone(),
                token.clone(),
                weak.clone(),
                Request::ReconnectTopic { topic_id: id },
            );
        });
    }
    {
        let runtime = runtime.clone();
        let endpoint = endpoint.clone();
        let token = token.clone();
        let state = state.clone();
        let weak = ui.as_weak();
        ui.on_save_preferences(
            move |theme, language, autostart, minimized, auto_copy, sound, timeout| {
                let mut config = state.lock().expect("config mutex poisoned").clone();
                config.theme = match theme {
                    0 => ThemeMode::Light,
                    1 => ThemeMode::Dark,
                    _ => ThemeMode::System,
                };
                config.language = language.into();
                config.autostart = autostart;
                config.start_minimized = minimized;
                config.notifications.auto_copy = auto_copy;
                config.notifications.sound = sound;
                config.notifications.timeout_seconds = timeout.max(0) as u32;
                persist(
                    &runtime,
                    endpoint.clone(),
                    token.clone(),
                    state.clone(),
                    weak.clone(),
                    config,
                );
            },
        );
    }
    {
        let runtime = runtime.clone();
        let endpoint = endpoint.clone();
        let token = token.clone();
        let weak = ui.as_weak();
        ui.on_quit_daemon(move || {
            send_simple(
                &runtime,
                endpoint.clone(),
                token.clone(),
                weak.clone(),
                Request::Shutdown,
            );
            let _ = slint::quit_event_loop();
        });
    }
    {
        let runtime = runtime.clone();
        let endpoint = endpoint.clone();
        let token = token.clone();
        let weak = ui.as_weak();
        ui.on_check_updates(move || {
            let weak = weak.clone();
            let endpoint = endpoint.clone();
            let token = token.clone();
            runtime.spawn(async move {
                match request(&endpoint, &token, Request::CheckForUpdates).await {
                    Ok(ResponseEnvelope {
                        result: Ok(Response::UpdateStatus(status)),
                        ..
                    }) => {
                        let text = if status.available {
                            if status.verified_asset_available {
                                format!(
                                    "Version {} is available. The artifact has a SHA-256 sidecar; open {} to update.",
                                    status.latest_version.unwrap_or_default(),
                                    status.release_url.unwrap_or_default()
                                )
                            } else {
                                format!(
                                    "Version {} is available, but no verified artifact is published. Open {} for manual instructions.",
                                    status.latest_version.unwrap_or_default(),
                                    status.release_url.unwrap_or_default()
                                )
                            }
                        } else {
                            format!("Version {} is up to date.", status.current_version)
                        };
                        let _ = weak.upgrade_in_event_loop(move |ui| {
                            ui.set_update_status(text.into())
                        });
                    }
                    Ok(ResponseEnvelope { result: Err(fault), .. }) => {
                        set_error(&weak, fault.message)
                    }
                    Ok(_) => set_error(&weak, "unexpected daemon response".into()),
                    Err(error) => set_error(&weak, error.to_string()),
                }
            });
        });
    }
}

fn persist(
    runtime: &Runtime,
    endpoint: Endpoint,
    token: String,
    state: Arc<Mutex<AppConfig>>,
    weak: slint::Weak<AppWindow>,
    config: AppConfig,
) {
    runtime.spawn(async move {
        match request(&endpoint, &token, Request::ReplaceConfig(config.clone())).await {
            Ok(ResponseEnvelope {
                result: Ok(Response::Accepted),
                ..
            }) => {
                *state.lock().expect("config mutex poisoned") = config;
                refresh_async(endpoint, token, state, weak).await;
            }
            Ok(ResponseEnvelope {
                result: Err(fault), ..
            }) => set_error(&weak, fault.message),
            Ok(_) => set_error(&weak, "unexpected daemon response".into()),
            Err(error) => set_error(&weak, error.to_string()),
        }
    });
}

fn refresh(
    runtime: &Runtime,
    endpoint: Endpoint,
    token: String,
    state: Arc<Mutex<AppConfig>>,
    weak: slint::Weak<AppWindow>,
) {
    runtime.spawn(refresh_async(endpoint, token, state, weak));
}

async fn refresh_async(
    endpoint: Endpoint,
    token: String,
    state: Arc<Mutex<AppConfig>>,
    weak: slint::Weak<AppWindow>,
) {
    match request(&endpoint, &token, Request::GetSnapshot).await {
        Ok(ResponseEnvelope {
            result: Ok(Response::Snapshot(snapshot)),
            ..
        }) => {
            *state.lock().expect("config mutex poisoned") = snapshot.config.clone();
            let _ = weak.upgrade_in_event_loop(move |ui| apply_snapshot(&ui, snapshot));
        }
        Ok(ResponseEnvelope {
            result: Err(fault), ..
        }) => set_error(&weak, fault.message),
        Ok(_) => set_error(&weak, "unexpected daemon response".into()),
        Err(error) => set_error(&weak, error.to_string()),
    }
}

fn send_simple(
    runtime: &Runtime,
    endpoint: Endpoint,
    token: String,
    weak: slint::Weak<AppWindow>,
    operation: Request,
) {
    runtime.spawn(async move {
        match request(&endpoint, &token, operation).await {
            Ok(ResponseEnvelope { result: Ok(_), .. }) => set_error(&weak, String::new()),
            Ok(ResponseEnvelope {
                result: Err(fault), ..
            }) => set_error(&weak, fault.message),
            Err(error) => set_error(&weak, error.to_string()),
        }
    });
}

fn set_error(weak: &slint::Weak<AppWindow>, message: String) {
    let weak = weak.clone();
    let _ = weak.upgrade_in_event_loop(move |ui| ui.set_error_message(message.into()));
}

fn apply_snapshot(ui: &AppWindow, snapshot: DaemonSnapshot) {
    let config = &snapshot.config;
    ui.set_servers(ModelRc::from(Rc::new(VecModel::from(server_rows(config)))));
    ui.set_topics(ModelRc::from(Rc::new(VecModel::from(topic_rows(
        config,
        &snapshot.topics,
    )))));
    ui.set_theme_mode(match config.theme {
        ThemeMode::Light => 0,
        ThemeMode::Dark => 1,
        ThemeMode::System => 2,
    });
    ui.invoke_apply_theme(ui.get_theme_mode());
    ui.set_language(config.language.clone().into());
    ui.set_autostart(config.autostart);
    ui.set_start_minimized(config.start_minimized);
    ui.set_auto_copy(config.notifications.auto_copy);
    ui.set_notification_sound(config.notifications.sound);
    ui.set_timeout_seconds(config.notifications.timeout_seconds as i32);
    let connected = snapshot
        .topics
        .iter()
        .filter(|status| status.state == ConnectionState::Connected)
        .count();
    let daemon_status = if config.language == "zh-CN" {
        format!("{connected}/{} 已连接", config.topics.len())
    } else {
        format!("{connected}/{} connected", config.topics.len())
    };
    ui.set_daemon_status(daemon_status.into());
    ui.set_app_version(snapshot.version.into());
    ui.set_error_message(SharedString::default());
}

fn server_rows(config: &AppConfig) -> Vec<ServerRow> {
    config
        .servers
        .iter()
        .map(|server| ServerRow {
            id: server.id.to_string().into(),
            name: server.name.clone().into(),
            url: server.base_url.to_string().into(),
            auth: server
                .credentials
                .as_ref()
                .map(|credentials| credentials.username.clone())
                .unwrap_or_default()
                .into(),
        })
        .collect()
}

fn topic_rows(config: &AppConfig, statuses: &[TopicStatus]) -> Vec<TopicRow> {
    let servers: HashMap<_, _> = config
        .servers
        .iter()
        .map(|server| (server.id, server.name.as_str()))
        .collect();
    let statuses: HashMap<_, _> = statuses
        .iter()
        .map(|status| (status.topic_id, status.state))
        .collect();
    config
        .topics
        .iter()
        .map(|topic| TopicRow {
            id: topic.id.to_string().into(),
            name: topic.name.clone().into(),
            server: servers
                .get(&topic.server_id)
                .copied()
                .unwrap_or(if config.language == "zh-CN" {
                    "服务器不存在"
                } else {
                    "Missing server"
                })
                .into(),
            protocol: match topic.protocol {
                SubscriptionProtocol::WebSocket => "WebSocket",
                SubscriptionProtocol::HttpStream => "HTTP Stream",
            }
            .into(),
            status: state_label(
                statuses
                    .get(&topic.id)
                    .copied()
                    .unwrap_or(ConnectionState::Connecting),
                config.language == "zh-CN",
            )
            .into(),
        })
        .collect()
}

fn state_label(state: ConnectionState, chinese: bool) -> &'static str {
    match (state, chinese) {
        (ConnectionState::Disabled, true) => "已禁用",
        (ConnectionState::Connecting, true) => "连接中",
        (ConnectionState::Connected, true) => "已连接",
        (ConnectionState::WaitingToRetry, true) => "等待重试",
        (ConnectionState::AuthenticationFailed, true) => "认证失败",
        (ConnectionState::Failed, true) => "连接失败",
        (ConnectionState::Disabled, false) => "Disabled",
        (ConnectionState::Connecting, false) => "Connecting",
        (ConnectionState::Connected, false) => "Connected",
        (ConnectionState::WaitingToRetry, false) => "Waiting to retry",
        (ConnectionState::AuthenticationFailed, false) => "Authentication failed",
        (ConnectionState::Failed, false) => "Failed",
    }
}
