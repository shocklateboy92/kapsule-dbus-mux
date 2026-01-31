//! D-Bus message utilities.
//!
//! Helper functions for working with D-Bus messages.

use std::sync::Arc;

use zbus::message::{Message, Type as MessageType};
use zbus::names::BusName;
use zbus::zvariant::Type;

use crate::error::{MessageError, Result};

/// Extension trait for working with D-Bus messages.
pub trait MessageExt {
    /// Get the destination of the message.
    fn destination_str(&self) -> Option<String>;
    
    /// Get the sender of the message.
    fn sender_str(&self) -> Option<String>;
    
    /// Get the interface of the message.
    fn interface_str(&self) -> Option<String>;
    
    /// Get the member (method/signal name) of the message.
    fn member_str(&self) -> Option<String>;
    
    /// Get the path of the message.
    fn path_str(&self) -> Option<String>;
    
    /// Get the serial number of the message.
    fn serial(&self) -> u32;
    
    /// Get the reply serial of the message (for replies).
    fn reply_serial(&self) -> Option<u32>;
    
    /// Check if this is a method call.
    fn is_method_call(&self) -> bool;
    
    /// Check if this is a method return.
    fn is_method_return(&self) -> bool;
    
    /// Check if this is an error reply.
    fn is_error(&self) -> bool;
    
    /// Check if this is a signal.
    fn is_signal(&self) -> bool;
    
    /// Check if this message is addressed to the D-Bus daemon itself.
    fn is_dbus_daemon_call(&self) -> bool;
    
    /// Check if this is a NameOwnerChanged signal.
    fn is_name_owner_changed(&self) -> bool;
}

impl MessageExt for Message {
    fn destination_str(&self) -> Option<String> {
        self.header().destination().map(|d| d.to_string())
    }
    
    fn sender_str(&self) -> Option<String> {
        self.header().sender().map(|s| s.to_string())
    }
    
    fn interface_str(&self) -> Option<String> {
        self.header().interface().map(|i| i.to_string())
    }
    
    fn member_str(&self) -> Option<String> {
        self.header().member().map(|m| m.to_string())
    }
    
    fn path_str(&self) -> Option<String> {
        self.header().path().map(|p| p.to_string())
    }
    
    fn serial(&self) -> u32 {
        self.primary_header().serial_num().get()
    }
    
    fn reply_serial(&self) -> Option<u32> {
        self.header().reply_serial().map(|n| n.get())
    }
    
    fn is_method_call(&self) -> bool {
        self.primary_header().msg_type() == MessageType::MethodCall
    }
    
    fn is_method_return(&self) -> bool {
        self.primary_header().msg_type() == MessageType::MethodReturn
    }
    
    fn is_error(&self) -> bool {
        self.primary_header().msg_type() == MessageType::Error
    }
    
    fn is_signal(&self) -> bool {
        self.primary_header().msg_type() == MessageType::Signal
    }
    
    fn is_dbus_daemon_call(&self) -> bool {
        self.destination_str().as_deref() == Some("org.freedesktop.DBus")
    }
    
    fn is_name_owner_changed(&self) -> bool {
        self.is_signal()
            && self.interface_str().as_deref() == Some("org.freedesktop.DBus")
            && self.member_str().as_deref() == Some("NameOwnerChanged")
    }
}

impl MessageExt for Arc<Message> {
    fn destination_str(&self) -> Option<String> {
        self.as_ref().destination_str()
    }
    
    fn sender_str(&self) -> Option<String> {
        self.as_ref().sender_str()
    }
    
    fn interface_str(&self) -> Option<String> {
        self.as_ref().interface_str()
    }
    
    fn member_str(&self) -> Option<String> {
        self.as_ref().member_str()
    }
    
    fn path_str(&self) -> Option<String> {
        self.as_ref().path_str()
    }
    
    fn serial(&self) -> u32 {
        self.as_ref().serial()
    }
    
    fn reply_serial(&self) -> Option<u32> {
        self.as_ref().reply_serial()
    }
    
    fn is_method_call(&self) -> bool {
        self.as_ref().is_method_call()
    }
    
    fn is_method_return(&self) -> bool {
        self.as_ref().is_method_return()
    }
    
    fn is_error(&self) -> bool {
        self.as_ref().is_error()
    }
    
    fn is_signal(&self) -> bool {
        self.as_ref().is_signal()
    }
    
    fn is_dbus_daemon_call(&self) -> bool {
        self.as_ref().is_dbus_daemon_call()
    }
    
