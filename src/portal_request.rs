//! Portal request tracking for XDG Desktop Portal signals.
//!
//! The XDG Desktop Portal uses a request/response pattern where:
//! 1. Client calls a portal method (e.g., ScreenCast.CreateSession)
//! 2. Portal returns a request handle path (e.g., /org/freedesktop/portal/desktop/request/1_156/token)
//! 3. Portal emits a Response signal on that path when the operation completes
//!
//! The request handle path includes the caller's unique name (with ':' removed and '.' replaced by '_').
//! When proxying through the mux, the portal sees the mux's unique name, not the original client's.
//! This module tracks which client made each portal request so Response signals can be delivered
//! to the correct client.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use tracing::{debug, info, warn};
use zbus::message::Message;
use zbus::zvariant::ObjectPath;

use crate::message::MessageExt;
use crate::serial_map::ClientId;

/// Tracks portal request handles to client ID mappings.
#[derive(Debug, Default)]
pub struct PortalRequestTracker {
    /// Maps request handle paths to the client that initiated the request.
    /// Key: request path (e.g., "/org/freedesktop/portal/desktop/request/1_156/token")
    /// Value: (client_id, timestamp)
    requests: HashMap<String, PendingPortalRequest>,
    /// Timeout for pending requests (default 5 minutes).
    timeout: Duration,
}

/// A pending portal request.
#[derive(Debug, Clone)]
struct PendingPortalRequest {
    /// The client that made the request.
    client_id: ClientId,
    /// When the request was registered.
    timestamp: Instant,
}

impl PortalRequestTracker {
    /// Create a new portal request tracker.
    pub fn new() -> Self {
        Self {
            requests: HashMap::new(),
            timeout: Duration::from_secs(300), // 5 minutes
        }
    }

    /// Register a portal request handle for a client.
    pub fn register(&mut self, request_path: String, client_id: ClientId) {
        info!(
            request_path = %request_path,
            client_id = client_id,
            "Registered portal request handle"
        );
        self.requests.insert(request_path, PendingPortalRequest {
            client_id,
            timestamp: Instant::now(),
        });
    }

    /// Look up which client should receive a Response signal for a given path.
    /// 
    /// This removes the entry from the tracker since Response signals are one-time.
    pub fn lookup_and_remove(&mut self, request_path: &str) -> Option<ClientId> {
        if let Some(pending) = self.requests.remove(request_path) {
            let age_ms = pending.timestamp.elapsed().as_millis();
            info!(
                request_path = %request_path,
                client_id = pending.client_id,
                age_ms = age_ms,
                "Found portal request owner"
            );
            Some(pending.client_id)
        } else {
            debug!(
                request_path = %request_path,
                "No registered owner for portal request path"
            );
            None
        }
    }

    /// Clean up expired pending requests.
    pub fn cleanup_expired(&mut self) -> usize {
        let timeout = self.timeout;
        let before = self.requests.len();
        self.requests.retain(|path, pending| {
            let expired = pending.timestamp.elapsed() >= timeout;
            if expired {
                warn!(
                    request_path = %path,
                    client_id = pending.client_id,
                    age_secs = pending.timestamp.elapsed().as_secs(),
                    "Removing expired portal request"
                );
            }
            !expired
        });
        before - self.requests.len()
    }

    /// Remove all requests for a specific client (e.g., on disconnect).
    pub fn remove_client(&mut self, client_id: ClientId) -> usize {
        let before = self.requests.len();
        self.requests.retain(|path, pending| {
            if pending.client_id == client_id {
                debug!(
                    request_path = %path,
                    client_id = client_id,
                    "Removing portal request on client disconnect"
                );
                false
            } else {
                true
            }
        });
        before - self.requests.len()
    }
}

