//! Client connection handling.
//!
//! This module manages connections from client applications that connect
//! to the multiplexer socket. It handles authentication and message forwarding.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::sync::mpsc;
use tracing::{debug, error, info, trace};
use zbus::message::Message;
use zbus::zvariant::serialized;

use crate::auth::{AuthResult, SaslAuth};
use crate::error::{ClientError, Result};
use crate::message::{ErrorBuilder, MessageExt, MethodReturnBuilder};
use crate::routing::Route;
use crate::serial_map::ClientId;

/// Counter for generating unique client IDs.
static CLIENT_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Generate a new unique client ID.
fn next_client_id() -> ClientId {
    CLIENT_ID_COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// A connected client.
#[derive(Debug)]
pub struct ClientConnection {
    /// Unique identifier for this client.
    id: ClientId,
    /// The client's unique name on our virtual bus (e.g., ":mux.1").
    unique_name: String,
    /// Names this client has requested ownership of.
    owned_names: HashSet<String>,
    /// Channel to send messages to this client.
    tx: mpsc::Sender<Arc<Message>>,
    /// The authenticated UID.
    uid: u32,
    /// The client's PID (process ID).
    pid: Option<u32>,
    /// The client's GID (group ID).
    gid: u32,
}

impl ClientConnection {
    /// Get the client's unique ID.
    pub fn id(&self) -> ClientId {
        self.id
    }

    /// Get the client's unique bus name.
    pub fn unique_name(&self) -> &str {
        &self.unique_name
    }

    /// Get the authenticated UID.
    pub fn uid(&self) -> u32 {
        self.uid
    }

    /// Get the client's PID.
    pub fn pid(&self) -> Option<u32> {
        self.pid
    }

    /// Get the client's GID.
    pub fn gid(&self) -> u32 {
        self.gid
    }

    /// Get the names owned by this client.
    pub fn owned_names(&self) -> &HashSet<String> {
        &self.owned_names
    }

    /// Add a name to this client's owned names.
    pub fn add_owned_name(&mut self, name: String) {
        self.owned_names.insert(name);
    }

    /// Remove a name from this client's owned names.
    pub fn remove_owned_name(&mut self, name: &str) -> bool {
        self.owned_names.remove(name)
    }

    /// Send a message to this client.
    pub async fn send(&self, msg: Arc<Message>) -> Result<()> {
        self.tx.send(msg).await
            .map_err(|_| ClientError::Disconnected)?;
        Ok(())
    }

    /// Check if the send channel is closed.
    pub fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }
}

/// Action to take after processing a client message.
#[derive(Debug)]
pub enum MessageAction {
    /// Forward the message to a bus.
    Forward {
        route: Route,
        message: Arc<Message>,
    },
    /// Send a reply directly to the client.
    Reply(Arc<Message>),
    /// No action needed (message handled internally).
    None,
}

/// Handler for a single client connection.
pub struct ClientHandler {
    /// The client connection info.
    client: ClientConnection,
    /// The raw Unix stream.
    stream: UnixStream,
    /// Buffer for reading messages.
    read_buf: Vec<u8>,
    /// Data buffered during authentication that needs to be processed first.
    pending_data: Vec<u8>,
    /// Current position in pending_data.
    pending_pos: usize,
}

impl ClientHandler {
    /// Accept a new client connection.
    ///
    /// Performs SASL authentication and returns the handler.
    pub async fn accept(mut stream: UnixStream, auth: &SaslAuth) -> Result<Self> {
        let id = next_client_id();
        info!(client_id = id, "New client connection");

        // Get peer credentials from the socket before authentication
        // This gives us PID and GID which aren't available through SASL EXTERNAL
        let (pid, gid) = match stream.peer_cred() {
            Ok(cred) => (
                cred.pid().map(|p| p as u32),
                cred.gid(),
            ),
            Err(e) => {
                debug!(client_id = id, error = %e, "Could not get peer credentials");
                (None, 0)
            }
        };

        // Perform SASL authentication
        let AuthResult { uid, buffered_data } = auth.authenticate_server(&mut stream).await?;
        debug!(client_id = id, uid = uid, pid = ?pid, gid = gid, buffered_bytes = buffered_data.len(), "Client authenticated");

        // Generate unique name
        let unique_name = format!(":mux.{}", id);

        // Create message channel
        let (tx, _rx) = mpsc::channel(256);

        let client = ClientConnection {
            id,
            unique_name,
            owned_names: HashSet::new(),
            tx,
            uid,
            pid,
            gid,
        };

        Ok(Self {
            client,
            stream,
            read_buf: vec![0u8; 16384],
            pending_data: buffered_data,
            pending_pos: 0,
        })
    }

