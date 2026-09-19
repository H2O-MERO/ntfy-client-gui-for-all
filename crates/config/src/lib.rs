//! Versioned configuration, atomic persistence, and non-destructive format import.

use directories::ProjectDirs;
use rand::RngCore;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};
use thiserror::Error;
use url::Url;
use uuid::Uuid;

pub const CONFIG_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ThemeMode {
    Light,
    Dark,
    #[default]
    System,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionProtocol {
    #[default]
    WebSocket,
    HttpStream,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BasicCredentials {
    pub username: String,
    #[serde(
        serialize_with = "serialize_secret",
        deserialize_with = "deserialize_secret"
    )]
    pub password: SecretString,
}

fn serialize_secret<S>(secret: &SecretString, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(secret.expose_secret())
}

fn deserialize_secret<'de, D>(deserializer: D) -> Result<SecretString, D::Error>
where
    D: serde::Deserializer<'de>,
{
    String::deserialize(deserializer).map(SecretString::from)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub id: Uuid,
    pub name: String,
    pub base_url: Url,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credentials: Option<BasicCredentials>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopicConfig {
    pub id: Uuid,
    pub server_id: Uuid,
    pub name: String,
    #[serde(default)]
    pub protocol: SubscriptionProtocol,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

const fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationSettings {
    pub timeout_seconds: u32,
    pub auto_copy: bool,
    pub sound: bool,
    pub show_actions: bool,
}

impl Default for NotificationSettings {
    fn default() -> Self {
        Self {
            timeout_seconds: 5,
            auto_copy: false,
            sound: true,
            show_actions: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReconnectSettings {
    /// Zero means retry forever.
    pub max_attempts: u32,
    pub initial_delay_seconds: u64,
    pub max_delay_seconds: u64,
}

impl Default for ReconnectSettings {
    fn default() -> Self {
        Self {
            max_attempts: 10,
            initial_delay_seconds: 3,
            max_delay_seconds: 300,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub version: u32,
    pub servers: Vec<ServerConfig>,
    pub topics: Vec<TopicConfig>,
    pub notifications: NotificationSettings,
    pub reconnect: ReconnectSettings,
    pub language: String,
    pub theme: ThemeMode,
    pub autostart: bool,
    pub start_minimized: bool,
    pub check_updates_on_start: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            servers: Vec::new(),
            topics: Vec::new(),
            notifications: NotificationSettings::default(),
            reconnect: ReconnectSettings::default(),
            language: "zh-CN".into(),
            theme: ThemeMode::System,
            autostart: false,
            start_minimized: false,
            check_updates_on_start: true,
        }
    }
}

impl AppConfig {
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.version != CONFIG_VERSION {
            return Err(ConfigError::UnsupportedVersion(self.version));
        }
        let servers: HashMap<_, _> = self
            .servers
            .iter()
            .map(|server| (server.id, server))
            .collect();
        if servers.len() != self.servers.len() {
            return Err(ConfigError::Validation("duplicate server id".into()));
        }
        let mut topic_ids = std::collections::HashSet::new();
        for server in &self.servers {
            if !matches!(server.base_url.scheme(), "http" | "https") {
                return Err(ConfigError::Validation(format!(
                    "server {} must use http or https",
                    server.name
                )));
            }
        }
        for topic in &self.topics {
            if !topic_ids.insert(topic.id) {
                return Err(ConfigError::Validation("duplicate topic id".into()));
            }
            if !servers.contains_key(&topic.server_id) {
                return Err(ConfigError::Validation(format!(
                    "topic {} references a missing server",
                    topic.name
                )));
            }
            if topic.name.trim().is_empty() || topic.name.contains('/') {
                return Err(ConfigError::Validation(
                    "topic names must be non-empty path segments".into(),
                ));
            }
        }
        if self.reconnect.max_delay_seconds < self.reconnect.initial_delay_seconds {
            return Err(ConfigError::Validation(
                "reconnect max delay must not be below initial delay".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("could not determine the per-user configuration directory")]
    NoConfigDirectory,
    #[error("configuration I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("configuration JSON failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("URL is invalid: {0}")]
    Url(#[from] url::ParseError),
    #[error("unsupported configuration version {0}")]
    UnsupportedVersion(u32),
    #[error("invalid configuration: {0}")]
    Validation(String),
}

#[derive(Debug, Clone)]
pub struct ConfigStore {
    root: PathBuf,
}

impl ConfigStore {
    pub fn discover(instance: &str) -> Result<Self, ConfigError> {
        let dirs = ProjectDirs::from("io", "H2O-MERO", "ntfy-client-gui-for-all")
            .ok_or(ConfigError::NoConfigDirectory)?;
        let safe_instance: String = instance
            .chars()
            .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
            .take(64)
            .collect();
        if safe_instance.is_empty() {
            return Err(ConfigError::Validation(
                "instance id is empty or invalid".into(),
            ));
        }
        Ok(Self {
            root: dirs.config_dir().join(safe_instance),
        })
    }

    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn config_path(&self) -> PathBuf {
        self.root.join("config.json")
    }

    pub fn load_or_create(&self) -> Result<AppConfig, ConfigError> {
        let path = self.config_path();
        if !path.exists() {
            let config = AppConfig::default();
            self.save(&config)?;
            return Ok(config);
        }
        let config: AppConfig = serde_json::from_slice(&fs::read(path)?)?;
        config.validate()?;
        Ok(config)
    }

    pub fn save(&self, config: &AppConfig) -> Result<(), ConfigError> {
        config.validate()?;
        fs::create_dir_all(&self.root)?;
        restrict_directory(&self.root)?;
        atomic_write(&self.config_path(), &serde_json::to_vec_pretty(config)?)?;
        Ok(())
    }

    pub fn load_or_create_ipc_token(&self) -> Result<String, ConfigError> {
        fs::create_dir_all(&self.root)?;
        restrict_directory(&self.root)?;
        let path = self.root.join("ipc-token");
        if path.exists() {
            let token = fs::read_to_string(&path)?;
            let token = token.trim();
            if token.len() >= 64 {
                return Ok(token.to_owned());
            }
        }
        let mut bytes = [0_u8; 32];
        rand::rng().fill_bytes(&mut bytes);
        let token: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        atomic_write(&path, token.as_bytes())?;
        Ok(token)
    }

    /// Imports compatible files without modifying or deleting them.
    pub fn import_compatible(
        &self,
        import_dir: &Path,
    ) -> Result<Option<ImportReport>, ConfigError> {
        if self.config_path().exists() {
            return Ok(None);
        }
        let settings_path = import_dir.join("settings.json");
        let topics_json_path = import_dir.join("topics.json");
        let topics_txt_path = import_dir.join("topics.txt");
        if !settings_path.exists() && !topics_json_path.exists() && !topics_txt_path.exists() {
            return Ok(None);
        }

        let mut config = AppConfig::default();
        let mut report = ImportReport::default();
        if settings_path.exists() {
            let old: ImportSettings = serde_json::from_slice(&fs::read(&settings_path)?)?;
            config.notifications.timeout_seconds = old.timeout.max(0.0).ceil() as u32;
            config.notifications.auto_copy = old.native_notifications_auto_copy_to_clipboard;
            config.notifications.sound = old.custom_tray_notifications_play_default_windows_sound;
            config.reconnect.max_attempts = old.reconnect_attempts.max(0.0).ceil() as u32;
            config.reconnect.initial_delay_seconds =
                old.reconnect_attempt_delay.max(0.0).ceil() as u64;
            config.language = old.language;
            config.autostart = old.auto_start_enabled;
            config.start_minimized = old.auto_start_silent;
            report.settings_imported = true;
        }

        let imported_topics = if topics_json_path.exists() {
            serde_json::from_slice::<Vec<ImportTopic>>(&fs::read(&topics_json_path)?)?
        } else if topics_txt_path.exists() {
            fs::read_to_string(&topics_txt_path)?
                .lines()
                .filter(|line| !line.trim().is_empty())
                .map(|name| ImportTopic {
                    topic_id: name.trim().to_owned(),
                    server_url: "https://ntfy.sh".into(),
                    username: None,
                    password: None,
                })
                .collect()
        } else {
            Vec::new()
        };

        let mut server_ids: HashMap<(String, Option<String>), Uuid> = HashMap::new();
        for old in imported_topics {
            let mut url = Url::parse(&old.server_url)?;
            let protocol = match url.scheme() {
                "ws" => {
                    url.set_scheme("http").expect("known scheme replacement");
                    SubscriptionProtocol::WebSocket
                }
                "wss" => {
                    url.set_scheme("https").expect("known scheme replacement");
                    SubscriptionProtocol::WebSocket
                }
                _ => SubscriptionProtocol::HttpStream,
            };
            let username = old.username.filter(|value| !value.trim().is_empty());
            let key = (url.to_string(), username.clone());
            let server_id = *server_ids.entry(key).or_insert_with(|| {
                let id = Uuid::new_v4();
                config.servers.push(ServerConfig {
                    id,
                    name: url.host_str().unwrap_or("ntfy server").to_owned(),
                    base_url: url.clone(),
                    credentials: username.clone().map(|username| BasicCredentials {
                        username,
                        password: SecretString::from(old.password.clone().unwrap_or_default()),
                    }),
                });
                id
            });
            config.topics.push(TopicConfig {
                id: Uuid::new_v4(),
                server_id,
                name: old.topic_id,
                protocol,
                enabled: true,
            });
            report.topics_imported += 1;
        }
        self.save(&config)?;
        report.source_files_preserved = true;
        Ok(Some(report))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportReport {
    pub settings_imported: bool,
    pub topics_imported: usize,
    pub source_files_preserved: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ImportSettings {
    #[serde(default)]
    timeout: f64,
    #[serde(default)]
    reconnect_attempts: f64,
    #[serde(default)]
    reconnect_attempt_delay: f64,
    #[serde(default)]
    custom_tray_notifications_play_default_windows_sound: bool,
    #[serde(default)]
    native_notifications_auto_copy_to_clipboard: bool,
    #[serde(default)]
    auto_start_enabled: bool,
    #[serde(default)]
    auto_start_silent: bool,
    #[serde(default = "default_language")]
    language: String,
}

fn default_language() -> String {
    "zh-CN".into()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ImportTopic {
    topic_id: String,
    server_url: String,
    username: Option<String>,
    password: Option<String>,
}

fn atomic_write(path: &Path, content: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("path has no parent"))?;
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name().unwrap_or_default().to_string_lossy(),
        Uuid::new_v4()
    ));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(content)?;
        file.sync_all()?;
        replace_file(&temporary, path).map_err(|error| {
            io::Error::new(
                error.kind(),
                format!(
                    "atomic rename {} -> {} failed: {error}",
                    temporary.display(),
                    path.display()
                ),
            )
        })?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(not(windows))]
fn replace_file(source: &Path, destination: &Path) -> io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };

    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    // SAFETY: Both pointers refer to valid, NUL-terminated UTF-16 buffers for the call.
    let moved = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if moved == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(unix)]
fn restrict_directory(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn restrict_directory(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_revision_four_without_deleting_sources() {
        let source = tempfile::tempdir().unwrap();
        fs::write(
            source.path().join("settings.json"),
            r#"{
                "Revision": 4,
                "Timeout": 7,
                "ReconnectAttempts": 0,
                "ReconnectAttemptDelay": 4,
                "NotificationsMethod": 0,
                "CustomTrayNotificationsShowTimeoutBar": true,
                "CustomTrayNotificationsShowInDarkMode": false,
                "CustomTrayNotificationsPlayDefaultWindowsSound": true,
                "NativeNotificationsAutoCopyToClipboard": true,
                "AutoStartEnabled": true,
                "AutoStartSilent": true,
                "Language": "en-US"
            }"#,
        )
        .unwrap();
        fs::write(
            source.path().join("topics.json"),
            r#"[{"TopicId":"alerts","ServerUrl":"wss://ntfy.sh","Username":"alice","Password":"secret"}]"#,
        )
        .unwrap();
        let destination = tempfile::tempdir().unwrap();
        let store = ConfigStore::at(destination.path());
        let report = store.import_compatible(source.path()).unwrap().unwrap();
        assert_eq!(report.topics_imported, 1);
        assert!(source.path().join("topics.json").exists());
        let imported = store.load_or_create().unwrap();
        assert_eq!(imported.topics[0].protocol, SubscriptionProtocol::WebSocket);
        assert_eq!(imported.notifications.timeout_seconds, 7);
        assert!(imported.notifications.auto_copy);
        assert_eq!(
            imported.servers[0]
                .credentials
                .as_ref()
                .unwrap()
                .password
                .expose_secret(),
            "secret"
        );
    }

    #[test]
    fn imports_topic_text_non_destructively() {
        let source = tempfile::tempdir().unwrap();
        fs::write(source.path().join("topics.txt"), "one\n\ntwo\n").unwrap();
        let destination = tempfile::tempdir().unwrap();
        let store = ConfigStore::at(destination.path());
        let report = store.import_compatible(source.path()).unwrap().unwrap();
        assert_eq!(report.topics_imported, 2);
        assert!(source.path().join("topics.txt").exists());
    }

    #[test]
    fn rejects_missing_server_reference() {
        let config = AppConfig {
            topics: vec![TopicConfig {
                id: Uuid::new_v4(),
                server_id: Uuid::new_v4(),
                name: "alerts".into(),
                protocol: SubscriptionProtocol::HttpStream,
                enabled: true,
            }],
            ..AppConfig::default()
        };
        assert!(matches!(config.validate(), Err(ConfigError::Validation(_))));
    }
}
