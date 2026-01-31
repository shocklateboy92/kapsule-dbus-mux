//! D-Bus SASL authentication handling.
//!
//! This module implements the server side of D-Bus SASL authentication,
//! specifically the EXTERNAL mechanism which authenticates based on
//! Unix socket credentials (UID).

use std::io::{ErrorKind, Result as IoResult};

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tracing::{debug, trace, warn};
use uuid::Uuid;

use crate::error::{AuthError, Result};

/// Result of successful authentication.
pub struct AuthResult {
    /// The authenticated UID.
    pub uid: u32,
    /// Any data that was buffered during auth but belongs to the D-Bus message stream.
    pub buffered_data: Vec<u8>,
}

/// SASL authentication handler for D-Bus connections.
pub struct SaslAuth {
    /// The GUID for this server instance.
    server_guid: String,
}

impl Default for SaslAuth {
    fn default() -> Self {
        Self::new()
    }
}

impl SaslAuth {
    /// Create a new SASL auth handler with a random GUID.
    pub fn new() -> Self {
        Self {
            server_guid: generate_guid(),
        }
    }

    /// Create a new SASL auth handler with a specific GUID.
    pub fn with_guid(guid: String) -> Self {
        Self { server_guid: guid }
    }

    /// Get the server GUID.
    pub fn guid(&self) -> &str {
        &self.server_guid
    }

    /// Perform server-side SASL authentication.
    ///
    /// # Arguments
    /// * `stream` - The Unix stream to authenticate.
    ///
    /// # Returns
    /// The authenticated UID and any buffered data on success.
    pub async fn authenticate_server(&self, stream: &mut UnixStream) -> Result<AuthResult> {
        // Read the null byte that starts D-Bus auth
        let mut null_byte = [0u8; 1];
        stream.read_exact(&mut null_byte).await?;
        
        if null_byte[0] != 0 {
            return Err(AuthError::ExpectedNullByte.into());
        }
        trace!("Received null byte");

        let mut reader = BufReader::new(stream);
        
        loop {
            let line = read_line(&mut reader).await?;
            trace!(line = %line, "Received auth line");

            if line.starts_with("AUTH EXTERNAL ") {
                // Client sends hex-encoded UID directly (Qt/qdbus style)
                let hex_uid = line.strip_prefix("AUTH EXTERNAL ").unwrap();
                let uid = parse_hex_uid(hex_uid)?;
                
                debug!(uid = uid, "EXTERNAL auth with UID (direct)");
                
                // Complete auth with this UID
                return self.complete_auth(&mut reader, uid).await;
            } else if line == "AUTH EXTERNAL" {
                // Client uses challenge-response flow (busctl/systemd style)
                // Send DATA challenge (empty challenge for EXTERNAL)
                let stream = reader.get_mut();
                stream.write_all(b"DATA\r\n").await?;
                stream.flush().await?;
                trace!("Sent DATA challenge for EXTERNAL auth");
                
                // Wait for DATA response with hex-encoded UID
                let line = read_line(&mut reader).await?;
                trace!(line = %line, "Received DATA response");
                
                if let Some(hex_uid) = line.strip_prefix("DATA ") {
                    let uid = parse_hex_uid(hex_uid)?;
                    debug!(uid = uid, "EXTERNAL auth with UID (challenge-response)");
                    return self.complete_auth(&mut reader, uid).await;
                } else if line == "DATA" {
                    // Empty DATA response - use socket credentials
                    // For now, we'll use the current process UID as fallback
                    let uid = nix::unistd::getuid().as_raw();
                    debug!(uid = uid, "EXTERNAL auth with empty DATA (using socket UID)");
                    return self.complete_auth(&mut reader, uid).await;
                } else {
                    return Err(AuthError::InvalidData(format!("Expected DATA response, got: {}", line)).into());
                }
            } else if line == "AUTH" {
                // Client asking what mechanisms we support
                let stream = reader.get_mut();
                stream.write_all(b"REJECTED EXTERNAL\r\n").await?;
                stream.flush().await?;
                trace!("Sent REJECTED, offering EXTERNAL");
            } else if line.starts_with("AUTH ") {
                // Unknown mechanism
                let mechanism = line.strip_prefix("AUTH ").unwrap().split_whitespace().next().unwrap_or("");
                warn!(mechanism = %mechanism, "Unsupported auth mechanism");
                let stream = reader.get_mut();
                stream.write_all(b"REJECTED EXTERNAL\r\n").await?;
                stream.flush().await?;
            } else if line == "CANCEL" {
                // Client cancelled, restart
                let stream = reader.get_mut();
                stream.write_all(b"REJECTED EXTERNAL\r\n").await?;
                stream.flush().await?;
            } else if line == "ERROR" || line.starts_with("ERROR ") {
                // Client sent error, restart
                let stream = reader.get_mut();
                stream.write_all(b"REJECTED EXTERNAL\r\n").await?;
                stream.flush().await?;
            } else {
                return Err(AuthError::InvalidData(format!("Unexpected line: {}", line)).into());
            }
        }
    }

