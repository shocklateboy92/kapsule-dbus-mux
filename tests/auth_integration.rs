//! Integration tests for D-Bus SASL EXTERNAL authentication.
//!
//! These tests verify the complete authentication flow including:
//! - Standard EXTERNAL auth with UID
//! - FD passing negotiation
//! - Error handling and edge cases
//! - Protocol compliance

use std::time::Duration;

use tempfile::tempdir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::time::timeout;

use kapsule_dbus_mux::auth::{encode_uid, generate_guid, SaslAuth};

/// Helper to create a connected pair of Unix streams for testing.
async fn create_stream_pair() -> (UnixStream, UnixStream) {
    let dir = tempdir().expect("Failed to create temp dir");
    let socket_path = dir.path().join("test.sock");
    
    let listener = UnixListener::bind(&socket_path).expect("Failed to bind listener");
    
    let client = UnixStream::connect(&socket_path).await.expect("Failed to connect client");
    let (server, _) = listener.accept().await.expect("Failed to accept connection");
    
    // Keep tempdir alive by leaking it (tests are short-lived anyway)
    std::mem::forget(dir);
    
    (server, client)
}

/// Helper to simulate a D-Bus client performing EXTERNAL auth.
async fn client_auth_external(stream: &mut UnixStream, uid: u32, negotiate_fd: bool) -> String {
    // Send null byte to start authentication
    stream.write_all(&[0u8]).await.expect("Failed to write null byte");
    
    // Send AUTH EXTERNAL with hex-encoded UID
    let hex_uid = encode_uid(uid);
    let auth_line = format!("AUTH EXTERNAL {}\r\n", hex_uid);
    stream.write_all(auth_line.as_bytes()).await.expect("Failed to write AUTH");
    stream.flush().await.expect("Failed to flush");
    
    // Read OK response
    let response = read_response(stream).await;
    assert!(response.starts_with("OK "), "Expected OK response, got: {}", response);
    
    let guid = response.strip_prefix("OK ").unwrap().trim().to_string();
    
    if negotiate_fd {
        // Negotiate FD passing
        stream.write_all(b"NEGOTIATE_UNIX_FD\r\n").await.expect("Failed to write NEGOTIATE_UNIX_FD");
        stream.flush().await.expect("Failed to flush");
        
        let response = read_response(stream).await;
        assert_eq!(response.trim(), "AGREE_UNIX_FD", "Expected AGREE_UNIX_FD, got: {}", response);
    }
    
    // Send BEGIN
    stream.write_all(b"BEGIN\r\n").await.expect("Failed to write BEGIN");
    stream.flush().await.expect("Failed to flush");
    
    guid
}

/// Helper to read a line response from the stream.
async fn read_response(stream: &mut UnixStream) -> String {
    let mut buf = vec![0u8; 256];
    let mut response = String::new();
    
    loop {
        let n = stream.read(&mut buf).await.expect("Failed to read response");
        if n == 0 {
            panic!("Unexpected EOF while reading response");
        }
        response.push_str(&String::from_utf8_lossy(&buf[..n]));
        if response.contains('\n') {
            break;
        }
    }
    
    response.trim_end_matches('\n').trim_end_matches('\r').to_string()
}

/// Helper to simulate busctl-style challenge-response EXTERNAL auth.
async fn client_auth_external_challenge_response(stream: &mut UnixStream, uid: u32, negotiate_fd: bool) -> String {
    // Send null byte to start authentication
    stream.write_all(&[0u8]).await.expect("Failed to write null byte");
    
    // Send AUTH EXTERNAL without UID (busctl style)
    stream.write_all(b"AUTH EXTERNAL\r\n").await.expect("Failed to write AUTH");
    stream.flush().await.expect("Failed to flush");
    
    // Expect DATA challenge
    let response = read_response(stream).await;
    assert_eq!(response, "DATA", "Expected DATA challenge, got: {}", response);
    
    // Send DATA response with hex-encoded UID
    let hex_uid = encode_uid(uid);
    let data_line = format!("DATA {}\r\n", hex_uid);
    stream.write_all(data_line.as_bytes()).await.expect("Failed to write DATA response");
    stream.flush().await.expect("Failed to flush");
    
    // Read OK response
    let response = read_response(stream).await;
    assert!(response.starts_with("OK "), "Expected OK response, got: {}", response);
    
    let guid = response.strip_prefix("OK ").unwrap().trim().to_string();
    
    if negotiate_fd {
        // Negotiate FD passing
        stream.write_all(b"NEGOTIATE_UNIX_FD\r\n").await.expect("Failed to write NEGOTIATE_UNIX_FD");
        stream.flush().await.expect("Failed to flush");
        
        let response = read_response(stream).await;
        assert_eq!(response.trim(), "AGREE_UNIX_FD", "Expected AGREE_UNIX_FD, got: {}", response);
    }
    
    // Send BEGIN
    stream.write_all(b"BEGIN\r\n").await.expect("Failed to write BEGIN");
    stream.flush().await.expect("Failed to flush");
    
    guid
}

