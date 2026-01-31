//! Serial number mapping for correlating D-Bus replies.
//!
//! When forwarding messages between buses, we need to track the original
//! serial numbers so we can properly correlate replies back to the
//! original client requests.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use crate::routing::Route;

/// Unique identifier for a client connection.
pub type ClientId = u64;

/// Information about a pending method call.
#[derive(Debug, Clone)]
pub struct PendingCall {
    /// The client that made the original call.
    pub client_id: ClientId,
    /// The serial number the client used.
    pub client_serial: u32,
    /// When the call was made (for timeout handling).
    pub timestamp: Instant,
}

/// Key for looking up pending calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SerialKey {
    /// Which bus the message was forwarded to.
    pub route: Route,
    /// The serial number used on that bus.
    pub serial: u32,
}

impl SerialKey {
    /// Create a new serial key.
    pub fn new(route: Route, serial: u32) -> Self {
        Self { route, serial }
    }
}

/// Maps forwarded serial numbers back to original client requests.
#[derive(Debug)]
pub struct SerialMap {
    /// Mapping from (route, forwarded_serial) to pending call info.
    pending: HashMap<SerialKey, PendingCall>,
    /// Timeout for pending calls.
    timeout: Duration,
}

impl Default for SerialMap {
    fn default() -> Self {
        Self::new()
    }
}

impl SerialMap {
    /// Create a new serial map with default timeout.
    pub fn new() -> Self {
        Self::with_timeout(Duration::from_secs(30))
    }

    /// Create a new serial map with a specific timeout.
    pub fn with_timeout(timeout: Duration) -> Self {
        Self {
            pending: HashMap::new(),
            timeout,
        }
    }

    /// Record a pending call.
    ///
    /// # Arguments
    /// * `route` - The bus the message was forwarded to.
    /// * `forwarded_serial` - The serial number used on that bus.
    /// * `client_id` - The client that made the original call.
    /// * `client_serial` - The serial number the client used.
    pub fn insert(
        &mut self,
        route: Route,
        forwarded_serial: u32,
        client_id: ClientId,
        client_serial: u32,
    ) {
        let key = SerialKey::new(route, forwarded_serial);
        let pending = PendingCall {
            client_id,
            client_serial,
            timestamp: Instant::now(),
        };
        self.pending.insert(key, pending);
    }

    /// Look up and remove a pending call by its reply.
    ///
    /// # Arguments
    /// * `route` - The bus the reply came from.
    /// * `reply_serial` - The serial number in the reply.
    ///
    /// # Returns
    /// The pending call info if found.
    pub fn remove(&mut self, route: Route, reply_serial: u32) -> Option<PendingCall> {
        let key = SerialKey::new(route, reply_serial);
        self.pending.remove(&key)
    }

    /// Clean up expired pending calls.
    ///
    /// Returns the number of calls that were cleaned up.
    pub fn cleanup_expired(&mut self) -> usize {
        let now = Instant::now();
        let timeout = self.timeout;
        let before = self.pending.len();
        
        self.pending.retain(|_, call| {
            now.duration_since(call.timestamp) < timeout
        });
        
        before - self.pending.len()
    }

    /// Get the number of pending calls.
    pub fn len(&self) -> usize {
        self.pending.len()
    }

    /// Check if there are no pending calls.
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// Get the number of pending calls for a specific client.
    pub fn pending_for_client(&self, client_id: ClientId) -> usize {
        self.pending.values()
            .filter(|call| call.client_id == client_id)
            .count()
    }
}

/// Generator for serial numbers.
#[derive(Debug)]
pub struct SerialGenerator {
    counter: AtomicU32,
}

impl Default for SerialGenerator {
    fn default() -> Self {
        Self::new()
    }
}

impl SerialGenerator {
    /// Create a new serial generator.
    pub fn new() -> Self {
        Self {
            counter: AtomicU32::new(1),
        }
    }

    /// Get the next serial number.
    ///
    /// Serial numbers wrap around at u32::MAX.
    pub fn next(&self) -> u32 {
        let serial = self.counter.fetch_add(1, Ordering::Relaxed);
        // D-Bus serial 0 is invalid, so wrap to 1
        if serial == 0 {
            self.counter.fetch_add(1, Ordering::Relaxed)
        } else {
            serial
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serial_map_insert_remove() {
        let mut map = SerialMap::new();
        
        map.insert(Route::Host, 100, 1, 5);
        
        let call = map.remove(Route::Host, 100);
        assert!(call.is_some());
        let call = call.unwrap();
        assert_eq!(call.client_id, 1);
        assert_eq!(call.client_serial, 5);
        
        // Should be removed now
        assert!(map.remove(Route::Host, 100).is_none());
    }

    #[test]
    fn test_serial_map_different_routes() {
        let mut map = SerialMap::new();
        
        // Same serial on different routes should be tracked separately
        map.insert(Route::Host, 100, 1, 5);
        map.insert(Route::Container, 100, 2, 10);
        
        let host_call = map.remove(Route::Host, 100).unwrap();
        assert_eq!(host_call.client_id, 1);
        
        let container_call = map.remove(Route::Container, 100).unwrap();
        assert_eq!(container_call.client_id, 2);
    }

    #[test]
    fn test_serial_generator() {
        let gen = SerialGenerator::new();
        
        assert_eq!(gen.next(), 1);
        assert_eq!(gen.next(), 2);
        assert_eq!(gen.next(), 3);
    }
}
