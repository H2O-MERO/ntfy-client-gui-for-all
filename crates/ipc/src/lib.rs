//! Authenticated local IPC shared by the daemon and settings GUI.

use ntfy_client_config::AppConfig;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
#[cfg(unix)]
use std::path::PathBuf;
use std::{io, path::Path, pin::Pin};
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use uuid::Uuid;

const MAX_IPC_FRAME_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestEnvelope {
    pub request_id: Uuid,
    pub token: String,
    pub request: Request,
}

impl RequestEnvelope {
    pub fn new(token: impl Into<String>, request: Request) -> Self {
        Self {
            request_id: Uuid::new_v4(),
            token: token.into(),
            request,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum Request {
    Ping,
    GetSnapshot,
    WatchEvents,
    ReplaceConfig(AppConfig),
    ConnectTopic { topic_id: Uuid },
    DisconnectTopic { topic_id: Uuid },
    ReconnectTopic { topic_id: Uuid },
    OpenGui,
    CheckForUpdates,
    Shutdown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseEnvelope {
    pub request_id: Uuid,
    pub result: Result<Response, IpcFault>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum Response {
    Pong,
    Accepted,
    Snapshot(DaemonSnapshot),
    UpdateStatus(UpdateStatus),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateStatus {
    pub current_version: String,
    pub latest_version: Option<String>,
    pub available: bool,
    pub release_url: Option<String>,
    pub release_notes: Option<String>,
    pub verified_asset_available: bool,
    pub automatic_install_supported: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcFault {
    pub code: String,
    pub message: String,
}

impl IpcFault {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }

    pub fn unauthorized() -> Self {
        Self::new("unauthorized", "IPC authentication failed")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionState {
    Disabled,
    Connecting,
    Connected,
    WaitingToRetry,
    AuthenticationFailed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopicStatus {
    pub topic_id: Uuid,
    pub state: ConnectionState,
    pub attempt: u32,
    pub last_error: Option<String>,
    pub last_event_unix_seconds: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonSnapshot {
    pub config: AppConfig,
    pub topics: Vec<TopicStatus>,
    pub started_unix_seconds: i64,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum Event {
    SnapshotChanged(DaemonSnapshot),
    ActivateWindow,
    UpdateAvailable { version: String, url: String },
}

#[derive(Debug, Error)]
pub enum TransportError {
    #[error("IPC I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("IPC JSON failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("IPC frame exceeds {MAX_IPC_FRAME_BYTES} bytes")]
    FrameTooLarge,
    #[error("IPC peer closed the connection")]
    Closed,
}

pub trait AsyncStream: AsyncRead + AsyncWrite + Send + Unpin {}
impl<T> AsyncStream for T where T: AsyncRead + AsyncWrite + Send + Unpin {}
pub type BoxedStream = Pin<Box<dyn AsyncStream>>;

pub async fn write_frame<T: Serialize + ?Sized>(
    stream: &mut BoxedStream,
    value: &T,
) -> Result<(), TransportError> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() > MAX_IPC_FRAME_BYTES {
        return Err(TransportError::FrameTooLarge);
    }
    stream.write_all(&bytes).await?;
    stream.write_all(b"\n").await?;
    stream.flush().await?;
    Ok(())
}

pub async fn read_frame<T: DeserializeOwned>(
    stream: &mut BoxedStream,
) -> Result<T, TransportError> {
    let reader = BufReader::new(stream);
    let mut bytes = Vec::new();
    let read = reader
        .take((MAX_IPC_FRAME_BYTES + 1) as u64)
        .read_until(b'\n', &mut bytes)
        .await?;
    if read == 0 {
        return Err(TransportError::Closed);
    }
    if bytes.len() > MAX_IPC_FRAME_BYTES {
        return Err(TransportError::FrameTooLarge);
    }
    Ok(serde_json::from_slice(&bytes)?)
}

#[derive(Debug, Clone)]
pub struct Endpoint {
    #[cfg(windows)]
    pipe_name: String,
    #[cfg(unix)]
    socket_path: PathBuf,
}

impl Endpoint {
    pub fn for_instance(config_root: &Path, instance: &str) -> Self {
        let safe: String = instance
            .chars()
            .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
            .take(64)
            .collect();
        #[cfg(windows)]
        {
            let _ = config_root;
            Self {
                pipe_name: format!(r"\\.\pipe\ntfy-client-gui-for-all-{safe}"),
            }
        }
        #[cfg(unix)]
        {
            Self {
                socket_path: config_root.join(format!("daemon-{safe}.sock")),
            }
        }
    }

    #[cfg(windows)]
    pub fn display_name(&self) -> &str {
        &self.pipe_name
    }

    #[cfg(unix)]
    pub fn display_name(&self) -> &str {
        self.socket_path
            .to_str()
            .unwrap_or("<non-UTF-8 Unix socket>")
    }
}

pub struct LocalServer {
    endpoint: Endpoint,
    #[cfg(windows)]
    pending: Option<tokio::net::windows::named_pipe::NamedPipeServer>,
    #[cfg(unix)]
    listener: tokio::net::UnixListener,
}

impl LocalServer {
    pub async fn bind(endpoint: Endpoint) -> Result<Self, TransportError> {
        #[cfg(windows)]
        {
            use tokio::net::windows::named_pipe::ServerOptions;
            let mut options = ServerOptions::new();
            options
                .first_pipe_instance(true)
                .reject_remote_clients(true);
            let pending = create_current_user_pipe(&options, &endpoint.pipe_name)?;
            Ok(Self {
                endpoint,
                pending: Some(pending),
            })
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Some(parent) = endpoint.socket_path.parent() {
                tokio::fs::create_dir_all(parent).await?;
                tokio::fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).await?;
            }
            match tokio::fs::remove_file(&endpoint.socket_path).await {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            let listener = tokio::net::UnixListener::bind(&endpoint.socket_path)?;
            tokio::fs::set_permissions(&endpoint.socket_path, fs::Permissions::from_mode(0o600))
                .await?;
            Ok(Self { endpoint, listener })
        }
    }

    pub async fn accept(&mut self) -> Result<BoxedStream, TransportError> {
        #[cfg(windows)]
        {
            use tokio::net::windows::named_pipe::ServerOptions;
            let server = self
                .pending
                .take()
                .expect("named pipe accept called concurrently");
            server.connect().await?;
            let mut options = ServerOptions::new();
            options.reject_remote_clients(true);
            self.pending = Some(create_current_user_pipe(
                &options,
                &self.endpoint.pipe_name,
            )?);
            Ok(Box::pin(server))
        }
        #[cfg(unix)]
        {
            let (stream, _) = self.listener.accept().await?;
            Ok(Box::pin(stream))
        }
    }
}

#[cfg(windows)]
fn create_current_user_pipe(
    options: &tokio::net::windows::named_pipe::ServerOptions,
    name: &str,
) -> io::Result<tokio::net::windows::named_pipe::NamedPipeServer> {
    use std::{ffi::c_void, mem::size_of, os::windows::ffi::OsStrExt};
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::{
            Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW,
            PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES,
        },
    };

    // Protected DACL: LocalSystem and the object owner (the current user) get full access.
    // Remote clients are independently rejected by ServerOptions.
    let sddl: Vec<u16> = std::ffi::OsStr::new("D:P(A;;GA;;;SY)(A;;GA;;;OW)")
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    // SAFETY: `sddl` is NUL-terminated and `descriptor` is a valid out pointer.
    let converted = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            1,
            &mut descriptor,
            std::ptr::null_mut(),
        )
    };
    if converted == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.cast::<c_void>(),
        bInheritHandle: 0,
    };
    // SAFETY: `attributes` and its LocalAlloc-backed descriptor remain alive for the call.
    let result = unsafe {
        options.create_with_security_attributes_raw(
            name,
            (&mut attributes as *mut SECURITY_ATTRIBUTES).cast::<c_void>(),
        )
    };
    // SAFETY: the conversion API allocated this descriptor with LocalAlloc.
    unsafe {
        LocalFree(descriptor.cast::<c_void>());
    }
    result
}

#[cfg(unix)]
impl Drop for LocalServer {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.endpoint.socket_path);
    }
}

pub async fn connect(endpoint: &Endpoint) -> Result<BoxedStream, TransportError> {
    #[cfg(windows)]
    {
        use tokio::net::windows::named_pipe::ClientOptions;
        Ok(Box::pin(ClientOptions::new().open(&endpoint.pipe_name)?))
    }
    #[cfg(unix)]
    {
        Ok(Box::pin(
            tokio::net::UnixStream::connect(&endpoint.socket_path).await?,
        ))
    }
}

pub async fn request(
    endpoint: &Endpoint,
    token: &str,
    request: Request,
) -> Result<ResponseEnvelope, TransportError> {
    let mut stream = connect(endpoint).await?;
    let envelope = RequestEnvelope::new(token, request);
    write_frame(&mut stream, &envelope).await?;
    read_frame(&mut stream).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn frame_round_trip_and_limit() {
        let (left, right) = tokio::io::duplex(4096);
        let mut left: BoxedStream = Box::pin(left);
        let mut right: BoxedStream = Box::pin(right);
        let sent = RequestEnvelope::new("secret", Request::Ping);
        let id = sent.request_id;
        let writer = tokio::spawn(async move { write_frame(&mut left, &sent).await });
        let read: RequestEnvelope = read_frame(&mut right).await.unwrap();
        writer.await.unwrap().unwrap();
        assert_eq!(read.request_id, id);
        assert!(matches!(read.request, Request::Ping));
    }

    #[tokio::test]
    async fn platform_endpoint_accepts_a_real_round_trip() {
        let root = tempfile::tempdir().unwrap();
        let instance = format!("ipc-test-{}", Uuid::new_v4());
        let endpoint = Endpoint::for_instance(root.path(), &instance);
        let mut server = LocalServer::bind(endpoint.clone()).await.unwrap();

        let server_task = tokio::spawn(async move {
            let mut stream = server.accept().await.unwrap();
            let request: RequestEnvelope = read_frame(&mut stream).await.unwrap();
            assert_eq!(request.token, "test-token");
            assert!(matches!(request.request, Request::Ping));
            write_frame(
                &mut stream,
                &ResponseEnvelope {
                    request_id: request.request_id,
                    result: Ok(Response::Pong),
                },
            )
            .await
            .unwrap();
        });

        let response = request(&endpoint, "test-token", Request::Ping)
            .await
            .unwrap();
        assert!(matches!(response.result, Ok(Response::Pong)));
        server_task.await.unwrap();
    }
}
