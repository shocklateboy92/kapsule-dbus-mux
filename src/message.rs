//! D-Bus message utilities.
//!
//! Helper functions for working with D-Bus messages.

use std::num::NonZeroU32;
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

/// Clone a message with a new serial number.
///
/// This is used when forwarding messages to upstream buses to avoid
/// serial number collisions between different clients.
///
/// # Arguments
/// * `msg` - The original message to clone
/// * `new_serial` - The new serial number to use
///
/// # Returns
/// A new message with the same content but different serial number
pub fn clone_message_with_serial(msg: &Message, new_serial: u32) -> Result<Message> {
    let new_serial = NonZeroU32::new(new_serial)
        .ok_or(MessageError::InvalidSerial)?;
    
    // Get the raw bytes and modify the serial in place
    // D-Bus message format: bytes 8-11 contain the serial (after endianness byte at 0)
    let data = msg.data();
    let mut bytes = data.bytes().to_vec();
    
    // Determine endianness from first byte
    let little_endian = bytes[0] == b'l';
    
    // Serial is at bytes 8-11 in the primary header
    let serial_bytes = if little_endian {
        new_serial.get().to_le_bytes()
    } else {
        new_serial.get().to_be_bytes()
    };
    bytes[8..12].copy_from_slice(&serial_bytes);
    
    // Reconstruct the message
    let endian = if little_endian {
        zbus::zvariant::Endian::Little
    } else {
        zbus::zvariant::Endian::Big
    };
    let context = zbus::zvariant::serialized::Context::new_dbus(endian, 0);
    let new_data = zbus::zvariant::serialized::Data::new(bytes, context);
    
    // SAFETY: We only modified the serial field, preserving all other message structure
    let new_msg = unsafe { Message::from_bytes(new_data)? };
    
    Ok(new_msg)
}

