//! Background subscription orchestration. This crate has no GUI dependency.

use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use futures_util::StreamExt;
use ntfy_pusher_config::{
    AppConfig, BasicCredentials, ReconnectSettings, ServerConfig, SubscriptionProtocol, TopicConfig,
};
use ntfy_pusher_ipc::{ConnectionState, TopicStatus};
use ntfy_pusher_protocol::{Deduplicator, EventKind, NdjsonDecoder, NtfyEvent};
use rand::Rng;
use reqwest::StatusCode;
use secrecy::ExposeSecret;
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use thiserror::Error;
use tokio::{
    sync::{mpsc, watch},
    task::JoinHandle,
};
use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest, http::HeaderValue};
use url::Url;
use uuid::Uuid;

mod update;
pub use update::{UpdateChecker, UpdateError, VerifiedDownload};

pub const EVENT_QUEUE_CAPACITY: usize = 256;
const DEDUPLICATION_WINDOW: usize = 1024;

#[derive(Debug)]
pub enum CoreEvent {
    Notification {
        server: ServerConfig,
        topic: TopicConfig,
        event: Box<NtfyEvent>,
    },
    Status(TopicStatus),
}

#[derive(Debug, Error)]
enum SubscribeError {
    #[error("authentication rejected")]
    Authentication,
    #[error("server returned HTTP {0}")]
    HttpStatus(StatusCode),
    #[error("subscription stream ended")]
    Ended,
    #[error("network error: {0}")]
    Network(String),
    #[error("protocol error: {0}")]
    Protocol(String),
}

pub struct SubscriptionSupervisor {
    client: reqwest::Client,
    tasks: HashMap<Uuid, SubscriptionTask>,
    events: mpsc::Sender<CoreEvent>,
}

struct SubscriptionTask {
    stop: watch::Sender<bool>,
    handle: JoinHandle<()>,
}

impl SubscriptionSupervisor {
    pub fn new(events: mpsc::Sender<CoreEvent>) -> Result<Self, reqwest::Error> {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(20))
            .tcp_keepalive(Duration::from_secs(30))
            .user_agent(concat!("ntfy-pusher/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self {
            client,
            tasks: HashMap::new(),
            events,
        })
    }

    pub async fn apply_config(&mut self, config: &AppConfig) {
        self.shutdown().await;
        let servers: HashMap<_, _> = config
            .servers
            .iter()
            .map(|server| (server.id, server.clone()))
            .collect();
        for topic in config.topics.iter().filter(|topic| topic.enabled) {
            if let Some(server) = servers.get(&topic.server_id) {
                self.start(server.clone(), topic.clone(), config.reconnect.clone());
            }
        }
    }

    pub fn start(
        &mut self,
        server: ServerConfig,
        topic: TopicConfig,
        reconnect: ReconnectSettings,
    ) {
        if self.tasks.contains_key(&topic.id) {
            return;
        }
        let (stop, stop_rx) = watch::channel(false);
        let id = topic.id;
        let client = self.client.clone();
        let events = self.events.clone();
        let handle = tokio::spawn(async move {
            subscription_loop(client, server, topic, reconnect, events, stop_rx).await;
        });
        self.tasks.insert(id, SubscriptionTask { stop, handle });
    }

    pub async fn stop(&mut self, topic_id: Uuid) {
        if let Some(task) = self.tasks.remove(&topic_id) {
            let _ = task.stop.send(true);
            let _ = task.handle.await;
        }
    }

    pub async fn shutdown(&mut self) {
        for (_, task) in self.tasks.drain() {
            let _ = task.stop.send(true);
            let _ = task.handle.await;
        }
    }
}

impl Drop for SubscriptionSupervisor {
    fn drop(&mut self) {
        for task in self.tasks.values() {
            let _ = task.stop.send(true);
            task.handle.abort();
        }
    }
}

async fn subscription_loop(
    client: reqwest::Client,
    server: ServerConfig,
    topic: TopicConfig,
    reconnect: ReconnectSettings,
    events: mpsc::Sender<CoreEvent>,
    mut stop: watch::Receiver<bool>,
) {
    let mut attempt = 0_u32;
    let mut dedupe = Deduplicator::new(DEDUPLICATION_WINDOW);
    let mut last_event_id = None;

    loop {
        if *stop.borrow() {
            publish_status(
                &events,
                &topic,
                ConnectionState::Disabled,
                attempt,
                None,
                None,
            )
            .await;
            return;
        }
        attempt = attempt.saturating_add(1);
        publish_status(
            &events,
            &topic,
            ConnectionState::Connecting,
            attempt,
            None,
            None,
        )
        .await;

        let result = match topic.protocol {
            SubscriptionProtocol::HttpStream => {
                listen_http(
                    &client,
                    &server,
                    &topic,
                    &events,
                    &mut dedupe,
                    &mut last_event_id,
                    &mut stop,
                )
                .await
            }
            SubscriptionProtocol::WebSocket => {
                listen_websocket(
                    &server,
                    &topic,
                    &events,
                    &mut dedupe,
                    &mut last_event_id,
                    &mut stop,
                )
                .await
            }
        };

        if *stop.borrow() {
            publish_status(
                &events,
                &topic,
                ConnectionState::Disabled,
                attempt,
                None,
                None,
            )
            .await;
            return;
        }
        if matches!(result, Err(SubscribeError::Authentication)) {
            publish_status(
                &events,
                &topic,
                ConnectionState::AuthenticationFailed,
                attempt,
                Some("authentication rejected".into()),
                None,
            )
            .await;
            return;
        }
        if reconnect.max_attempts != 0 && attempt >= reconnect.max_attempts {
            publish_status(
                &events,
                &topic,
                ConnectionState::Failed,
                attempt,
                result.err().map(|error| error.to_string()),
                None,
            )
            .await;
            return;
        }

        let delay = backoff_delay(&reconnect, attempt);
        publish_status(
            &events,
            &topic,
            ConnectionState::WaitingToRetry,
            attempt,
            result.err().map(|error| error.to_string()),
            None,
        )
        .await;
        tokio::select! {
            _ = tokio::time::sleep(delay) => {}
            changed = stop.changed() => {
                if changed.is_err() || *stop.borrow() { return; }
            }
        }
    }
}