    /// Read exactly `len` bytes into read_buf starting at `offset`, 
    /// first from pending data, then from stream.
    async fn read_exact_into_buf(&mut self, offset: usize, len: usize) -> std::io::Result<()> {
        let mut filled = 0;
        
        // First, consume any pending data from auth buffering
        if self.pending_pos < self.pending_data.len() {
            let pending_remaining = &self.pending_data[self.pending_pos..];
            let to_copy = std::cmp::min(pending_remaining.len(), len);
            self.read_buf[offset..offset + to_copy].copy_from_slice(&pending_remaining[..to_copy]);
            self.pending_pos += to_copy;
            filled = to_copy;
            
            // If we've consumed all pending data, clear it to free memory
            if self.pending_pos >= self.pending_data.len() {
                self.pending_data.clear();
                self.pending_pos = 0;
            }
        }
        
        // Read the rest from the stream if needed
        if filled < len {
            self.stream.read_exact(&mut self.read_buf[offset + filled..offset + len]).await?;
        }
        
        Ok(())
    }

    /// Read exactly `buf.len()` bytes, first from pending data, then from stream.
    async fn read_exact(&mut self, buf: &mut [u8]) -> std::io::Result<()> {
        let mut filled = 0;
        
        // First, consume any pending data from auth buffering
        if self.pending_pos < self.pending_data.len() {
            let pending_remaining = &self.pending_data[self.pending_pos..];
            let to_copy = std::cmp::min(pending_remaining.len(), buf.len());
            buf[..to_copy].copy_from_slice(&pending_remaining[..to_copy]);
            self.pending_pos += to_copy;
            filled = to_copy;
            
            // If we've consumed all pending data, clear it to free memory
            if self.pending_pos >= self.pending_data.len() {
                self.pending_data.clear();
                self.pending_pos = 0;
            }
        }
        
        // Read the rest from the stream if needed
        if filled < buf.len() {
            self.stream.read_exact(&mut buf[filled..]).await?;
        }
        
        Ok(())
    }

    /// Get a reference to the client connection.
    pub fn client(&self) -> &ClientConnection {
        &self.client
    }

    /// Get a mutable reference to the client connection.
    pub fn client_mut(&mut self) -> &mut ClientConnection {
        &mut self.client
    }

    /// Take the client connection info, consuming the handler.
    pub fn into_client(self) -> ClientConnection {
        self.client
    }

    /// Get the client ID.
    pub fn id(&self) -> ClientId {
        self.client.id
    }

    /// Get the client's unique name.
    pub fn unique_name(&self) -> &str {
        &self.client.unique_name
    }

    /// Get the client's UID.
    pub fn uid(&self) -> u32 {
        self.client.uid
    }

    /// Get the client's PID.
    pub fn pid(&self) -> Option<u32> {
        self.client.pid
    }

    /// Get the client's GID.
    pub fn gid(&self) -> u32 {
        self.client.gid
    }

    /// Create the Hello reply message for this client.
    pub fn create_hello_reply(&self, original_msg: &Message) -> Result<Message> {
        MethodReturnBuilder::new(original_msg)
            .destination(self.client.unique_name.as_str())
            .build(&self.client.unique_name)
    }

    /// Create the RequestName reply.
    pub fn create_request_name_reply(&self, original_msg: &Message, result: u32) -> Result<Message> {
        MethodReturnBuilder::new(original_msg)
            .destination(self.client.unique_name.as_str())
            .build(&result)
    }

    /// Create an error reply.
    pub fn create_error_reply(
        &self,
        original_msg: &Message,
        error_name: &str,
        message: &str,
    ) -> Result<Message> {
        ErrorBuilder::new(original_msg, error_name)
            .destination(self.client.unique_name.as_str())
            .build(message)
    }

    /// Send the Hello reply to the client.
    pub async fn send_hello_reply(&mut self, original_msg: &Message) -> Result<()> {
        let reply = self.create_hello_reply(original_msg)?;
        self.send_message(&reply).await
    }