/// Check if a message is a portal method call that will return a request handle.
/// 
/// Portal methods that return request handles typically have names like:
/// - CreateSession, SelectSources, Start (ScreenCast)
/// - OpenFile, SaveFile (FileChooser)
/// - etc.
/// 
/// The common pattern is that these are on interfaces under org.freedesktop.portal.*
pub fn is_portal_request_method(msg: &Message) -> bool {
    if !msg.is_method_call() {
        return false;
    }

    let interface: String = match msg.interface_str() {
        Some(i) => i,
        None => return false,
    };

    // Check if it's a portal interface
    if !interface.starts_with("org.freedesktop.portal.") {
        return false;
    }

    // Exclude some interfaces that don't use the request pattern
    if interface == "org.freedesktop.portal.Request" 
        || interface == "org.freedesktop.portal.Session"
        || interface == "org.freedesktop.portal.Settings"
        || interface == "org.freedesktop.portal.ProxyResolver"
        || interface == "org.freedesktop.portal.NetworkMonitor"
        || interface == "org.freedesktop.portal.MemoryMonitor"
        || interface == "org.freedesktop.portal.PowerProfileMonitor"
    {
        return false;
    }

    true
}

/// Check if a message is a portal Request.Response signal.
pub fn is_portal_response_signal(msg: &Message) -> bool {
    if !msg.is_signal() {
        return false;
    }

    msg.interface_str().as_deref() == Some("org.freedesktop.portal.Request")
        && msg.member_str().as_deref() == Some("Response")
}

/// Decode and log the portal Response body for debugging.
/// Returns the response code (0 = success, 1 = user cancelled, 2 = other error).
pub fn decode_portal_response(msg: &Message) -> Option<u32> {
    use std::collections::HashMap;
    use zbus::zvariant::OwnedValue;
    
    let body = msg.body();
    
    // Response body is (u, a{sv}) - response code and results dict
    if let Ok((response_code, results)) = body.deserialize::<(u32, HashMap<String, OwnedValue>)>() {
        info!(
            response_code = response_code,
            "Portal Response decoded"
        );
        
        // Log interesting fields from the results
        for (key, value) in &results {
            debug!(key = %key, value = ?value, "Portal Response result");
        }
        
        return Some(response_code);
    }
    
    warn!("Failed to decode portal Response body");
    None
}

/// Extract the request handle path from a portal method reply.
/// 
/// Portal methods return an object path like:
/// `/org/freedesktop/portal/desktop/request/1_156/token`
pub fn extract_request_handle(msg: &Message) -> Option<String> {
    use tracing::info;
    
    if !msg.is_method_return() {
        return None;
    }

    // Log what we're checking
    let body = msg.body();
    let signature = body.signature();
    info!(
        signature = %signature,
        reply_serial = ?msg.reply_serial(),
        "Checking method_return for portal request handle"
    );

    // Try to deserialize as a single ObjectPath (most common)
    if let Ok(path) = body.deserialize::<ObjectPath>() {
        let path_str = path.as_str();
        info!(path = %path_str, "Deserialized as ObjectPath");
        if path_str.starts_with("/org/freedesktop/portal/desktop/request/") {
            info!(path = %path_str, "Found portal request handle!");
            return Some(path_str.to_string());
        }
    }

    // Try as a tuple with ObjectPath as first element
    if let Ok((path,)) = body.deserialize::<(ObjectPath,)>() {
        let path_str = path.as_str();
        info!(path = %path_str, "Deserialized as tuple with ObjectPath");
        if path_str.starts_with("/org/freedesktop/portal/desktop/request/") {
            info!(path = %path_str, "Found portal request handle (tuple)!");
            return Some(path_str.to_string());
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tracker_basic() {
        let mut tracker = PortalRequestTracker::new();
        
        let path = "/org/freedesktop/portal/desktop/request/1_156/token123".to_string();
        tracker.register(path.clone(), 42);
        
        assert_eq!(tracker.lookup_and_remove(&path), Some(42));
        assert_eq!(tracker.lookup_and_remove(&path), None); // Already removed
    }

    #[test]
    fn test_tracker_remove_client() {
        let mut tracker = PortalRequestTracker::new();
        
        tracker.register("/path/1".to_string(), 1);
        tracker.register("/path/2".to_string(), 1);
        tracker.register("/path/3".to_string(), 2);
        
        let removed = tracker.remove_client(1);
        assert_eq!(removed, 2);
        
        assert_eq!(tracker.lookup_and_remove("/path/1"), None);
        assert_eq!(tracker.lookup_and_remove("/path/2"), None);
        assert_eq!(tracker.lookup_and_remove("/path/3"), Some(2));
    }
}