// ============================================================================
// Basic Authentication Tests
// ============================================================================

#[tokio::test]
async fn test_external_auth_success() {
    let (mut server_stream, mut client_stream) = create_stream_pair().await;
    
    let auth = SaslAuth::new();
    let expected_guid = auth.guid().to_string();
    
    // Run server auth in a task
    let server_handle = tokio::spawn(async move {
        auth.authenticate_server(&mut server_stream).await
    });
    
    // Run client auth
    let uid = nix::unistd::getuid().as_raw();
    let received_guid = client_auth_external(&mut client_stream, uid, false).await;
    
    // Verify server completed successfully
    let result = server_handle.await.expect("Server task panicked");
    let authenticated_uid = result.expect("Authentication should succeed");
    
    assert_eq!(authenticated_uid, uid);
    assert_eq!(received_guid, expected_guid);
}

#[tokio::test]
async fn test_external_auth_challenge_response() {
    // Test busctl-style challenge-response EXTERNAL auth
    let (mut server_stream, mut client_stream) = create_stream_pair().await;
    
    let auth = SaslAuth::new();
    let expected_guid = auth.guid().to_string();
    
    let server_handle = tokio::spawn(async move {
        auth.authenticate_server(&mut server_stream).await
    });
    
    let uid = nix::unistd::getuid().as_raw();
    let received_guid = client_auth_external_challenge_response(&mut client_stream, uid, false).await;
    
    let result = server_handle.await.expect("Server task panicked");
    let authenticated_uid = result.expect("Challenge-response auth should succeed");
    
    assert_eq!(authenticated_uid, uid);
    assert_eq!(received_guid, expected_guid);
}

#[tokio::test]
async fn test_external_auth_challenge_response_with_fd() {
    // Test busctl-style challenge-response with FD negotiation
    let (mut server_stream, mut client_stream) = create_stream_pair().await;
    
    let auth = SaslAuth::new();
    
    let server_handle = tokio::spawn(async move {
        auth.authenticate_server(&mut server_stream).await
    });
    
    let uid = nix::unistd::getuid().as_raw();
    client_auth_external_challenge_response(&mut client_stream, uid, true).await;
    
    let result = server_handle.await.expect("Server task panicked");
    let authenticated_uid = result.expect("Challenge-response with FD should succeed");
    
    assert_eq!(authenticated_uid, uid);
}

#[tokio::test]
async fn test_external_auth_empty_data_response() {
    // Test empty DATA response (should use socket UID fallback)
    let (mut server_stream, mut client_stream) = create_stream_pair().await;
    
    let auth = SaslAuth::new();
    
    let server_handle = tokio::spawn(async move {
        auth.authenticate_server(&mut server_stream).await
    });
    
    // Send null byte
    client_stream.write_all(&[0u8]).await.unwrap();
    
    // Send AUTH EXTERNAL without UID
    client_stream.write_all(b"AUTH EXTERNAL\r\n").await.unwrap();
    client_stream.flush().await.unwrap();
    
    // Expect DATA challenge
    let response = read_response(&mut client_stream).await;
    assert_eq!(response, "DATA");
    
    // Send empty DATA response
    client_stream.write_all(b"DATA\r\n").await.unwrap();
    client_stream.flush().await.unwrap();
    
    // Should get OK
    let response = read_response(&mut client_stream).await;
    assert!(response.starts_with("OK "), "Expected OK, got: {}", response);
    
    client_stream.write_all(b"BEGIN\r\n").await.unwrap();
    client_stream.flush().await.unwrap();
    
    let result = server_handle.await.expect("Server task panicked");
    let authenticated_uid = result.expect("Empty DATA auth should succeed");
    
    // Should use the current process UID as fallback
    assert_eq!(authenticated_uid, nix::unistd::getuid().as_raw());
}

