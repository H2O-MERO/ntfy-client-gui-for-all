//! Native notification, clipboard, and autostart adapters.

use ntfy_client_protocol::{NtfyAction, Priority};
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCommand {
    OpenSettings,
    CheckForUpdates,
    Quit,
}

pub struct TrayHandle {
    #[allow(dead_code)]
    thread: std::thread::JoinHandle<()>,
}

#[derive(Debug, Clone)]
pub struct NotificationRequest {
    pub title: String,
    pub body: String,
    pub priority: Priority,
    pub timeout_seconds: u32,
    pub auto_copy: bool,
    pub play_sound: bool,
    pub click: Option<url::Url>,
    pub actions: Vec<NtfyAction>,
}

#[derive(Debug, Error)]
pub enum PlatformError {
    #[error("notification failed: {0}")]
    Notification(String),
    #[error("clipboard failed: {0}")]
    Clipboard(String),
    #[error("autostart failed: {0}")]
    Autostart(String),
    #[error("tray failed: {0}")]
    Tray(String),
}

/// Starts a native tray/status indicator without initializing Slint.
///
/// macOS requires its status item on the process main AppKit thread. The daemon's
/// main-loop integration for that platform is intentionally reported as unsupported
/// until it can satisfy that invariant rather than creating an unreliable item.
#[cfg(not(target_os = "macos"))]
pub fn spawn_tray(
    handler: impl Fn(TrayCommand) + Send + Sync + 'static,
) -> Result<TrayHandle, PlatformError> {
    use std::sync::Arc;
    use tray_icon::{
        Icon, TrayIconBuilder,
        menu::{Menu, MenuEvent, MenuItem},
    };

    let handler = Arc::new(handler);
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
    let thread = std::thread::Builder::new()
        .name("ntfy-tray".into())
        .spawn(move || {
            let result = (|| -> Result<(), PlatformError> {
                let open = MenuItem::with_id("open", "Open settings", true, None);
                let update = MenuItem::with_id("update", "Check for updates", true, None);
                let quit = MenuItem::with_id("quit", "Quit", true, None);
                let menu = Menu::with_items(&[&open, &update, &quit])
                    .map_err(|error| PlatformError::Tray(error.to_string()))?;
                let open_id = open.id().clone();
                let update_id = update.id().clone();
                let quit_id = quit.id().clone();
                let callback = handler.clone();
                MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
                    if event.id() == &open_id {
                        callback(TrayCommand::OpenSettings);
                    } else if event.id() == &update_id {
                        callback(TrayCommand::CheckForUpdates);
                    } else if event.id() == &quit_id {
                        callback(TrayCommand::Quit);
                    }
                }));
                let icon = Icon::from_rgba(tray_icon_rgba(), 32, 32)
                    .map_err(|error| PlatformError::Tray(error.to_string()))?;
                let tray = TrayIconBuilder::new()
                    .with_menu(Box::new(menu))
                    .with_tooltip("Ntfy Client GUI")
                    .with_icon(icon)
                    .build()
                    .map_err(|error| PlatformError::Tray(error.to_string()))?;
                ready_tx.send(Ok(())).ok();

                #[cfg(windows)]
                unsafe {
                    use windows_sys::Win32::UI::WindowsAndMessaging::{
                        DispatchMessageW, GetMessageW, MSG, TranslateMessage,
                    };
                    // SAFETY: The tray thread owns this standard Win32 message loop.
                    let mut message: MSG = std::mem::zeroed();
                    while GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) > 0 {
                        TranslateMessage(&message);
                        DispatchMessageW(&message);
                    }
                }
                #[cfg(target_os = "linux")]
                {
                    let _keep_tray_alive = tray;
                    loop {
                        std::thread::park();
                    }
                }
                #[cfg(windows)]
                {
                    drop(tray);
                    Ok(())
                }
            })();
            if let Err(error) = result {
                ready_tx.send(Err(error)).ok();
            }
        })
        .map_err(|error| PlatformError::Tray(error.to_string()))?;
    ready_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .map_err(|error| PlatformError::Tray(error.to_string()))??;
    Ok(TrayHandle { thread })
}

#[cfg(target_os = "macos")]
pub fn spawn_tray(
    _handler: impl Fn(TrayCommand) + Send + Sync + 'static,
) -> Result<TrayHandle, PlatformError> {
    Err(PlatformError::Tray(
        "macOS status item requires main-thread AppKit integration".into(),
    ))
}

fn tray_icon_rgba() -> Vec<u8> {
    let mut pixels = vec![0_u8; 32 * 32 * 4];
    for y in 0_usize..32 {
        for x in 0_usize..32 {
            let index = (y * 32 + x) * 4;
            let rounded = (4..28).contains(&x) && (4..28).contains(&y);
            let n = ((9..13).contains(&x) || (19..23).contains(&x)) && (8..24).contains(&y)
                || (12..20).contains(&x) && (x + y).abs_diff(31) < 3;
            let (red, green, blue, alpha) = if n {
                (12, 42, 66, 255)
            } else if rounded {
                (96, 184, 244, 255)
            } else {
                (0, 0, 0, 0)
            };
            pixels[index..index + 4].copy_from_slice(&[red, green, blue, alpha]);
        }
    }
    pixels
}

pub trait NotificationBackend: Send + Sync {
    fn show(&self, request: NotificationRequest) -> Result<(), PlatformError>;
}

#[derive(Debug, Clone)]
pub struct NativeNotificationBackend {
    app_id: String,
}

impl NativeNotificationBackend {
    pub fn new(app_id: impl Into<String>) -> Self {
        Self {
            app_id: app_id.into(),
        }
    }
}

