//! Error types for kapsule-dbus-mux.

use thiserror::Error;

/// Result type alias for kapsule-dbus-mux operations.
pub type Result<T> = std::result::Result<T, Error>;

/// Errors that can occur in the D-Bus multiplexer.
#[derive(Debug, Error)]
pub enum Error {
    /// I/O error.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// D-Bus error from zbus.
    #[error("D-Bus error: {0}")]
    DBus(#[from] zbus::Error),

    /// Authentication error.
    #[error("Authentication error: {0}")]
    Auth(#[from] AuthError),

    /// Routing error.
    #[error("Routing error: {0}")]
    Routing(#[from] RoutingError),

    /// Client connection error.
    #[error("Client error: {0}")]
    Client(#[from] ClientError),

    /// Message parsing error.
    #[error("Message error: {0}")]
    Message(#[from] MessageError),

    /// Configuration error.
    #[error("Configuration error: {0}")]
    Config(String),

    /// The multiplexer was shut down.
    #[error("Multiplexer shut down")]
    Shutdown,
}

/// Authentication-related errors.
#[derive(Debug, Error)]
pub enum AuthError {
    /// The client sent an unsupported authentication mechanism.
    #[error("Unsupported authentication mechanism: {0}")]
    UnsupportedMechanism(String),

    /// Expected the null byte at the start of authentication.
    #[error("Expected null byte at start of authentication")]
    ExpectedNullByte,

    /// Expected the BEGIN command after successful authentication.
    #[error("Expected BEGIN after authentication")]
    ExpectedBegin,

    /// Invalid authentication data format.
    #[error("Invalid auth data: {0}")]
    InvalidData(String),

    /// Authentication timeout.
    #[error("Authentication timed out")]
    Timeout,

    /// The UID provided does not match expectations.
    #[error("UID mismatch: expected {expected}, got {actual}")]
    UidMismatch { expected: u32, actual: u32 },
}

/// Routing-related errors.
#[derive(Debug, Error)]
pub enum RoutingError {
    /// No route found for the destination.
    #[error("No route found for destination: {0}")]
    NoRoute(String),

    /// Failed to update routing table.
    #[error("Failed to update routing table: {0}")]
    UpdateFailed(String),

    /// The destination name is invalid.
    #[error("Invalid destination name: {0}")]
    InvalidDestination(String),
}

/// Client connection errors.
#[derive(Debug, Error)]
pub enum ClientError {
    /// Client disconnected unexpectedly.
    #[error("Client disconnected")]
    Disconnected,

    /// Client sent an invalid message.
    #[error("Invalid message from client: {0}")]
    InvalidMessage(String),

    /// Client not authenticated.
    #[error("Client not authenticated")]
    NotAuthenticated,

    /// Client ID not found.
    #[error("Unknown client ID: {0}")]
    UnknownClient(u64),

    /// Too many pending requests from client.
    #[error("Too many pending requests from client")]
    TooManyPending,
}

/// Message-related errors.
#[derive(Debug, Error)]
pub enum MessageError {
    /// Missing required message header field.
    #[error("Missing message header field: {0}")]
    MissingHeader(String),

    /// Invalid message serial number.
    #[error("Invalid serial number")]
    InvalidSerial,

    /// No reply serial in reply message.
    #[error("Reply message missing reply serial")]
    NoReplySerial,

    /// Message body deserialization failed.
    #[error("Failed to deserialize message body: {0}")]
    DeserializeFailed(String),

    /// Message body serialization failed.
    #[error("Failed to serialize message body: {0}")]
    SerializeFailed(String),
}

impl From<zbus::zvariant::Error> for Error {
    fn from(err: zbus::zvariant::Error) -> Self {
        Error::Message(MessageError::DeserializeFailed(err.to_string()))
    }
}