#[tokio::test]
async fn test_external_auth_with_custom_guid() {
    let (mut server_stream, mut client_stream) = create_stream_pair().await;
    
    let custom_guid = "abcdef0123456789abcdef0123456789".to_string();
    let auth = SaslAuth::with_guid(custom_guid.clone());
    
    let server_handle = tokio::spawn(async move {
        auth.authenticate_server(&mut server_stream).await
    });
    
    let uid = nix::unistd::getuid().as_raw();
    let received_guid = client_auth_external(&mut client_stream, uid, false).await;
    
    let result = server_handle.await.expect("Server task panicked");
    result.expect("Authentication should succeed");
    
    assert_eq!(received_guid, custom_guid);
}

#[tokio::test]
async fn test_external_auth_with_fd_negotiation() {
    let (mut server_stream, mut client_stream) = create_stream_pair().await;
    
    let auth = SaslAuth::new();
    
    let server_handle = tokio::spawn(async move {
        auth.authenticate_server(&mut server_stream).await
    });
    
    let uid = nix::unistd::getuid().as_raw();
    client_auth_external(&mut client_stream, uid, true).await;
    
    let result = server_handle.await.expect("Server task panicked");
    let authenticated_uid = result.expect("Authentication should succeed");
    
    assert_eq!(authenticated_uid, uid);
}

#[tokio::test]
async fn test_external_auth_different_uid() {
    let (mut server_stream, mut client_stream) = create_stream_pair().await;
    
    let auth = SaslAuth::new();
    
    let server_handle = tokio::spawn(async move {
        auth.authenticate_server(&mut server_stream).await
    });
    
    // Use a different UID (0 = root, assuming we're not running as root)
    let claimed_uid = 0u32;
    client_auth_external(&mut client_stream, claimed_uid, false).await;
    
    let result = server_handle.await.expect("Server task panicked");
    let authenticated_uid = result.expect("Authentication should succeed");
    
    // The auth module accepts any claimed UID (credential check would be done elsewhere)
    assert_eq!(authenticated_uid, claimed_uid);
}

// ============================================================================
// Protocol Negotiation Tests
// ============================================================================

#[tokio::test]
async fn test_auth_mechanism_query() {
    let (mut server_stream, mut client_stream) = create_stream_pair().await;
    
    let auth = SaslAuth::new();
    
    // Run server in background
    let server_handle = tokio::spawn(async move {
        auth.authenticate_server(&mut server_stream).await
    });
    
    // Send null byte
    client_stream.write_all(&[0u8]).await.unwrap();
    
    // Query available mechanisms
    client_stream.write_all(b"AUTH\r\n").await.unwrap();
    client_stream.flush().await.unwrap();
    
    let response = read_response(&mut client_stream).await;
    assert!(response.starts_with("REJECTED"), "Expected REJECTED, got: {}", response);
    assert!(response.contains("EXTERNAL"), "Expected EXTERNAL mechanism in: {}", response);
    
    // Now do proper auth
    let uid = nix::unistd::getuid().as_raw();
    let hex_uid = encode_uid(uid);
    let auth_line = format!("AUTH EXTERNAL {}\r\n", hex_uid);
    client_stream.write_all(auth_line.as_bytes()).await.unwrap();
    client_stream.flush().await.unwrap();
    
    let response = read_response(&mut client_stream).await;
    assert!(response.starts_with("OK "), "Expected OK, got: {}", response);
    
    client_stream.write_all(b"BEGIN\r\n").await.unwrap();
    client_stream.flush().await.unwrap();
    
    let result = server_handle.await.expect("Server task panicked");
    result.expect("Authentication should succeed");
}