pub fn copy_text(text: impl Into<String>) -> Result<(), PlatformError> {
    let mut clipboard =
        arboard::Clipboard::new().map_err(|error| PlatformError::Clipboard(error.to_string()))?;
    clipboard
        .set_text(text.into())
        .map_err(|error| PlatformError::Clipboard(error.to_string()))
}

#[cfg(windows)]
impl NotificationBackend for NativeNotificationBackend {
    fn show(&self, request: NotificationRequest) -> Result<(), PlatformError> {
        use tauri_winrt_notification::{Duration, Scenario, Sound, Toast};

        if request.auto_copy {
            copy_text(request.body.clone())?;
        }
        let copy_body = request.body.clone();
        let button_text = if request.auto_copy {
            "已自动复制 / Copied"
        } else {
            "复制内容 / Copy"
        };
        let mut view_urls = Vec::new();
        let mut toast = Toast::new(&self.app_id)
            .title(&request.title)
            .text1(&request.body)
            .duration(Duration::Short)
            .scenario(if request.priority == Priority::MAX {
                Scenario::Alarm
            } else {
                Scenario::Default
            })
            .sound(request.play_sound.then_some(Sound::Default))
            .add_button(button_text, "copy");

        if let Some(url) = request.click.filter(is_safe_view_url) {
            let index = view_urls.len();
            view_urls.push(url);
            toast = toast.add_button("打开 / Open", &format!("view:{index}"));
        }

        // URL actions are safe to expose as protocol activation arguments, but arbitrary
        // HTTP actions require confirmation UI and are deliberately not auto-executed.
        for action in &request.actions {
            if action.action == "view"
                && let Some(url) = action.url.as_ref().filter(|url| is_safe_view_url(url))
            {
                let index = view_urls.len();
                view_urls.push(url.clone());
                toast = toast.add_button(&action.label, &format!("view:{index}"));
            }
        }
        toast = toast.on_activated(move |action| {
            if action.as_deref() == Some("copy") {
                let _ = copy_text(copy_body.clone());
            } else if let Some(index) = action
                .as_deref()
                .and_then(|value| value.strip_prefix("view:"))
                .and_then(|value| value.parse::<usize>().ok())
                && let Some(url) = view_urls.get(index)
            {
                let _ = webbrowser::open(url.as_str());
            }
            Ok(())
        });
        toast
            .show()
            .map_err(|error| PlatformError::Notification(error.to_string()))
    }
}

#[cfg(not(windows))]
impl NotificationBackend for NativeNotificationBackend {
    fn show(&self, request: NotificationRequest) -> Result<(), PlatformError> {
        use notify_rust::{Notification, Timeout};

        if request.auto_copy {
            copy_text(request.body.clone())?;
        }
        let mut notification = Notification::new();
        notification
            .appname(&self.app_id)
            .summary(&request.title)
            .body(&request.body)
            .timeout(if request.timeout_seconds == 0 {
                Timeout::Never
            } else {
                Timeout::Milliseconds(request.timeout_seconds.saturating_mul(1000))
            })
            .action("copy", if request.auto_copy { "Copied" } else { "Copy" });
        let mut view_urls = Vec::new();
        if let Some(url) = request.click.filter(is_safe_view_url) {
            view_urls.push(url);
            notification.action("view-0", "Open");
        }
        for action in &request.actions {
            if action.action == "view"
                && let Some(url) = action.url.as_ref().filter(|url| is_safe_view_url(url))
            {
                let index = view_urls.len();
                view_urls.push(url.clone());
                notification.action(&format!("view-{index}"), &action.label);
            }
        }
        let body = request.body;
        let handle = notification
            .show()
            .map_err(|error| PlatformError::Notification(error.to_string()))?;
        std::thread::Builder::new()
            .name("ntfy-notification-action".into())
            .spawn(move || {
                handle.wait_for_action(|action| {
                    if action == "copy" {
                        let _ = copy_text(body.clone());
                    } else if let Some(index) = action
                        .strip_prefix("view-")
                        .and_then(|value| value.parse::<usize>().ok())
                        && let Some(url) = view_urls.get(index)
                    {
                        let _ = webbrowser::open(url.as_str());
                    }
                });
            })
            .map_err(|error| PlatformError::Notification(error.to_string()))?;
        Ok(())
    }
}

fn is_safe_view_url(url: &url::Url) -> bool {
    matches!(url.scheme(), "http" | "https")
}

pub struct Autostart {
    inner: auto_launch::AutoLaunch,
}

impl Autostart {
    pub fn new(
        executable: &Path,
        instance: &str,
        start_minimized: bool,
    ) -> Result<Self, PlatformError> {
        let executable = executable
            .to_str()
            .ok_or_else(|| PlatformError::Autostart("executable path is not UTF-8".into()))?;
        let mut args = vec!["--instance", instance];
        if start_minimized {
            args.push("--start-in-tray");
        }
        let inner = auto_launch::AutoLaunchBuilder::new()
            .set_app_name("Ntfy Client GUI")
            .set_app_path(executable)
            .set_args(&args)
            .build()
            .map_err(|error| PlatformError::Autostart(error.to_string()))?;
        Ok(Self { inner })
    }

    pub fn set_enabled(&self, enabled: bool) -> Result<(), PlatformError> {
        let result = if enabled {
            self.inner.enable()
        } else {
            self.inner.disable()
        };
        result.map_err(|error| PlatformError::Autostart(error.to_string()))
    }

    pub fn is_enabled(&self) -> Result<bool, PlatformError> {
        self.inner
            .is_enabled()
            .map_err(|error| PlatformError::Autostart(error.to_string()))
    }
}