    fn is_name_owner_changed(&self) -> bool {
        self.as_ref().is_name_owner_changed()
    }
}

/// Parse a NameOwnerChanged signal body.
///
/// Returns (name, old_owner, new_owner).
/// Empty strings indicate the name was acquired (old_owner empty) or released (new_owner empty).
pub fn parse_name_owner_changed(msg: &Message) -> Result<(String, String, String)> {
    let body = msg.body();
    let (name, old_owner, new_owner): (String, String, String) = body
        .deserialize()
        .map_err(|e| MessageError::DeserializeFailed(e.to_string()))?;
    
    Ok((name, old_owner, new_owner))
}

/// Create a method return message from a request message.
pub fn create_method_return<B: serde::Serialize + Type>(
    request: &Message,
    body: &B,
) -> Result<Message> {
    let reply = Message::method_return(&request.header())?
        .build(body)?;
    Ok(reply)
}

/// Create an error message from a request message.
pub fn create_error(
    request: &Message,
    error_name: &str,
    error_message: &str,
) -> Result<Message> {
    let reply = Message::error(&request.header(), error_name)?
        .build(&(error_message,))?;
    Ok(reply)
}

/// Builder for creating D-Bus method return messages.
/// This stores the original message for reply construction.
pub struct MethodReturnBuilder {
    original: Message,
    destination: Option<String>,
}

impl MethodReturnBuilder {
    /// Create a new method return builder from an original message.
    pub fn new(original: &Message) -> Self {
        // Clone the message to get ownership
        Self {
            original: original.clone(),
            destination: None,
        }
    }
    
    /// Set the destination of the reply.
    pub fn destination(mut self, dest: impl Into<String>) -> Self {
        self.destination = Some(dest.into());
        self
    }
    
    /// Build the reply message with the given body.
    pub fn build<B: serde::Serialize + Type>(self, body: &B) -> Result<Message> {
        let mut builder = Message::method_return(&self.original.header())?;
        
        if let Some(dest) = &self.destination {
            let bus_name: BusName = dest.as_str().try_into()
                .map_err(|e| MessageError::DeserializeFailed(format!("Invalid destination: {}", e)))?;
            builder = builder.destination(bus_name)?;
        }
        
        let msg = builder.build(body)?;
        Ok(msg)
    }
    
    /// Build an empty reply message.
    pub fn build_empty(self) -> Result<Message> {
        self.build(&())
    }
}

/// Builder for creating D-Bus error messages.
/// This stores the original message for error construction.
pub struct ErrorBuilder {
    original: Message,
    destination: Option<String>,
    error_name: String,
}

impl ErrorBuilder {
    /// Create a new error builder from an original message.
    pub fn new(original: &Message, error_name: impl Into<String>) -> Self {
        Self {
            original: original.clone(),
            destination: None,
            error_name: error_name.into(),
        }
    }
    
    /// Set the destination of the error.
    pub fn destination(mut self, dest: impl Into<String>) -> Self {
        self.destination = Some(dest.into());
        self
    }
    
    /// Build the error message with a message body.
    pub fn build(self, message: &str) -> Result<Message> {
        let error_name: zbus::names::ErrorName = self.error_name.as_str().try_into()
            .map_err(|e| MessageError::DeserializeFailed(format!("Invalid error name: {}", e)))?;
        
        let mut builder = Message::error(
            &self.original.header(),
            error_name,
        )?;
        
        if let Some(dest) = &self.destination {
            let bus_name: BusName = dest.as_str().try_into()
                .map_err(|e| MessageError::DeserializeFailed(format!("Invalid destination: {}", e)))?;
            builder = builder.destination(bus_name)?;
        }
        
        let msg = builder.build(&(message,))?;
        Ok(msg)
    }
}

/// Common D-Bus error names.
pub mod error_names {
    pub const UNKNOWN_METHOD: &str = "org.freedesktop.DBus.Error.UnknownMethod";
    pub const NAME_HAS_NO_OWNER: &str = "org.freedesktop.DBus.Error.NameHasNoOwner";
    pub const SERVICE_UNKNOWN: &str = "org.freedesktop.DBus.Error.ServiceUnknown";
    pub const FAILED: &str = "org.freedesktop.DBus.Error.Failed";
    pub const ACCESS_DENIED: &str = "org.freedesktop.DBus.Error.AccessDenied";
    pub const INVALID_ARGS: &str = "org.freedesktop.DBus.Error.InvalidArgs";
    pub const NO_REPLY: &str = "org.freedesktop.DBus.Error.NoReply";
}