#[tokio::test]
async fn test_unsupported_mechanism_rejected() {
    let (mut server_stream, mut client_stream) = create_stream_pair().await;
    
    let auth = SaslAuth::new();
    
    let server_handle = tokio::spawn(async move {
        auth.authenticate_server(&mut server_stream).await
    });
    
    // Send null byte
    client_stream.write_all(&[0u8]).await.unwrap();
    
    // Try DBUS_COOKIE_SHA1 mechanism (should be rejected)
    client_stream.write_all(b"AUTH DBUS_COOKIE_SHA1 some_data\r\n").await.unwrap();
    client_stream.flush().await.unwrap();
    
    let response = read_response(&mut client_stream).await;
    assert!(response.starts_with("REJECTED"), "Expected REJECTED, got: {}", response);
    
    // Try ANONYMOUS mechanism (should be rejected)
    client_stream.write_all(b"AUTH ANONYMOUS\r\n").await.unwrap();
    client_stream.flush().await.unwrap();
    
    let response = read_response(&mut client_stream).await;
    assert!(response.starts_with("REJECTED"), "Expected REJECTED, got: {}", response);
    
    // Now use correct mechanism
    let uid = nix::unistd::getuid().as_raw();
    let hex_uid = encode_uid(uid);
    let auth_line = format!("AUTH EXTERNAL {}\r\n", hex_uid);
    client_stream.write_all(auth_line.as_bytes()).await.unwrap();
    client_stream.flush().await.unwrap();
    
    let response = read_response(&mut client_stream).await;
    assert!(response.starts_with("OK "), "Expected OK, got: {}", response);
    
    client_stream.write_all(b"BEGIN\r\n").await.unwrap();
    client_stream.flush().await.unwrap();
    
    let result = server_handle.await.expect("Server task panicked");
    result.expect("Should succeed after retry with correct mechanism");
}

#[tokio::test]
async fn test_cancel_and_retry() {
    let (mut server_stream, mut client_stream) = create_stream_pair().await;
    
    let auth = SaslAuth::new();
    
    let server_handle = tokio::spawn(async move {
        auth.authenticate_server(&mut server_stream).await
    });
    
    // Send null byte
    client_stream.write_all(&[0u8]).await.unwrap();
    
    // Query available mechanisms first
    client_stream.write_all(b"AUTH\r\n").await.unwrap();
    client_stream.flush().await.unwrap();
    
    let response = read_response(&mut client_stream).await;
    assert!(response.starts_with("REJECTED"), "Expected REJECTED, got: {}", response);
    
    // Send CANCEL (protocol allows this as a way to reset state)
    client_stream.write_all(b"CANCEL\r\n").await.unwrap();
    client_stream.flush().await.unwrap();
    
    let response = read_response(&mut client_stream).await;
    assert!(response.starts_with("REJECTED"), "Expected REJECTED after CANCEL, got: {}", response);
    
    // Now do proper auth
    let uid = nix::unistd::getuid().as_raw();
    let hex_uid = encode_uid(uid);
    let auth_line = format!("AUTH EXTERNAL {}\r\n", hex_uid);
    client_stream.write_all(auth_line.as_bytes()).await.unwrap();
    client_stream.flush().await.unwrap();
    
    let response = read_response(&mut client_stream).await;
    assert!(response.starts_with("OK "), "Expected OK, got: {}", response);
    
    client_stream.write_all(b"BEGIN\r\n").await.unwrap();
    client_stream.flush().await.unwrap();
    
    let result = server_handle.await.expect("Server task panicked");
    result.expect("Should succeed after cancel and retry");
}

#[tokio::test]
async fn test_error_recovery() {
    let (mut server_stream, mut client_stream) = create_stream_pair().await;
    
    let auth = SaslAuth::new();
    
    let server_handle = tokio::spawn(async move {
        auth.authenticate_server(&mut server_stream).await
    });
    
    // Send null byte
    client_stream.write_all(&[0u8]).await.unwrap();
    
    // Send ERROR command
    client_stream.write_all(b"ERROR something went wrong\r\n").await.unwrap();
    client_stream.flush().await.unwrap();
    
    let response = read_response(&mut client_stream).await;
    assert!(response.starts_with("REJECTED"), "Expected REJECTED after ERROR, got: {}", response);
    
    // Now authenticate properly
    let uid = nix::unistd::getuid().as_raw();
    let hex_uid = encode_uid(uid);
    let auth_line = format!("AUTH EXTERNAL {}\r\n", hex_uid);
    client_stream.write_all(auth_line.as_bytes()).await.unwrap();
    client_stream.flush().await.unwrap();
    
    let response = read_response(&mut client_stream).await;
    assert!(response.starts_with("OK "), "Expected OK, got: {}", response);
    
    client_stream.write_all(b"BEGIN\r\n").await.unwrap();
    client_stream.flush().await.unwrap();
    
    let result = server_handle.await.expect("Server task panicked");
    result.expect("Should succeed after error recovery");
}

// ============================================================================
// Error Handling Tests
// ============================================================================

