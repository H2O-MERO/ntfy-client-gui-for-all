#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use anyhow::{Context, Result};
use ntfy_pusher_config::{AppConfig, ConfigStore};
use ntfy_pusher_core::{CoreEvent, SubscriptionSupervisor, UpdateChecker, event_channel, unix_now};
use ntfy_pusher_ipc::{
    DaemonSnapshot, Endpoint, IpcFault, LocalServer, Request, RequestEnvelope, Response,
    ResponseEnvelope, TopicStatus, read_frame, write_frame,
};
use ntfy_pusher_platform::{
    Autostart, NativeNotificationBackend, NotificationBackend, NotificationRequest, TrayCommand,
    spawn_tray,
};
use ntfy_pusher_protocol::Priority;
use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
};
use tokio::sync::{Mutex, RwLock, broadcast, watch};
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

const APP_ID: &str = "io.github.h2omero.ntfy-pusher";

#[derive(Debug)]
struct Options {
    instance: String,
    allow_multiple: bool,
    start_in_tray: bool,
    legacy_dir: Option<PathBuf>,
    config_dir: Option<PathBuf>,
}

fn print_help() {
    println!(
        "ntfy-pusher {}\n\n\
         Usage: ntfy-pusher [OPTIONS]\n\n\
         Options:\n\
           -h, --help                        Show this help\n\
           -t, --start-in-tray               Compatibility alias; daemon always runs in background\n\
           -m, --allow-multiple-instances    Start an isolated PID-named instance\n\
               --instance <NAME>             Select an isolated config and IPC namespace\n\
               --config-dir <PATH>           Override the per-user config root (testing/portable use)\n\
               --legacy-dir <PATH>           Import legacy settings/topics without deleting them",
        env!("CARGO_PKG_VERSION")
    );
}

fn parse_options() -> Result<Option<Options>> {
    let mut instance = "default".to_owned();
    let mut explicit_instance = false;
    let mut allow_multiple = false;
    let mut start_in_tray = false;
    let mut legacy_dir = None;
    let mut config_dir = None;
    let mut args = env::args_os().skip(1);
    while let Some(arg) = args.next() {
        match arg.to_string_lossy().as_ref() {
            "-h" | "--help" => {
                print_help();
                return Ok(None);
            }
            "-t" | "--start-in-tray" => start_in_tray = true,
            "-m" | "--allow-multiple-instances" => allow_multiple = true,
            "--instance" => {
                instance = args
                    .next()
                    .context("--instance requires a value")?
                    .to_string_lossy()
                    .into_owned();
                explicit_instance = true;
            }
            "--legacy-dir" => {
                legacy_dir = Some(PathBuf::from(
                    args.next().context("--legacy-dir requires a path")?,
                ));
            }
            "--config-dir" => {
                config_dir = Some(PathBuf::from(
                    args.next().context("--config-dir requires a path")?,
                ));
            }
            unknown => anyhow::bail!("unknown option {unknown}; use --help"),
        }
    }
    if allow_multiple && !explicit_instance {
        instance = format!("default-{}", std::process::id());
    }
    Ok(Some(Options {
        instance,
        allow_multiple,
        start_in_tray,
        legacy_dir,
        config_dir,
    }))
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(false)
        .compact()
        .init();
    let Some(options) = parse_options()? else {
        return Ok(());
    };
    run(options).await
}

