use std::io::{Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

use litedroid_core::{LiteDroidError, Result};
use tracing::{debug, error, info, warn};

// ---------------------------------------------------------------------------
// ADB protocol constants
// ---------------------------------------------------------------------------

/// ADB protocol version (1 << 24).
const ADB_VERSION: u32 = 0x0100_0000;
/// Default maximum payload size.
const ADB_MAX_PAYLOAD: u32 = 4096;
const ADB_HEADER_SIZE: usize = 24;

pub const CMD_CNXN: [u8; 4] = *b"CNXN";
pub const CMD_OPEN: [u8; 4] = *b"OPEN";
pub const CMD_OKAY: [u8; 4] = *b"OKAY";
pub const CMD_CLSE: [u8; 4] = *b"CLSE";
pub const CMD_WRTE: [u8; 4] = *b"WRTE";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Compute the ADB checksum (sum of all bytes as u32).
fn checksum(data: &[u8]) -> u32 {
    data.iter().fold(0u32, |acc, &b| acc.wrapping_add(b as u32))
}

/// Write an ADB message to a TCP stream, ignoring I/O errors.
fn send_msg(stream: &mut TcpStream, msg: &AdbMessage) {
    if let Err(e) = stream.write_all(&msg.encode()) {
        warn!(err = %e, "failed to send ADB message");
    }
}

// ---------------------------------------------------------------------------
// AdbMessage
// ---------------------------------------------------------------------------

/// A single ADB protocol message (24-byte header + optional payload).
#[derive(Debug, Clone)]
pub struct AdbMessage {
    pub command: [u8; 4],
    pub arg0: u32,
    pub arg1: u32,
    pub data_length: u32,
    pub data_check: u32,
    pub data: Vec<u8>,
}

impl AdbMessage {
    // -- Constructors --------------------------------------------------------

    /// Build a CNXN (connect) message.
    pub fn connect(identity: &str, max_payload: u32) -> Self {
        let data = identity.as_bytes().to_vec();
        let data_check = checksum(&data);
        Self {
            command: CMD_CNXN,
            arg0: ADB_VERSION,
            arg1: max_payload,
            data_length: data.len() as u32,
            data_check,
            data,
        }
    }

    /// Build an OPEN message.
    pub fn open(local_id: u32, dest: &str) -> Self {
        let data = dest.as_bytes().to_vec();
        let data_check = checksum(&data);
        Self {
            command: CMD_OPEN,
            arg0: local_id,
            arg1: 0,
            data_length: data.len() as u32,
            data_check,
            data,
        }
    }

    /// Build an OKAY message.
    pub fn okay(local_id: u32, remote_id: u32) -> Self {
        Self {
            command: CMD_OKAY,
            arg0: local_id,
            arg1: remote_id,
            data_length: 0,
            data_check: 0,
            data: Vec::new(),
        }
    }

    /// Build a CLSE message.
    pub fn close(local_id: u32, remote_id: u32) -> Self {
        Self {
            command: CMD_CLSE,
            arg0: local_id,
            arg1: remote_id,
            data_length: 0,
            data_check: 0,
            data: Vec::new(),
        }
    }

    /// Build a WRTE (write) message carrying `payload`.
    pub fn write_msg(local_id: u32, remote_id: u32, payload: &[u8]) -> Self {
        let data = payload.to_vec();
        let data_check = checksum(&data);
        Self {
            command: CMD_WRTE,
            arg0: local_id,
            arg1: remote_id,
            data_length: data.len() as u32,
            data_check,
            data,
        }
    }

    // -- Serialisation -------------------------------------------------------

    /// Encode the message into wire format (24-byte header + payload).
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(ADB_HEADER_SIZE + self.data.len());
        out.extend_from_slice(&self.command);
        out.extend_from_slice(&self.arg0.to_le_bytes());
        out.extend_from_slice(&self.arg1.to_le_bytes());
        out.extend_from_slice(&self.data_length.to_le_bytes());
        out.extend_from_slice(&self.data_check.to_le_bytes());
        let magic = u32::from_le_bytes(self.command) ^ 0xFFFF_FFFF;
        out.extend_from_slice(&magic.to_le_bytes());
        out.extend_from_slice(&self.data);
        out
    }

    /// Decode a message from wire format.
    pub fn decode(data: &[u8]) -> Result<Self> {
        if data.len() < ADB_HEADER_SIZE {
            return Err(LiteDroidError::AdbProtocolError("message too short".into()));
        }

        let command: [u8; 4] = data[0..4]
            .try_into()
            .map_err(|_| LiteDroidError::AdbProtocolError("command parse error".into()))?;
        let arg0 = u32::from_le_bytes(data[4..8].try_into().unwrap());
        let arg1 = u32::from_le_bytes(data[8..12].try_into().unwrap());
        let data_length = u32::from_le_bytes(data[12..16].try_into().unwrap());
        let data_check = u32::from_le_bytes(data[16..20].try_into().unwrap());
        let magic = u32::from_le_bytes(data[20..24].try_into().unwrap());

        let expected_magic = u32::from_le_bytes(command) ^ 0xFFFF_FFFF;
        if magic != expected_magic {
            return Err(LiteDroidError::AdbProtocolError("invalid magic".into()));
        }

        let total = ADB_HEADER_SIZE + data_length as usize;
        if data.len() < total {
            return Err(LiteDroidError::AdbProtocolError(
                "incomplete payload".into(),
            ));
        }

        let payload = data[ADB_HEADER_SIZE..total].to_vec();
        let computed = checksum(&payload);
        if computed != data_check {
            return Err(LiteDroidError::AdbProtocolError("checksum mismatch".into()));
        }

        Ok(Self {
            command,
            arg0,
            arg1,
            data_length,
            data_check,
            data: payload,
        })
    }
}