#[tokio::test]
async fn test_missing_null_byte() {
    let (mut server_stream, mut client_stream) = create_stream_pair().await;
    
    let auth = SaslAuth::new();
    
    let server_handle = tokio::spawn(async move {
        auth.authenticate_server(&mut server_stream).await
    });
    
    // Send non-null byte
    client_stream.write_all(&[0x01]).await.unwrap();
    
    let result = server_handle.await.expect("Server task panicked");
    assert!(result.is_err(), "Should fail without null byte");
    
    let err = result.unwrap_err();
    assert!(err.to_string().contains("null byte"), "Error should mention null byte: {}", err);
}

#[tokio::test]
async fn test_invalid_hex_uid() {
    let (mut server_stream, mut client_stream) = create_stream_pair().await;
    
    let auth = SaslAuth::new();
    
    let server_handle = tokio::spawn(async move {
        auth.authenticate_server(&mut server_stream).await
    });
    
    // Send null byte
    client_stream.write_all(&[0u8]).await.unwrap();
    
    // Send AUTH with invalid hex
    client_stream.write_all(b"AUTH EXTERNAL not_valid_hex!\r\n").await.unwrap();
    client_stream.flush().await.unwrap();
    
    let result = server_handle.await.expect("Server task panicked");
    assert!(result.is_err(), "Should fail with invalid hex");
}

#[tokio::test]
async fn test_invalid_uid_format() {
    let (mut server_stream, mut client_stream) = create_stream_pair().await;
    
    let auth = SaslAuth::new();
    
    let server_handle = tokio::spawn(async move {
        auth.authenticate_server(&mut server_stream).await
    });
    
    // Send null byte
    client_stream.write_all(&[0u8]).await.unwrap();
    
    // Send AUTH with hex that decodes to invalid UID
    let invalid_uid = hex::encode("not_a_number");
    let auth_line = format!("AUTH EXTERNAL {}\r\n", invalid_uid);
    client_stream.write_all(auth_line.as_bytes()).await.unwrap();
    client_stream.flush().await.unwrap();
    
    let result = server_handle.await.expect("Server task panicked");
    assert!(result.is_err(), "Should fail with invalid UID format");
}

#[tokio::test]
async fn test_missing_begin_after_ok() {
    let (mut server_stream, mut client_stream) = create_stream_pair().await;
    
    let auth = SaslAuth::new();
    
    let server_handle = tokio::spawn(async move {
        auth.authenticate_server(&mut server_stream).await
    });
    
    // Send null byte
    client_stream.write_all(&[0u8]).await.unwrap();
    
    // Authenticate
    let uid = nix::unistd::getuid().as_raw();
    let hex_uid = encode_uid(uid);
    let auth_line = format!("AUTH EXTERNAL {}\r\n", hex_uid);
    client_stream.write_all(auth_line.as_bytes()).await.unwrap();
    client_stream.flush().await.unwrap();
    
    let response = read_response(&mut client_stream).await;
    assert!(response.starts_with("OK "));
    
    // Send something other than BEGIN or NEGOTIATE_UNIX_FD
    client_stream.write_all(b"DATA something\r\n").await.unwrap();
    client_stream.flush().await.unwrap();
    
    let result = server_handle.await.expect("Server task panicked");
    assert!(result.is_err(), "Should fail without BEGIN");
    
    let err = result.unwrap_err();
    assert!(err.to_string().contains("BEGIN"), "Error should mention BEGIN: {}", err);
}

#[tokio::test]
async fn test_missing_begin_after_fd_negotiation() {
    let (mut server_stream, mut client_stream) = create_stream_pair().await;
    
    let auth = SaslAuth::new();
    
    let server_handle = tokio::spawn(async move {
        auth.authenticate_server(&mut server_stream).await
    });
    
    // Send null byte
    client_stream.write_all(&[0u8]).await.unwrap();
    
    // Authenticate
    let uid = nix::unistd::getuid().as_raw();
    let hex_uid = encode_uid(uid);
    let auth_line = format!("AUTH EXTERNAL {}\r\n", hex_uid);
    client_stream.write_all(auth_line.as_bytes()).await.unwrap();
    client_stream.flush().await.unwrap();
    
    let _ = read_response(&mut client_stream).await;
    
    // Negotiate FD
    client_stream.write_all(b"NEGOTIATE_UNIX_FD\r\n").await.unwrap();
    client_stream.flush().await.unwrap();
    
    let response = read_response(&mut client_stream).await;
    assert_eq!(response, "AGREE_UNIX_FD");
    
    // Send something other than BEGIN
    client_stream.write_all(b"DATA something\r\n").await.unwrap();
    client_stream.flush().await.unwrap();
    
    let result = server_handle.await.expect("Server task panicked");
    assert!(result.is_err(), "Should fail without BEGIN after FD negotiation");
}