    /// Send a message to the client.
    pub async fn send_message(&mut self, msg: &Message) -> Result<()> {
        let data = msg.data();
        self.stream.write_all(data.bytes()).await?;
        trace!(
            client_id = self.client.id,
            serial = msg.serial(),
            "Sent message to client"
        );
        Ok(())
    }

    /// Read a message from the client.
    ///
    /// Returns None if the client disconnected.
    pub async fn read_message(&mut self) -> Result<Option<Message>> {
        // D-Bus messages start with a fixed 16-byte header
        // We need to read the header first to know the full message size
        
        let mut header = [0u8; 16];
        if let Err(e) = self.read_exact(&mut header).await {
            if e.kind() == std::io::ErrorKind::UnexpectedEof {
                debug!(client_id = self.client.id, "Client disconnected");
                return Ok(None);
            }
            return Err(e.into());
        }

        // Parse header to get message length
        // Byte 0: endianness ('l' = little, 'B' = big)
        // Bytes 4-7: body length (u32)
        // Bytes 12-15: array of header fields length (u32)
        let little_endian = header[0] == b'l';
        
        let body_len = if little_endian {
            u32::from_le_bytes([header[4], header[5], header[6], header[7]])
        } else {
            u32::from_be_bytes([header[4], header[5], header[6], header[7]])
        };
        
        let fields_len = if little_endian {
            u32::from_le_bytes([header[12], header[13], header[14], header[15]])
        } else {
            u32::from_be_bytes([header[12], header[13], header[14], header[15]])
        };

        // Header fields are aligned to 8 bytes
        let fields_len_padded = (fields_len + 7) & !7;
        
        // Total message size
        let total_size = 16 + fields_len_padded + body_len;
        
        // Read the rest of the message
        if self.read_buf.len() < total_size as usize {
            self.read_buf.resize(total_size as usize, 0);
        }
        
        self.read_buf[..16].copy_from_slice(&header);
        
        let remaining = (total_size - 16) as usize;
        if remaining > 0 {
            self.read_exact_into_buf(16, remaining).await?;
        }

        // Parse the message
        // Determine endianness from the first byte
        let endian = if header[0] == b'l' {
            zbus::zvariant::Endian::Little
        } else {
            zbus::zvariant::Endian::Big
        };
        
        // Create a copy of the data with owned bytes
        let bytes: Vec<u8> = self.read_buf[..total_size as usize].to_vec();
        let context = serialized::Context::new_dbus(endian, 0);
        let data = serialized::Data::new(bytes, context);
        
        // SAFETY: The data is valid D-Bus message bytes that we just read
        let msg = unsafe { Message::from_bytes(data)? };
        
        trace!(
            client_id = self.client.id,
            serial = msg.serial(),
            msg_type = ?msg.primary_header().msg_type(),
            destination = ?msg.destination_str(),
            interface = ?msg.interface_str(),
            member = ?msg.member_str(),
            "Received message from client"
        );
        
        Ok(Some(msg))
    }

    /// Run the client handler event loop.
    ///
    /// Returns messages from the client and sends messages to the client.
    pub async fn run(
        mut self,
        mut outbound_rx: mpsc::Receiver<Arc<Message>>,
        inbound_tx: mpsc::Sender<(ClientId, Message)>,
    ) -> Result<()> {
        loop {
            tokio::select! {
                // Receive message from client
                result = self.read_message() => {
                    match result {
                        Ok(Some(msg)) => {
                            if inbound_tx.send((self.client.id, msg)).await.is_err() {
                                debug!(client_id = self.client.id, "Inbound channel closed");
                                break;
                            }
                        }
                        Ok(None) => {
                            // Client disconnected
                            break;
                        }
                        Err(e) => {
                            error!(client_id = self.client.id, error = %e, "Error reading from client");
                            break;
                        }
                    }
                }
                
                // Send message to client
                Some(msg) = outbound_rx.recv() => {
                    if let Err(e) = self.send_message(&msg).await {
                        error!(client_id = self.client.id, error = %e, "Error writing to client");
                        break;
                    }
                }
            }
        }

        info!(client_id = self.client.id, "Client handler exiting");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_id_generation() {
        let id1 = next_client_id();
        let id2 = next_client_id();
        assert_ne!(id1, id2);
    }
}