// ---------------------------------------------------------------------------
// AdbConnection
// ---------------------------------------------------------------------------

/// Stateless handler for a single ADB client TCP connection.
pub struct AdbConnection;

impl AdbConnection {
    /// Handle an incoming ADB connection (CNXN → message loop).
    pub fn handle_client(mut stream: TcpStream) -> Result<()> {
        // 1. Send the initial CNXN handshake.
        let cnxn = AdbMessage::connect("device::litedroid", ADB_MAX_PAYLOAD);
        stream.write_all(&cnxn.encode())?;

        let mut next_local_id: u32 = 1;
        let mut hdr_buf = [0u8; ADB_HEADER_SIZE];

        loop {
            // 2. Read 24-byte header.
            if stream.read_exact(&mut hdr_buf).is_err() {
                break;
            }

            // 3. Determine payload length and read it.
            let data_len = u32::from_le_bytes(hdr_buf[12..16].try_into().unwrap()) as usize;
            let mut full = hdr_buf.to_vec();
            if data_len > 0 {
                let mut payload = vec![0u8; data_len];
                if stream.read_exact(&mut payload).is_err() {
                    break;
                }
                full.extend_from_slice(&payload);
            }

            // 4. Decode.
            let msg = match AdbMessage::decode(&full) {
                Ok(m) => m,
                Err(e) => {
                    error!(err = %e, "ADB message decode failed");
                    break;
                }
            };

            // 5. Dispatch.
            match &msg.command {
                b"OPEN" => {
                    let dest = String::from_utf8_lossy(&msg.data).to_string();
                    let local_id = next_local_id;
                    next_local_id += 1;
                    #[allow(unused)]
                    let remote_id = msg.arg0;

                    if dest == "host:version" {
                        send_msg(&mut stream, &AdbMessage::okay(local_id, remote_id));
                        send_msg(
                            &mut stream,
                            &AdbMessage::write_msg(local_id, remote_id, b"00040024"),
                        );
                    } else if dest == "host:devices" {
                        send_msg(&mut stream, &AdbMessage::okay(local_id, remote_id));
                        send_msg(
                            &mut stream,
                            &AdbMessage::write_msg(local_id, remote_id, b"LITEDROID001\tdevice"),
                        );
                    } else if dest == "host:kill" {
                        send_msg(&mut stream, &AdbMessage::close(local_id, remote_id));
                        break;
                    } else {
                        send_msg(&mut stream, &AdbMessage::okay(local_id, remote_id));
                    }
                }
                b"CNXN" => {
                    send_msg(
                        &mut stream,
                        &AdbMessage::connect("device::litedroid", ADB_MAX_PAYLOAD),
                    );
                }
                b"OKAY" | b"WRTE" => {
                    #[allow(unused)]
                    let _arg0 = msg.arg0;
                    #[allow(unused)]
                    let _arg1 = msg.arg1;
                    debug!(
                        cmd = %String::from_utf8_lossy(&msg.command),
                        len = msg.data.len(),
                        "ADB message"
                    );
                }
                b"CLSE" => {
                    break;
                }
                #[allow(unused)]
                _ => {
                    debug!("unknown ADB command");
                }
            }
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// AdbServer
// ---------------------------------------------------------------------------

/// TCP server that accepts ADB client connections and spawns handlers.
pub struct AdbServer {
    port: u16,
    running: Arc<AtomicBool>,
    listener: Option<TcpListener>,
}

impl AdbServer {
    /// Bind to `port` and prepare to accept connections.
    pub fn new(port: u16) -> Result<Self> {
        let listener = TcpListener::bind(("0.0.0.0", port))
            .map_err(|e| LiteDroidError::AdbConnectionFailed(format!("bind port {port}: {e}")));
        info!(port, "ADB server bound");
        Ok(Self {
            port,
            running: Arc::new(AtomicBool::new(false)),
            listener: Some(listener?),
        })
    }

    /// Spawn a background thread that accepts connections in a loop.
    pub fn start(&mut self) -> Result<JoinHandle<()>> {
        let listener = self
            .listener
            .take()
            .ok_or_else(|| LiteDroidError::AdbConnectionFailed("already started".into()))?;
        self.running.store(true, Ordering::SeqCst);
        let running = self.running.clone();

        Ok(std::thread::spawn(move || loop {
            if !running.load(Ordering::SeqCst) {
                break;
            }
            match listener.accept() {
                Ok((stream, addr)) => {
                    info!(%addr, "ADB client connected");
                    if let Err(e) = AdbConnection::handle_client(stream) {
                        error!(%addr, err = %e, "ADB connection error");
                    }
                }
                Err(e) => {
                    if !running.load(Ordering::SeqCst) {
                        break;
                    }
                    warn!(err = %e, "ADB accept error");
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            }
        }))
    }

    /// Signal the accept loop to stop and unblock the listener.
    pub fn stop(&self) -> Result<()> {
        self.running.store(false, Ordering::SeqCst);
        // Connect to ourselves to unblock accept().
        let _ = TcpStream::connect(("127.0.0.1", self.port));
        Ok(())
    }

    /// Whether the server has been started and not yet stopped.
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}