#[tokio::test]
async fn test_client_disconnect_during_auth() {
    let (mut server_stream, mut client_stream) = create_stream_pair().await;
    
    let auth = SaslAuth::new();
    
    let server_handle = tokio::spawn(async move {
        auth.authenticate_server(&mut server_stream).await
    });
    
    // Send null byte
    client_stream.write_all(&[0u8]).await.unwrap();
    
    // Drop client before completing auth
    drop(client_stream);
    
    let result = server_handle.await.expect("Server task panicked");
    assert!(result.is_err(), "Should fail when client disconnects");
}

#[tokio::test]
async fn test_unexpected_command() {
    let (mut server_stream, mut client_stream) = create_stream_pair().await;
    
    let auth = SaslAuth::new();
    
    let server_handle = tokio::spawn(async move {
        auth.authenticate_server(&mut server_stream).await
    });
    
    // Send null byte
    client_stream.write_all(&[0u8]).await.unwrap();
    
    // Send unexpected command
    client_stream.write_all(b"FOOBAR unknown command\r\n").await.unwrap();
    client_stream.flush().await.unwrap();
    
    let result = server_handle.await.expect("Server task panicked");
    assert!(result.is_err(), "Should fail with unexpected command");
}

// ============================================================================
// UID Encoding Tests
// ============================================================================

#[tokio::test]
async fn test_various_uid_values() {
    // Test with various UID values
    let test_uids = [0u32, 1, 1000, 65534, 65535, 999999];
    
    for test_uid in test_uids {
        let (mut server_stream, mut client_stream) = create_stream_pair().await;
        
        let auth = SaslAuth::new();
        
        let server_handle = tokio::spawn(async move {
            auth.authenticate_server(&mut server_stream).await
        });
        
        client_auth_external(&mut client_stream, test_uid, false).await;
        
        let result = server_handle.await.expect("Server task panicked");
        let authenticated_uid = result.expect(&format!("Auth should succeed for UID {}", test_uid));
        
        assert_eq!(authenticated_uid, test_uid, "UID mismatch for {}", test_uid);
    }
}

// ============================================================================
// GUID Tests
// ============================================================================

#[test]
fn test_guid_format() {
    let guid = generate_guid();
    
    // D-Bus GUIDs are 32 hex characters
    assert_eq!(guid.len(), 32, "GUID should be 32 characters");
    assert!(guid.chars().all(|c| c.is_ascii_hexdigit()), "GUID should be all hex");
}

#[test]
fn test_guid_uniqueness() {
    let guids: Vec<String> = (0..100).map(|_| generate_guid()).collect();
    let unique_guids: std::collections::HashSet<&String> = guids.iter().collect();
    
    assert_eq!(guids.len(), unique_guids.len(), "All GUIDs should be unique");
}

// ============================================================================
// Concurrent Authentication Tests
// ============================================================================

#[tokio::test]
async fn test_multiple_concurrent_auths() {
    let dir = tempdir().expect("Failed to create temp dir");
    let socket_path = dir.path().join("test.sock");
    
    let listener = UnixListener::bind(&socket_path).expect("Failed to bind listener");
    
    let auth = std::sync::Arc::new(SaslAuth::new());
    
    // Spawn server acceptor
    let auth_clone = auth.clone();
    let server_handle = tokio::spawn(async move {
        let mut results = Vec::new();
        
        for _ in 0..5 {
            let (mut stream, _) = listener.accept().await.expect("Failed to accept");
            let auth = auth_clone.clone();
            
            let handle = tokio::spawn(async move {
                auth.authenticate_server(&mut stream).await
            });
            
            results.push(handle);
        }
        
        // Wait for all to complete
        let mut authenticated_uids = Vec::new();
        for handle in results {
            if let Ok(Ok(uid)) = handle.await {
                authenticated_uids.push(uid);
            }
        }
        authenticated_uids
    });
    
    // Spawn multiple clients
    let uid = nix::unistd::getuid().as_raw();
    let client_handles: Vec<_> = (0..5).map(|_| {
        let path = socket_path.clone();
        tokio::spawn(async move {
            let mut stream = UnixStream::connect(&path).await.expect("Failed to connect");
            client_auth_external(&mut stream, uid, false).await;
        })
    }).collect();
    
    // Wait for clients
    for handle in client_handles {
        handle.await.expect("Client task panicked");
    }
    
    // Verify server results
    let authenticated_uids = server_handle.await.expect("Server task panicked");
    assert_eq!(authenticated_uids.len(), 5, "All 5 clients should authenticate");
    assert!(authenticated_uids.iter().all(|&u| u == uid), "All UIDs should match");
}