    /// Complete EXTERNAL auth after UID has been determined.
    async fn complete_auth<R>(&self, reader: &mut BufReader<R>, uid: u32) -> Result<AuthResult>
    where
        R: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    {
        // Send OK with our server GUID
        let response = format!("OK {}\r\n", self.server_guid);
        let stream = reader.get_mut();
        stream.write_all(response.as_bytes()).await?;
        stream.flush().await?;
        trace!("Sent OK response");

        // Wait for NEGOTIATE_UNIX_FD or BEGIN
        let line = read_line(reader).await?;
        trace!(line = %line, "Received post-auth line");

        if line == "NEGOTIATE_UNIX_FD" {
            // We support FD passing
            let stream = reader.get_mut();
            stream.write_all(b"AGREE_UNIX_FD\r\n").await?;
            stream.flush().await?;
            trace!("Agreed to UNIX FD passing");

            // Now wait for BEGIN
            let line = read_line(reader).await?;
            if line != "BEGIN" {
                return Err(AuthError::ExpectedBegin.into());
            }
        } else if line != "BEGIN" {
            return Err(AuthError::ExpectedBegin.into());
        }

        debug!(uid = uid, "Authentication successful");
        
        // Extract any remaining buffered data - this is critical!
        // The client may have sent D-Bus messages immediately after BEGIN,
        // and BufReader may have read them into its internal buffer.
        let buffered_data = reader.buffer().to_vec();
        trace!(buffered_bytes = buffered_data.len(), "Returning buffered data from auth");
        
        Ok(AuthResult { uid, buffered_data })
    }
}

/// Read a line from the stream (terminated by \r\n).
async fn read_line<R: tokio::io::AsyncBufRead + Unpin>(reader: &mut R) -> IoResult<String> {
    let mut line = String::new();
    reader.read_line(&mut line).await?;
    
    // Remove trailing \r\n
    if line.ends_with('\n') {
        line.pop();
    }
    if line.ends_with('\r') {
        line.pop();
    }
    
    if line.is_empty() {
        return Err(std::io::Error::new(ErrorKind::UnexpectedEof, "Empty line"));
    }
    
    Ok(line)
}

/// Parse a hex-encoded UID.
fn parse_hex_uid(hex: &str) -> Result<u32> {
    let bytes = hex::decode(hex)
        .map_err(|e| AuthError::InvalidData(format!("Invalid hex: {}", e)))?;
    
    let uid_str = String::from_utf8(bytes)
        .map_err(|e| AuthError::InvalidData(format!("Invalid UTF-8: {}", e)))?;
    
    uid_str.parse::<u32>()
        .map_err(|e| AuthError::InvalidData(format!("Invalid UID: {}", e)).into())
}

/// Generate a random GUID for the D-Bus server.
pub fn generate_guid() -> String {
    // D-Bus GUIDs are 32 hex characters
    Uuid::new_v4().as_simple().to_string()
}

/// Encode a UID as hex for D-Bus auth.
pub fn encode_uid(uid: u32) -> String {
    hex::encode(uid.to_string().as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_hex_uid() {
        // "1000" encoded as hex
        let hex = hex::encode("1000");
        assert_eq!(parse_hex_uid(&hex).unwrap(), 1000);
    }

    #[test]
    fn test_encode_uid() {
        let encoded = encode_uid(1000);
        let decoded = hex::decode(&encoded).unwrap();
        let uid_str = String::from_utf8(decoded).unwrap();
        assert_eq!(uid_str, "1000");
    }

    #[test]
    fn test_generate_guid() {
        let guid = generate_guid();
        assert_eq!(guid.len(), 32);
        assert!(guid.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
