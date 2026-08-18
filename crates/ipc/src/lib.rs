use std::io::{Read as _, Write as _};
use std::os::unix::net::{UnixListener, UnixStream};
use std::time::Duration;

use bytes::BytesMut;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{debug, info, warn};
use uuid::Uuid;

use litedroid_core::{LiteDroidError, Result, IPC_PROTOCOL_VERSION};

// ---------------------------------------------------------------------------
// Wire protocol messages
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcRequest {
    pub version: u32,
    pub request_id: String,
    pub method: String,
    pub params: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcResponse {
    pub version: u32,
    pub request_id: String,
    pub success: bool,
    pub result: Option<Value>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcEvent {
    pub event_type: String,
    pub device_id: String,
    pub data: Value,
}

// ---------------------------------------------------------------------------
// IpcStream – framed reader/writer over a Unix stream
// ---------------------------------------------------------------------------

/// A length-prefixed (4-byte big-endian) JSON stream over a Unix domain socket.
pub struct IpcStream {
    stream: UnixStream,
}

impl IpcStream {
    fn new(stream: UnixStream) -> Self {
        Self { stream }
    }

    pub(crate) fn write_frame(&mut self, data: &[u8]) -> Result<()> {
        let mut buf = BytesMut::with_capacity(4 + data.len());
        buf.extend_from_slice(&(data.len() as u32).to_be_bytes());
        buf.extend_from_slice(data);
        self.stream.write_all(&buf)?;
        self.stream.flush()?;
        Ok(())
    }

    pub(crate) fn read_frame(&mut self) -> Result<Vec<u8>> {
        let mut len_buf = [0u8; 4];
        self.stream.read_exact(&mut len_buf)?;
        let len = u32::from_be_bytes(len_buf) as usize;
        let mut payload = vec![0u8; len];
        if len > 0 {
            self.stream.read_exact(&mut payload)?;
        }
        Ok(payload)
    }

    /// Block until an [`IpcRequest`] arrives.
    pub fn recv_request(&mut self) -> Result<IpcRequest> {
        let payload = self.read_frame()?;
        let request: IpcRequest = serde_json::from_slice(&payload)
            .map_err(|e| LiteDroidError::IpcProtocolError(e.to_string()))?;
        debug!(method = %request.method, id = %request.request_id, "received IPC request");
        Ok(request)
    }

    /// Send an [`IpcResponse`] back to the peer.
    pub fn send_response(&mut self, response: &IpcResponse) -> Result<()> {
        let json = serde_json::to_vec(response)
            .map_err(|e| LiteDroidError::IpcProtocolError(e.to_string()))?;
        self.write_frame(&json)?;
        debug!(id = %response.request_id, success = response.success, "sent IPC response");
        Ok(())
    }

    /// Push an unsolicited [`IpcEvent`] to the peer.
    pub fn send_event(&mut self, event: &IpcEvent) -> Result<()> {
        let json = serde_json::to_vec(event)
            .map_err(|e| LiteDroidError::IpcProtocolError(e.to_string()))?;
        self.write_frame(&json)?;
        debug!(
            event_type = %event.event_type,
            device_id = %event.device_id,
            "sent IPC event"
        );
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// IpcServer
// ---------------------------------------------------------------------------

pub struct IpcServer {
    listener: UnixListener,
}

impl IpcServer {
    /// Bind a new IPC server to `socket_path`.
    pub fn new(socket_path: &str) -> Result<Self> {
        // Remove stale socket if present.
        let _ = std::fs::remove_file(socket_path);
        let listener = UnixListener::bind(socket_path)
            .map_err(|e| LiteDroidError::IpcConnectionFailed(e.to_string()))?;
        info!(path = socket_path, "IPC server listening");
        Ok(Self { listener })
    }

    /// Accept an incoming connection and return the framed stream plus the
    /// peer address as a string.
    pub fn accept(&self) -> Result<(IpcStream, String)> {
        let (stream, addr) = self
            .listener
            .accept()
            .map_err(|e| LiteDroidError::IpcConnectionFailed(e.to_string()))?;
        stream.set_read_timeout(Some(Duration::from_secs(30)))?;
        stream.set_write_timeout(Some(Duration::from_secs(30)))?;
        let addr_str = addr
            .as_pathname()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| format!("{:?}", addr));
        debug!(addr = %addr_str, "IPC client connected");
        Ok((IpcStream::new(stream), addr_str))
    }

    /// Signal that the server should stop.  The listener is actually closed
    /// when this value is dropped.
    pub fn shutdown(&self) -> Result<()> {
        info!("IPC server shutdown requested");
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// IpcClient
// ---------------------------------------------------------------------------

pub struct IpcClient {
    stream: IpcStream,
}

impl IpcClient {
    /// Connect to a running IPC server.
    pub fn connect(socket_path: &str) -> Result<Self> {
        let unix_stream = UnixStream::connect(socket_path)
            .map_err(|e| LiteDroidError::IpcConnectionFailed(e.to_string()))?;
        unix_stream.set_read_timeout(Some(Duration::from_secs(30)))?;
        unix_stream.set_write_timeout(Some(Duration::from_secs(30)))?;
        info!(path = socket_path, "IPC client connected");
        Ok(Self {
            stream: IpcStream::new(unix_stream),
        })
    }

    /// Send a request and block until the response arrives.
    pub fn send_request(&mut self, method: &str, params: Value) -> Result<IpcResponse> {
        let request = IpcRequest {
            version: IPC_PROTOCOL_VERSION,
            request_id: Uuid::new_v4().to_string(),
            method: method.to_string(),
            params,
        };

        let json = serde_json::to_vec(&request)
            .map_err(|e| LiteDroidError::IpcProtocolError(e.to_string()))?;

        self.stream.write_frame(&json)?;

        let payload = self.stream.read_frame()?;
        let response: IpcResponse = serde_json::from_slice(&payload)
            .map_err(|e| LiteDroidError::IpcProtocolError(e.to_string()))?;

        debug!(method = %method, success = response.success, "IPC request completed");
        Ok(response)
    }

    /// Send a `ping` request and return whether the server responded
    /// successfully.
    pub fn ping(&mut self) -> bool {
        match self.send_request("ping", Value::Null) {
            Ok(response) => response.success,
            Err(e) => {
                warn!("ping failed: {}", e);
                false
            }
        }
    }
}