// ============================================================================
// Timeout and Edge Case Tests  
// ============================================================================

#[tokio::test]
async fn test_slow_client_auth() {
    let (mut server_stream, mut client_stream) = create_stream_pair().await;
    
    let auth = SaslAuth::new();
    
    let server_handle = tokio::spawn(async move {
        // Use timeout to prevent hanging forever
        timeout(
            Duration::from_secs(5),
            auth.authenticate_server(&mut server_stream)
        ).await
    });
    
    // Simulate slow client with delays between operations
    tokio::time::sleep(Duration::from_millis(100)).await;
    client_stream.write_all(&[0u8]).await.unwrap();
    
    tokio::time::sleep(Duration::from_millis(100)).await;
    let uid = nix::unistd::getuid().as_raw();
    let hex_uid = encode_uid(uid);
    let auth_line = format!("AUTH EXTERNAL {}\r\n", hex_uid);
    client_stream.write_all(auth_line.as_bytes()).await.unwrap();
    client_stream.flush().await.unwrap();
    
    let _ = read_response(&mut client_stream).await;
    
    tokio::time::sleep(Duration::from_millis(100)).await;
    client_stream.write_all(b"BEGIN\r\n").await.unwrap();
    client_stream.flush().await.unwrap();
    
    let result = server_handle.await.expect("Server task panicked");
    let inner_result = result.expect("Should not timeout");
    inner_result.expect("Auth should succeed even with slow client");
}

#[tokio::test]
async fn test_empty_external_arg() {
    let (mut server_stream, mut client_stream) = create_stream_pair().await;
    
    let auth = SaslAuth::new();
    
    let server_handle = tokio::spawn(async move {
        auth.authenticate_server(&mut server_stream).await
    });
    
    // Send null byte
    client_stream.write_all(&[0u8]).await.unwrap();
    
    // Send AUTH EXTERNAL with empty argument (just whitespace after)
    client_stream.write_all(b"AUTH EXTERNAL \r\n").await.unwrap();
    client_stream.flush().await.unwrap();
    
    // This should fail due to empty hex string
    let result = server_handle.await.expect("Server task panicked");
    assert!(result.is_err(), "Should fail with empty EXTERNAL argument");
}

#[tokio::test]
async fn test_large_uid_number() {
    let (mut server_stream, mut client_stream) = create_stream_pair().await;
    
    let auth = SaslAuth::new();
    
    let server_handle = tokio::spawn(async move {
        auth.authenticate_server(&mut server_stream).await
    });
    
    // Use maximum u32 value
    let large_uid = u32::MAX;
    client_auth_external(&mut client_stream, large_uid, false).await;
    
    let result = server_handle.await.expect("Server task panicked");
    let authenticated_uid = result.expect("Auth should succeed with large UID");
    
    assert_eq!(authenticated_uid, large_uid);
}

// ============================================================================
// Encoding/Decoding Round-trip Tests
// ============================================================================

#[test]
fn test_uid_encoding_roundtrip() {
    let test_cases = [0u32, 1, 500, 1000, 65534, 65535, 999999, u32::MAX];
    
    for uid in test_cases {
        let encoded = encode_uid(uid);
        let decoded_bytes = hex::decode(&encoded).expect("Should decode");
        let decoded_str = String::from_utf8(decoded_bytes).expect("Should be UTF-8");
        let decoded_uid: u32 = decoded_str.parse().expect("Should parse as u32");
        
        assert_eq!(decoded_uid, uid, "Roundtrip failed for UID {}", uid);
    }
}

#[test]
fn test_uid_encoding_format() {
    // Verify the encoding produces expected hex output
    let uid = 1000u32;
    let encoded = encode_uid(uid);
    
    // "1000" in hex is "31303030"
    assert_eq!(encoded, "31303030");
    
    // Verify decoding
    let decoded = hex::decode(&encoded).unwrap();
    assert_eq!(decoded, b"1000");
}