async fn listen_http(
    client: &reqwest::Client,
    server: &ServerConfig,
    topic: &TopicConfig,
    events: &mpsc::Sender<CoreEvent>,
    dedupe: &mut Deduplicator,
    last_event_id: &mut Option<String>,
    stop: &mut watch::Receiver<bool>,
) -> Result<(), SubscribeError> {
    let url = subscription_url(
        &server.base_url,
        &topic.name,
        "json",
        false,
        last_event_id.as_deref(),
    )?;
    let mut request = client.get(url);
    if let Some(credentials) = &server.credentials {
        request = request.basic_auth(
            &credentials.username,
            Some(credentials.password.expose_secret()),
        );
    }
    let response = request.send().await.map_err(network_error)?;
    if matches!(
        response.status(),
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
    ) {
        return Err(SubscribeError::Authentication);
    }
    if !response.status().is_success() {
        return Err(SubscribeError::HttpStatus(response.status()));
    }
    publish_status(events, topic, ConnectionState::Connected, 0, None, None).await;

    let mut decoder = NdjsonDecoder::default();
    let mut stream = response.bytes_stream();
    loop {
        tokio::select! {
            chunk = stream.next() => match chunk {
                Some(Ok(bytes)) => process_bytes(&bytes, server, topic, events, dedupe, last_event_id, &mut decoder).await?,
                Some(Err(error)) => return Err(network_error(error)),
                None => return Err(SubscribeError::Ended),
            },
            changed = stop.changed() => {
                if changed.is_err() || *stop.borrow() { return Ok(()); }
            }
        }
    }
}

async fn listen_websocket(
    server: &ServerConfig,
    topic: &TopicConfig,
    events: &mpsc::Sender<CoreEvent>,
    dedupe: &mut Deduplicator,
    last_event_id: &mut Option<String>,
    stop: &mut watch::Receiver<bool>,
) -> Result<(), SubscribeError> {
    let url = subscription_url(
        &server.base_url,
        &topic.name,
        "ws",
        true,
        last_event_id.as_deref(),
    )?;
    let mut request = url.as_str().into_client_request().map_err(protocol_error)?;
    if let Some(credentials) = &server.credentials {
        let value = basic_authorization(credentials);
        request.headers_mut().insert(
            http::header::AUTHORIZATION,
            HeaderValue::from_str(&value).map_err(protocol_error)?,
        );
    }
    let (mut socket, response) =
        tokio_tungstenite::connect_async(request)
            .await
            .map_err(|error| {
                if let tokio_tungstenite::tungstenite::Error::Http(response) = &error
                    && matches!(
                        response.status(),
                        http::StatusCode::UNAUTHORIZED | http::StatusCode::FORBIDDEN
                    )
                {
                    return SubscribeError::Authentication;
                }
                SubscribeError::Network(error.to_string())
            })?;
    if !response.status().is_success() && response.status() != http::StatusCode::SWITCHING_PROTOCOLS
    {
        return Err(SubscribeError::Protocol(format!(
            "WebSocket handshake returned {}",
            response.status()
        )));
    }
    publish_status(events, topic, ConnectionState::Connected, 0, None, None).await;
    let mut decoder = NdjsonDecoder::default();
    loop {
        tokio::select! {
            message = socket.next() => match message {
                Some(Ok(Message::Text(text))) => process_bytes(text.as_bytes(), server, topic, events, dedupe, last_event_id, &mut decoder).await?,
                Some(Ok(Message::Binary(bytes))) => process_bytes(&bytes, server, topic, events, dedupe, last_event_id, &mut decoder).await?,
                Some(Ok(Message::Ping(_)|Message::Pong(_)|Message::Frame(_))) => {}
                Some(Ok(Message::Close(_))) | None => return Err(SubscribeError::Ended),
                Some(Err(error)) => return Err(SubscribeError::Network(error.to_string())),
            },
            changed = stop.changed() => {
                if changed.is_err() || *stop.borrow() {
                    let _ = socket.close(None).await;
                    return Ok(());
                }
            }
        }
    }
}