/// Clone a reply message with a new reply_serial field.
///
/// This is used when forwarding replies back to clients to restore
/// the original serial number the client used.
///
/// # Arguments
/// * `msg` - The reply message to clone
/// * `new_reply_serial` - The new reply_serial to use
///
/// # Returns
/// A new message with the same content but different reply_serial
pub fn clone_reply_with_serial(msg: &Message, new_reply_serial: u32) -> Result<Message> {
    // For replies, we need to modify the reply_serial in the header fields
    // This is more complex as it's in the variable-length header array
    // We'll rebuild the message using the builder API
    
    let header = msg.header();
    let primary = msg.primary_header();
    
    let new_reply_serial = NonZeroU32::new(new_reply_serial)
        .ok_or(MessageError::InvalidSerial)?;
    
    // Start with a builder from the header
    let mut builder = zbus::message::Builder::from(header.clone())
        .reply_serial(Some(new_reply_serial));
    
    // Set a new serial for this message (it will get its own serial)
    if let Some(serial) = NonZeroU32::new(primary.serial_num().get()) {
        builder = builder.serial(serial);
    }
    
    // Build with the original body bytes
    let body = msg.body();
    let body_data = body.data();
    let body_bytes = body_data.bytes();
    
    // Get signature - clone the reference
    let sig = header.signature().clone();
    
    // SAFETY: We're rebuilding from valid message bytes
    let new_msg = unsafe {
        builder.build_raw_body(body_bytes, sig, Vec::new())?
    };
    Ok(new_msg)
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

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::message::Message;

    fn create_test_method_call() -> Message {
        Message::method_call("/org/test/Path", "TestMethod")
            .unwrap()
            .destination("org.test.Destination")
            .unwrap()
            .interface("org.test.Interface")
            .unwrap()
            .build(&("hello", 42i32))
            .unwrap()
    }

    fn create_test_method_return(request: &Message) -> Message {
        Message::method_return(&request.header())
            .unwrap()
            .build(&("response", 123i32))
            .unwrap()
    }

    #[test]
    fn test_clone_message_with_serial() {
        let original = create_test_method_call();
        let original_serial = original.serial();
        
        let new_serial = 9999u32;
        let cloned = clone_message_with_serial(&original, new_serial).unwrap();
        
        // Serial should be changed
        assert_eq!(cloned.serial(), new_serial);
        assert_ne!(cloned.serial(), original_serial);
        
        // All other fields should be preserved
        assert_eq!(cloned.destination_str(), original.destination_str());
        assert_eq!(cloned.interface_str(), original.interface_str());
        assert_eq!(cloned.member_str(), original.member_str());
        assert_eq!(cloned.path_str(), original.path_str());
        
        // Body should be preserved
        let original_body: (String, i32) = original.body().deserialize().unwrap();
        let cloned_body: (String, i32) = cloned.body().deserialize().unwrap();
        assert_eq!(original_body, cloned_body);
    }

    #[test]
    fn test_clone_message_with_serial_zero_rejected() {
        let original = create_test_method_call();
        
        // Serial 0 is invalid in D-Bus
        let result = clone_message_with_serial(&original, 0);
        assert!(result.is_err());
    }

    #[test]
    fn test_clone_message_preserves_message_type() {
        let original = create_test_method_call();
        let cloned = clone_message_with_serial(&original, 5000).unwrap();
        
        assert!(original.is_method_call());
        assert!(cloned.is_method_call());
    }

    #[test]
    fn test_clone_reply_with_serial() {
        let request = create_test_method_call();
        let original_reply = create_test_method_return(&request);
        let original_reply_serial = original_reply.reply_serial();
        
        let new_reply_serial = 7777u32;
        let cloned_reply = clone_reply_with_serial(&original_reply, new_reply_serial).unwrap();
        
        // Reply serial should be changed
        assert_eq!(cloned_reply.reply_serial(), Some(new_reply_serial));
        assert_ne!(cloned_reply.reply_serial(), original_reply_serial);
        
        // Should still be a method return
        assert!(cloned_reply.is_method_return());
        
        // Body should be preserved
        let original_body: (String, i32) = original_reply.body().deserialize().unwrap();
        let cloned_body: (String, i32) = cloned_reply.body().deserialize().unwrap();
        assert_eq!(original_body, cloned_body);
    }

    #[test]
    fn test_clone_reply_with_serial_zero_rejected() {
        let request = create_test_method_call();
        let reply = create_test_method_return(&request);
        
        // Reply serial 0 is invalid in D-Bus
        let result = clone_reply_with_serial(&reply, 0);
        assert!(result.is_err());
    }

    #[test]
    fn test_clone_message_different_serials() {
        let original = create_test_method_call();
        
        // Clone with multiple different serials
        let clone1 = clone_message_with_serial(&original, 100).unwrap();
        let clone2 = clone_message_with_serial(&original, 200).unwrap();
        let clone3 = clone_message_with_serial(&original, 300).unwrap();
        
        assert_eq!(clone1.serial(), 100);
        assert_eq!(clone2.serial(), 200);
        assert_eq!(clone3.serial(), 300);
        
        // All should have the same content
        assert_eq!(clone1.destination_str(), clone2.destination_str());
        assert_eq!(clone2.destination_str(), clone3.destination_str());
    }

    #[test]
    fn test_message_ext_serial() {
        let msg = create_test_method_call();
        let serial = msg.serial();
        assert!(serial > 0);  // Valid serial is non-zero
    }

    #[test]
    fn test_message_ext_is_method_call() {
        let msg = create_test_method_call();
        assert!(msg.is_method_call());
        assert!(!msg.is_method_return());
        assert!(!msg.is_error());
        assert!(!msg.is_signal());
    }

    #[test]
    fn test_message_ext_is_method_return() {
        let request = create_test_method_call();
        let reply = create_test_method_return(&request);
        
        assert!(reply.is_method_return());
        assert!(!reply.is_method_call());
        assert!(!reply.is_error());
        assert!(!reply.is_signal());
    }

    #[test]
    fn test_message_ext_reply_serial() {
        let request = create_test_method_call();
        let request_serial = request.serial();
        let reply = create_test_method_return(&request);
        
        // Reply should reference the request's serial
        assert_eq!(reply.reply_serial(), Some(request_serial));
        
        // Request doesn't have a reply serial
        assert_eq!(request.reply_serial(), None);
    }

    #[test]
    fn test_is_dbus_daemon_call() {
        let daemon_call = Message::method_call("/org/freedesktop/DBus", "ListNames")
            .unwrap()
            .destination("org.freedesktop.DBus")
            .unwrap()
            .interface("org.freedesktop.DBus")
            .unwrap()
            .build(&())
            .unwrap();
        
        assert!(daemon_call.is_dbus_daemon_call());
        
        let regular_call = create_test_method_call();
        assert!(!regular_call.is_dbus_daemon_call());
    }

    #[test]
    fn test_method_return_builder() {
        let request = create_test_method_call();
        let reply = MethodReturnBuilder::new(&request)
            .destination(":1.123")
            .build(&"test response")
            .unwrap();
        
        assert!(reply.is_method_return());
        assert_eq!(reply.destination_str(), Some(":1.123".to_string()));
        assert_eq!(reply.reply_serial(), Some(request.serial()));
        
        let body: String = reply.body().deserialize().unwrap();
        assert_eq!(body, "test response");
    }

    #[test]
    fn test_error_builder() {
        let request = create_test_method_call();
        let error = ErrorBuilder::new(&request, error_names::FAILED)
            .destination(":1.123")
            .build("Something went wrong")
            .unwrap();
        
        assert!(error.is_error());
        assert_eq!(error.destination_str(), Some(":1.123".to_string()));
        assert_eq!(error.reply_serial(), Some(request.serial()));
    }
}