async fn run(options: Options) -> Result<()> {
    let store = options
        .config_dir
        .as_ref()
        .map(|root| ConfigStore::at(root.join(&options.instance)))
        .map(Ok)
        .unwrap_or_else(|| ConfigStore::discover(&options.instance))?;
    if !store.config_path().exists() {
        let legacy_dir = options
            .legacy_dir
            .clone()
            .or_else(current_executable_directory);
        if let Some(legacy_dir) = legacy_dir
            && let Some(report) = store.migrate_legacy(&legacy_dir)?
        {
            info!(
                topics = report.topics_imported,
                "legacy configuration imported; source files preserved"
            );
        }
    }
    let initial_config = store.load_or_create()?;
    let token = store.load_or_create_ipc_token()?;
    let endpoint = Endpoint::for_instance(store.root(), &options.instance);
    let mut server = match LocalServer::bind(endpoint.clone()).await {
        Ok(server) => server,
        Err(error) if !options.allow_multiple => {
            let _ = ntfy_pusher_ipc::request(&endpoint, &token, Request::OpenGui).await;
            info!(%error, "daemon already running; requested settings activation");
            return Ok(());
        }
        Err(error) => return Err(error.into()),
    };

    info!(instance = %options.instance, endpoint = endpoint.display_name(), start_in_tray = options.start_in_tray, "daemon started");
    let config = Arc::new(RwLock::new(initial_config));
    let statuses: Arc<RwLock<Vec<TopicStatus>>> = Arc::new(RwLock::new(Vec::new()));
    let (event_tx, mut event_rx) = event_channel();
    let mut initial_supervisor = SubscriptionSupervisor::new(event_tx)?;
    {
        let initial = config.read().await;
        initial_supervisor.apply_config(&initial).await;
    }
    let supervisor = Arc::new(Mutex::new(initial_supervisor));
    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
    let (ipc_events, _) = broadcast::channel(32);
    let backend = NativeNotificationBackend::new(APP_ID);
    let started = unix_now();
    let (tray_tx, mut tray_rx) = tokio::sync::mpsc::unbounded_channel();
    let _tray = match spawn_tray(move |command| {
        let _ = tray_tx.send(command);
    }) {
        Ok(tray) => Some(tray),
        Err(error) => {
            warn!(%error, "native tray is unavailable; IPC and command-line shutdown remain available");
            None
        }
    };

    let event_config = config.clone();
    let event_statuses = statuses.clone();
    let event_backend = backend.clone();
    let event_ipc = ipc_events.clone();
    let event_task = tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            match event {
                CoreEvent::Status(status) => {
                    upsert_status(&event_statuses, status).await;
                    let snapshot = DaemonSnapshot {
                        config: event_config.read().await.clone(),
                        topics: event_statuses.read().await.clone(),
                        started_unix_seconds: started,
                        version: env!("CARGO_PKG_VERSION").into(),
                    };
                    let _ = event_ipc.send(ntfy_pusher_ipc::Event::SnapshotChanged(snapshot));
                }
                CoreEvent::Notification { server, event, .. } => {
                    let event = *event;
                    let settings = event_config.read().await.notifications.clone();
                    let request = NotificationRequest {
                        title: event.notification_title(&server.base_url),
                        body: event.message,
                        priority: event.priority.unwrap_or(Priority::DEFAULT),
                        timeout_seconds: settings.timeout_seconds,
                        auto_copy: settings.auto_copy,
                        play_sound: settings.sound,
                        actions: if settings.show_actions {
                            event.actions
                        } else {
                            Vec::new()
                        },
                    };
                    let backend = event_backend.clone();
                    tokio::task::spawn_blocking(move || {
                        if let Err(error) = backend.show(request) {
                            warn!(%error, "native notification failed");
                        }
                    });
                }
            }
        }
    });

    loop {
        tokio::select! {
            accepted = server.accept() => match accepted {
                Ok(stream) => {
                    let context = ClientContext {
                        token: token.clone(),
                        instance: options.instance.clone(),
                        store: store.clone(),
                        config: config.clone(),
                        statuses: statuses.clone(),
                        supervisor: supervisor.clone(),
                        shutdown: shutdown_tx.clone(),
                        events: ipc_events.clone(),
                        started,
                    };
                    tokio::spawn(async move {
                        if let Err(error) = handle_client(stream, context).await {
                            warn!(%error, "IPC client failed");
                        }
                    });
                }
                Err(error) => error!(%error, "IPC accept failed"),
            },
            _ = tokio::signal::ctrl_c() => {
                info!("interrupt received");
                break;
            }
            changed = shutdown_rx.changed() => {
                if changed.is_err() || *shutdown_rx.borrow() { break; }
            }
            Some(command) = tray_rx.recv() => match command {
                TrayCommand::OpenSettings => {
                    if let Err(error) = open_gui(&options.instance) {
                        warn!(%error, "could not open settings from tray");
                    }
                }
                TrayCommand::CheckForUpdates => {
                    let backend = backend.clone();
                    tokio::spawn(async move {
                        let result = async {
                            let checker = UpdateChecker::new()?;
                            let current = semver::Version::parse(env!("CARGO_PKG_VERSION"))?;
                            Ok::<_, anyhow::Error>(checker.check(&current).await?)
                        }.await;
                        let (title, body) = match result {
                            Ok(status) if status.available => (
                                "ntfy pusher update",
                                format!("Version {} is available. Open settings for details.", status.latest_version.unwrap_or_default()),
                            ),
                            Ok(status) => (
                                "ntfy pusher",
                                format!("Version {} is up to date.", status.current_version),
                            ),
                            Err(error) => ("ntfy pusher update", format!("Update check failed: {error}")),
                        };
                        let request = NotificationRequest {
                            title: title.into(), body, priority: Priority::DEFAULT,
                            timeout_seconds: 8, auto_copy: false, play_sound: false, actions: Vec::new(),
                        };
                        let _ = tokio::task::spawn_blocking(move || backend.show(request)).await;
                    });
                }
                TrayCommand::Quit => break,
            }
        }
    }

    supervisor.lock().await.shutdown().await;
    event_task.abort();
    info!("daemon stopped");
    Ok(())
}