async fn process_bytes(
    bytes: &[u8],
    server: &ServerConfig,
    topic: &TopicConfig,
    events: &mpsc::Sender<CoreEvent>,
    dedupe: &mut Deduplicator,
    last_event_id: &mut Option<String>,
    decoder: &mut NdjsonDecoder,
) -> Result<(), SubscribeError> {
    let decoded = decoder.push(bytes).map_err(protocol_error)?;
    for event in decoded {
        if event.event == EventKind::Message && dedupe.insert(&event.id) {
            let time = event.time;
            let event_id = event.id.clone();
            if events
                .send(CoreEvent::Notification {
                    server: server.clone(),
                    topic: topic.clone(),
                    event: Box::new(event),
                })
                .await
                .is_err()
            {
                return Ok(());
            }
            *last_event_id = Some(event_id);
            publish_status(
                events,
                topic,
                ConnectionState::Connected,
                0,
                None,
                Some(time),
            )
            .await;
        }
    }
    Ok(())
}

fn subscription_url(
    base: &Url,
    topic: &str,
    endpoint: &str,
    websocket: bool,
    since: Option<&str>,
) -> Result<Url, SubscribeError> {
    let mut url = base.clone();
    if websocket {
        let scheme = match base.scheme() {
            "https" => "wss",
            "http" => "ws",
            other => {
                return Err(SubscribeError::Protocol(format!(
                    "unsupported URL scheme {other}"
                )));
            }
        };
        url.set_scheme(scheme)
            .map_err(|_| SubscribeError::Protocol("could not set WebSocket scheme".into()))?;
    }
    {
        let mut segments = url
            .path_segments_mut()
            .map_err(|_| SubscribeError::Protocol("server URL cannot be a base URL".into()))?;
        segments.pop_if_empty().push(topic).push(endpoint);
    }
    if let Some(since) = since {
        url.query_pairs_mut().append_pair("since", since);
    }
    Ok(url)
}

fn basic_authorization(credentials: &BasicCredentials) -> String {
    let combined = format!(
        "{}:{}",
        credentials.username,
        credentials.password.expose_secret()
    );
    format!("Basic {}", BASE64.encode(combined))
}

fn network_error(error: impl std::fmt::Display) -> SubscribeError {
    SubscribeError::Network(error.to_string())
}

fn protocol_error(error: impl std::fmt::Display) -> SubscribeError {
    SubscribeError::Protocol(error.to_string())
}

fn backoff_delay(settings: &ReconnectSettings, attempt: u32) -> Duration {
    let exponent = attempt.saturating_sub(1).min(20);
    let base = settings
        .initial_delay_seconds
        .saturating_mul(1_u64 << exponent);
    let capped = base.min(settings.max_delay_seconds).max(1);
    let jitter = rand::rng().random_range(80_u64..=120);
    Duration::from_millis(capped.saturating_mul(1000).saturating_mul(jitter) / 100)
}

async fn publish_status(
    events: &mpsc::Sender<CoreEvent>,
    topic: &TopicConfig,
    state: ConnectionState,
    attempt: u32,
    last_error: Option<String>,
    last_event_unix_seconds: Option<i64>,
) {
    let _ = events
        .send(CoreEvent::Status(TopicStatus {
            topic_id: topic.id,
            state,
            attempt,
            last_error,
            last_event_unix_seconds,
        }))
        .await;
}

pub fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

pub fn event_channel() -> (mpsc::Sender<CoreEvent>, mpsc::Receiver<CoreEvent>) {
    mpsc::channel(EVENT_QUEUE_CAPACITY)
}

#[derive(Clone)]
pub struct SharedSnapshot(pub Arc<tokio::sync::RwLock<Vec<TopicStatus>>>);

impl Default for SharedSnapshot {
    fn default() -> Self {
        Self(Arc::new(tokio::sync::RwLock::new(Vec::new())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_encoded_subscription_urls() {
        let base = Url::parse("https://example.com/ntfy/").unwrap();
        let http = subscription_url(&base, "alerts / 中文", "json", false, None).unwrap();
        assert_eq!(
            http.as_str(),
            "https://example.com/ntfy/alerts%20%2F%20%E4%B8%AD%E6%96%87/json"
        );
        let ws = subscription_url(&base, "alerts", "ws", true, Some("nFS3knfcQ1xe")).unwrap();
        assert_eq!(
            ws.as_str(),
            "wss://example.com/ntfy/alerts/ws?since=nFS3knfcQ1xe"
        );
    }

    #[test]
    fn backoff_is_capped_and_jittered() {
        let settings = ReconnectSettings {
            max_attempts: 0,
            initial_delay_seconds: 3,
            max_delay_seconds: 10,
        };
        for _ in 0..50 {
            let delay = backoff_delay(&settings, 20);
            assert!((Duration::from_secs(8)..=Duration::from_secs(12)).contains(&delay));
        }
    }
}
