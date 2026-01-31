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

    #[test]
    fn test_serial_generator_thread_safety() {
        use std::sync::Arc;
        use std::thread;
        
        let gen = Arc::new(SerialGenerator::new());
        let mut handles = vec![];
        
        // Spawn multiple threads that each generate serials
        for _ in 0..10 {
            let gen = Arc::clone(&gen);
            handles.push(thread::spawn(move || {
                let mut serials = vec![];
                for _ in 0..100 {
                    serials.push(gen.next());
                }
                serials
            }));
        }
        
        // Collect all serials
        let mut all_serials: Vec<u32> = vec![];
        for handle in handles {
            all_serials.extend(handle.join().unwrap());
        }
        
        // All serials should be unique (no collisions)
        all_serials.sort();
        let len_before = all_serials.len();
        all_serials.dedup();
        assert_eq!(all_serials.len(), len_before, "Serial collision detected!");
    }

    #[test]
    fn test_serial_map_multiple_clients_same_serial() {
        let mut map = SerialMap::new();
        
        // Simulate what happens with proper serial translation:
        // Client 1 sends with client_serial=1, gets forwarded_serial=100
        // Client 2 sends with client_serial=1, gets forwarded_serial=101
        map.insert(Route::Host, 100, 1, 1);  // (route, forwarded, client_id, client_serial)
        map.insert(Route::Host, 101, 2, 1);
        
        // Both should be retrievable without collision
        let call1 = map.remove(Route::Host, 100).unwrap();
        assert_eq!(call1.client_id, 1);
        assert_eq!(call1.client_serial, 1);
        
        let call2 = map.remove(Route::Host, 101).unwrap();
        assert_eq!(call2.client_id, 2);
        assert_eq!(call2.client_serial, 1);
    }

    #[test]
    fn test_serial_map_collision_scenario() {
        // This test demonstrates the OLD bug:
        // Without serial translation, both clients would use the same key
        let mut map = SerialMap::new();
        
        // OLD BUG: If we used client_serial as forwarded_serial directly,
        // the second insert would overwrite the first
        // map.insert(Route::Host, 1, 1, 1);  // Client 1, serial 1
        // map.insert(Route::Host, 1, 2, 1);  // Client 2, serial 1 - OVERWRITES!
        
        // NEW CORRECT: Each gets a unique forwarded serial
        map.insert(Route::Host, 1001, 1, 1);  // Client 1's serial 1 -> forwarded 1001
        map.insert(Route::Host, 1002, 2, 1);  // Client 2's serial 1 -> forwarded 1002
        
        assert_eq!(map.len(), 2);  // Both are preserved
    }

    #[test]
    fn test_pending_for_client() {
        let mut map = SerialMap::new();
        
        // Client 1 has 3 pending calls
        map.insert(Route::Host, 100, 1, 1);
        map.insert(Route::Host, 101, 1, 2);
        map.insert(Route::Host, 102, 1, 3);
        
        // Client 2 has 2 pending calls
        map.insert(Route::Host, 200, 2, 1);
        map.insert(Route::Container, 201, 2, 2);
        
        assert_eq!(map.pending_for_client(1), 3);
        assert_eq!(map.pending_for_client(2), 2);
        assert_eq!(map.pending_for_client(999), 0);  // Unknown client
    }

    #[test]
    fn test_cleanup_expired() {
        use std::time::Duration;
        use std::thread;
        
        let mut map = SerialMap::with_timeout(Duration::from_millis(50));
        
        map.insert(Route::Host, 100, 1, 5);
        map.insert(Route::Host, 101, 2, 10);
        
        assert_eq!(map.len(), 2);
        
        // Wait for timeout
        thread::sleep(Duration::from_millis(100));
        
        let cleaned = map.cleanup_expired();
        assert_eq!(cleaned, 2);
        assert_eq!(map.len(), 0);
    }

    #[test]
    fn test_cleanup_expired_partial() {
        use std::time::Duration;
        use std::thread;
        
        let mut map = SerialMap::with_timeout(Duration::from_millis(100));
        
        // Insert first entry
        map.insert(Route::Host, 100, 1, 5);
        
        // Wait a bit, then insert second entry
        thread::sleep(Duration::from_millis(60));
        map.insert(Route::Host, 101, 2, 10);
        
        // Wait enough for first to expire but not second
        thread::sleep(Duration::from_millis(60));
        
        let cleaned = map.cleanup_expired();
        assert_eq!(cleaned, 1);  // Only first entry expired
        assert_eq!(map.len(), 1);  // Second entry still present
        
        // The remaining entry should be client 2
        let remaining = map.remove(Route::Host, 101).unwrap();
        assert_eq!(remaining.client_id, 2);
    }

    #[test]
    fn test_serial_key_equality() {
        let key1 = SerialKey::new(Route::Host, 100);
        let key2 = SerialKey::new(Route::Host, 100);
        let key3 = SerialKey::new(Route::Container, 100);
        let key4 = SerialKey::new(Route::Host, 101);
        
        assert_eq!(key1, key2);  // Same route and serial
        assert_ne!(key1, key3);  // Different route
        assert_ne!(key1, key4);  // Different serial
    }
}