#[derive(Clone)]
struct ClientContext {
    token: String,
    instance: String,
    store: ConfigStore,
    config: Arc<RwLock<AppConfig>>,
    statuses: Arc<RwLock<Vec<TopicStatus>>>,
    supervisor: Arc<Mutex<SubscriptionSupervisor>>,
    shutdown: watch::Sender<bool>,
    events: broadcast::Sender<ntfy_pusher_ipc::Event>,
    started: i64,
}

async fn handle_client(
    mut stream: ntfy_pusher_ipc::BoxedStream,
    context: ClientContext,
) -> Result<()> {
    let request: RequestEnvelope = read_frame(&mut stream).await?;
    if !tokens_equal(&request.token, &context.token) {
        write_frame(
            &mut stream,
            &ResponseEnvelope {
                request_id: request.request_id,
                result: Err(IpcFault::unauthorized()),
            },
        )
        .await?;
        return Ok(());
    }
    if matches!(request.request, Request::WatchEvents) {
        let mut events = context.events.subscribe();
        write_frame(
            &mut stream,
            &ResponseEnvelope {
                request_id: request.request_id,
                result: Ok(Response::Accepted),
            },
        )
        .await?;
        write_frame(
            &mut stream,
            &ntfy_pusher_ipc::Event::SnapshotChanged(snapshot(&context).await),
        )
        .await?;
        loop {
            match events.recv().await {
                Ok(event) => write_frame(&mut stream, &event).await?,
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    write_frame(
                        &mut stream,
                        &ntfy_pusher_ipc::Event::SnapshotChanged(snapshot(&context).await),
                    )
                    .await?;
                }
                Err(broadcast::error::RecvError::Closed) => return Ok(()),
            }
        }
    }
    let result = { dispatch(request.request, &context).await };
    write_frame(
        &mut stream,
        &ResponseEnvelope {
            request_id: request.request_id,
            result,
        },
    )
    .await?;
    Ok(())
}

async fn dispatch(request: Request, context: &ClientContext) -> Result<Response, IpcFault> {
    match request {
        Request::Ping => Ok(Response::Pong),
        Request::GetSnapshot => Ok(Response::Snapshot(snapshot(context).await)),
        Request::WatchEvents => unreachable!("event streams are handled before dispatch"),
        Request::ReplaceConfig(replacement) => {
            replacement.validate().map_err(fault)?;
            context.store.save(&replacement).map_err(fault)?;
            apply_autostart(&replacement, &context.instance).map_err(fault)?;
            context
                .supervisor
                .lock()
                .await
                .apply_config(&replacement)
                .await;
            *context.config.write().await = replacement;
            context.statuses.write().await.clear();
            let _ = context.events.send(ntfy_pusher_ipc::Event::SnapshotChanged(
                snapshot(context).await,
            ));
            Ok(Response::Accepted)
        }
        Request::ConnectTopic { topic_id } | Request::ReconnectTopic { topic_id } => {
            context.supervisor.lock().await.stop(topic_id).await;
            let config = context.config.read().await;
            let topic = config
                .topics
                .iter()
                .find(|topic| topic.id == topic_id)
                .cloned()
                .ok_or_else(|| IpcFault::new("not_found", "topic was not found"))?;
            let server = config
                .servers
                .iter()
                .find(|server| server.id == topic.server_id)
                .cloned()
                .ok_or_else(|| IpcFault::new("not_found", "topic server was not found"))?;
            context
                .supervisor
                .lock()
                .await
                .start(server, topic, config.reconnect.clone());
            Ok(Response::Accepted)
        }
        Request::DisconnectTopic { topic_id } => {
            context.supervisor.lock().await.stop(topic_id).await;
            Ok(Response::Accepted)
        }
        Request::OpenGui => {
            open_gui(&context.instance).map_err(fault)?;
            Ok(Response::Accepted)
        }
        Request::CheckForUpdates => {
            let checker = UpdateChecker::new().map_err(fault)?;
            let current = semver::Version::parse(env!("CARGO_PKG_VERSION")).map_err(fault)?;
            let status = checker.check(&current).await.map_err(fault)?;
            Ok(Response::UpdateStatus(status))
        }
        Request::Shutdown => {
            let _ = context.shutdown.send(true);
            Ok(Response::Accepted)
        }
    }
}

async fn snapshot(context: &ClientContext) -> DaemonSnapshot {
    DaemonSnapshot {
        config: context.config.read().await.clone(),
        topics: context.statuses.read().await.clone(),
        started_unix_seconds: context.started,
        version: env!("CARGO_PKG_VERSION").into(),
    }
}

async fn upsert_status(statuses: &RwLock<Vec<TopicStatus>>, status: TopicStatus) {
    let mut statuses = statuses.write().await;
    if let Some(existing) = statuses
        .iter_mut()
        .find(|existing| existing.topic_id == status.topic_id)
    {
        if status.last_event_unix_seconds.is_none() {
            let last_event = existing.last_event_unix_seconds;
            *existing = status;
            existing.last_event_unix_seconds = last_event;
        } else {
            *existing = status;
        }
    } else {
        statuses.push(status);
    }
}

fn tokens_equal(left: &str, right: &str) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.bytes()
        .zip(right.bytes())
        .fold(0_u8, |difference, (a, b)| difference | (a ^ b))
        == 0
}

fn apply_autostart(config: &AppConfig, instance: &str) -> Result<()> {
    let executable = env::current_exe()?;
    Autostart::new(&executable, instance, config.start_minimized)?.set_enabled(config.autostart)?;
    Ok(())
}

fn open_gui(instance: &str) -> Result<()> {
    let mut executable = env::current_exe()?;
    executable.set_file_name(if cfg!(windows) {
        "ntfy-pusher-gui.exe"
    } else {
        "ntfy-pusher-gui"
    });
    Command::new(executable)
        .args(["--instance", instance])
        .spawn()?;
    Ok(())
}

fn current_executable_directory() -> Option<PathBuf> {
    env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))
}

fn fault(error: impl std::fmt::Display) -> IpcFault {
    IpcFault::new("operation_failed", error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_comparison_handles_lengths_and_content() {
        assert!(tokens_equal("abc", "abc"));
        assert!(!tokens_equal("abc", "abd"));
        assert!(!tokens_equal("abc", "ab"));
    }
}
