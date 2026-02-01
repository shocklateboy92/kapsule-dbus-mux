//! Name queue management for D-Bus name ownership.
//!
//! The D-Bus specification supports name ownership queuing, where multiple clients
//! can request the same name and be placed in a queue. When the primary owner releases
//! the name, ownership passes to the next client in the queue.
//!
//! This module tracks name ownership queues for clients of the mux, allowing
//! proper handling of RequestName flags and ListQueuedOwners queries.

use std::collections::{HashMap, VecDeque};

use crate::serial_map::ClientId;

/// Request name flags (from D-Bus spec).
pub mod request_name_flags {
    /// Allow replacement of this owner by another connection.
    pub const ALLOW_REPLACEMENT: u32 = 0x1;
    /// Attempt to replace the existing owner if possible.
    pub const REPLACE_EXISTING: u32 = 0x2;
    /// Don't queue if the name is already owned - return immediately.
    pub const DO_NOT_QUEUE: u32 = 0x4;
}

/// Request name reply codes (from D-Bus spec).
pub mod request_name_reply {
    /// Caller is now the primary owner of the name.
    pub const PRIMARY_OWNER: u32 = 1;
    /// Caller is in queue waiting for the name.
    pub const IN_QUEUE: u32 = 2;
    /// Name is already owned and DO_NOT_QUEUE was specified.
    pub const EXISTS: u32 = 3;
    /// Caller was already the primary owner of the name.
    pub const ALREADY_OWNER: u32 = 4;
}

/// Release name reply codes (from D-Bus spec).
pub mod release_name_reply {
    /// Caller has released the name successfully.
    pub const RELEASED: u32 = 1;
    /// The name does not exist (was not owned by anyone).
    pub const NON_EXISTENT: u32 = 2;
    /// The caller is not the owner of this name.
    pub const NOT_OWNER: u32 = 3;
}

/// Entry in the name ownership queue.
#[derive(Debug, Clone)]
struct QueueEntry {
    /// The client that requested this name.
    client_id: ClientId,
    /// The unique name of the client (":mux.N").
    unique_name: String,
    /// Whether this client allows being replaced.
    allow_replacement: bool,
}

/// Queue for a single name, tracking ownership and waiters.
#[derive(Debug, Default)]
struct NameQueue {
    /// The current primary owner (if any).
    primary_owner: Option<QueueEntry>,
    /// Clients waiting in queue for ownership (in order).
    waiters: VecDeque<QueueEntry>,
}

impl NameQueue {
    /// Get all owners/waiters as unique names, primary first.
    fn all_owners(&self) -> Vec<String> {
        let mut result = Vec::new();
        if let Some(ref owner) = self.primary_owner {
            result.push(owner.unique_name.clone());
        }
        for waiter in &self.waiters {
            result.push(waiter.unique_name.clone());
        }
        result
    }

    /// Check if a client is in the queue (either as owner or waiter).
    fn contains_client(&self, client_id: ClientId) -> bool {
        if let Some(ref owner) = self.primary_owner {
            if owner.client_id == client_id {
                return true;
            }
        }
        self.waiters.iter().any(|e| e.client_id == client_id)
    }

    /// Check if a client is the primary owner.
    fn is_primary_owner(&self, client_id: ClientId) -> bool {
        self.primary_owner
            .as_ref()
            .is_some_and(|o| o.client_id == client_id)
    }
}

/// Manages name ownership queues for all names.
#[derive(Debug, Default)]
pub struct NameQueueManager {
    /// Queue for each well-known name.
    queues: HashMap<String, NameQueue>,
}

/// Result of a RequestName operation.
#[derive(Debug, Clone)]
pub struct RequestNameResult {
    /// The reply code to return to the client.
    pub reply_code: u32,
    /// Whether to forward the request to the upstream bus.
    /// Only true when the client becomes the primary owner.
    pub forward_to_bus: bool,
    /// The previous owner's unique name (if a replacement occurred).
    pub replaced_owner: Option<String>,
}

/// Result of a ReleaseName operation.
#[derive(Debug, Clone)]
pub struct ReleaseNameResult {
    /// The reply code to return to the client.
    pub reply_code: u32,
    /// Whether to forward the release to the upstream bus.
    /// Only true when the actual primary owner releases.
    pub forward_to_bus: bool,
    /// The new primary owner (if any waiter was promoted).
    pub new_owner: Option<(ClientId, String)>,
}

impl NameQueueManager {
    /// Create a new name queue manager.
    pub fn new() -> Self {
        Self::default()
    }

    /// Request a name for a client.
    ///
    /// # Arguments
    /// * `name` - The well-known name being requested.
    /// * `client_id` - The client requesting the name.
    /// * `unique_name` - The client's unique name (":mux.N").
    /// * `flags` - RequestName flags.
    ///
    /// # Returns
    /// The result indicating what reply code to use and whether to forward.
    pub fn request_name(
        &mut self,
        name: &str,
        client_id: ClientId,
        unique_name: &str,
        flags: u32,
    ) -> RequestNameResult {
        let allow_replacement = (flags & request_name_flags::ALLOW_REPLACEMENT) != 0;
        let replace_existing = (flags & request_name_flags::REPLACE_EXISTING) != 0;
        let do_not_queue = (flags & request_name_flags::DO_NOT_QUEUE) != 0;

        let queue = self.queues.entry(name.to_string()).or_default();

        // Check if this client already owns or is queued for this name
        if queue.is_primary_owner(client_id) {
            return RequestNameResult {
                reply_code: request_name_reply::ALREADY_OWNER,
                forward_to_bus: false,
                replaced_owner: None,
            };
        }

        if queue.contains_client(client_id) {
            // Already in queue, just return IN_QUEUE
            return RequestNameResult {
                reply_code: request_name_reply::IN_QUEUE,
                forward_to_bus: false,
                replaced_owner: None,
            };
        }

        let entry = QueueEntry {
            client_id,
            unique_name: unique_name.to_string(),
            allow_replacement,
        };

        // If no current owner, this client becomes the primary owner
        if queue.primary_owner.is_none() {
            queue.primary_owner = Some(entry);
            return RequestNameResult {
                reply_code: request_name_reply::PRIMARY_OWNER,
                forward_to_bus: true,
                replaced_owner: None,
            };
        }

        // There's a current owner - check if we can replace them
        let current_owner = queue.primary_owner.as_ref().unwrap();
        if replace_existing && current_owner.allow_replacement {
            // Replace the current owner
            let old_owner_name = current_owner.unique_name.clone();
            let old_owner = queue.primary_owner.take().unwrap();

            // Push the old owner to the front of the waiter queue
            queue.waiters.push_front(old_owner);
            queue.primary_owner = Some(entry);

            return RequestNameResult {
                reply_code: request_name_reply::PRIMARY_OWNER,
                forward_to_bus: true,
                replaced_owner: Some(old_owner_name),
            };
        }

        // Can't replace - either queue up or return EXISTS
        if do_not_queue {
            return RequestNameResult {
                reply_code: request_name_reply::EXISTS,
                forward_to_bus: false,
                replaced_owner: None,
            };
        }

        // Add to wait queue
        queue.waiters.push_back(entry);
        RequestNameResult {
            reply_code: request_name_reply::IN_QUEUE,
            forward_to_bus: false,
            replaced_owner: None,
        }
    }

    /// Release a name for a client.
    ///
    /// # Arguments
    /// * `name` - The well-known name being released.
    /// * `client_id` - The client releasing the name.
    ///
    /// # Returns
    /// The result indicating what reply code to use and whether to forward.
    pub fn release_name(&mut self, name: &str, client_id: ClientId) -> ReleaseNameResult {
        let queue = match self.queues.get_mut(name) {
            Some(q) => q,
            None => {
                return ReleaseNameResult {
                    reply_code: release_name_reply::NON_EXISTENT,
                    forward_to_bus: false,
                    new_owner: None,
                };
            }
        };

        // Check if client is the primary owner
        if queue.is_primary_owner(client_id) {
            queue.primary_owner = None;

            // Promote next waiter if any
            let new_owner = if let Some(next) = queue.waiters.pop_front() {
                let owner_info = (next.client_id, next.unique_name.clone());
                queue.primary_owner = Some(next);
                Some(owner_info)
            } else {
                // No waiters, clean up the queue
                self.queues.remove(name);
                None
            };

            return ReleaseNameResult {
                reply_code: release_name_reply::RELEASED,
                forward_to_bus: new_owner.is_none(), // Only forward if no new owner
                new_owner,
            };
        }

        // Check if client is in the wait queue
        let initial_len = queue.waiters.len();
        queue.waiters.retain(|e| e.client_id != client_id);

        if queue.waiters.len() < initial_len {
            // Client was in queue and has been removed
            return ReleaseNameResult {
                reply_code: release_name_reply::RELEASED,
                forward_to_bus: false,
                new_owner: None,
            };
        }

        // Client doesn't own or queue for this name
        ReleaseNameResult {
            reply_code: release_name_reply::NOT_OWNER,
            forward_to_bus: false,
            new_owner: None,
        }
    }

    /// Get the list of queued owners for a name (primary owner first).
    pub fn list_queued_owners(&self, name: &str) -> Vec<String> {
        self.queues
            .get(name)
            .map(|q| q.all_owners())
            .unwrap_or_default()
    }

    /// Remove a client from all queues (for client disconnect cleanup).
    ///
    /// # Returns
    /// A list of (name, new_owner) tuples where new_owner is Some if a waiter
    /// was promoted due to this client disconnecting as primary owner.
    pub fn remove_client(&mut self, client_id: ClientId) -> Vec<(String, Option<(ClientId, String)>)> {
        let mut results = Vec::new();
        let mut empty_queues = Vec::new();

        for (name, queue) in &mut self.queues {
            // Check if this client was the primary owner
            if queue.is_primary_owner(client_id) {
                queue.primary_owner = None;

                // Promote next waiter
                let new_owner = if let Some(next) = queue.waiters.pop_front() {
                    let info = (next.client_id, next.unique_name.clone());
                    queue.primary_owner = Some(next);
                    Some(info)
                } else {
                    empty_queues.push(name.clone());
                    None
                };

                results.push((name.clone(), new_owner));
            } else {
                // Remove from wait queue
                queue.waiters.retain(|e| e.client_id != client_id);
                if queue.primary_owner.is_none() && queue.waiters.is_empty() {
                    empty_queues.push(name.clone());
                }
            }
        }

        // Clean up empty queues
        for name in empty_queues {
            self.queues.remove(&name);
        }

        results
    }

    /// Check if a name has any owner (primary or waiting).
    pub fn name_has_owner(&self, name: &str) -> bool {
        self.queues.get(name).is_some_and(|q| q.primary_owner.is_some())
    }

    /// Get the primary owner of a name.
    pub fn get_primary_owner(&self, name: &str) -> Option<(ClientId, &str)> {
        self.queues.get(name).and_then(|q| {
            q.primary_owner
                .as_ref()
                .map(|o| (o.client_id, o.unique_name.as_str()))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_first_request_becomes_owner() {
        let mut mgr = NameQueueManager::new();
        let result = mgr.request_name("org.example.Test", 1, ":mux.1", 0);

        assert_eq!(result.reply_code, request_name_reply::PRIMARY_OWNER);
        assert!(result.forward_to_bus);
        assert!(result.replaced_owner.is_none());
    }

    #[test]
    fn test_already_owner() {
        let mut mgr = NameQueueManager::new();
        mgr.request_name("org.example.Test", 1, ":mux.1", 0);

        let result = mgr.request_name("org.example.Test", 1, ":mux.1", 0);
        assert_eq!(result.reply_code, request_name_reply::ALREADY_OWNER);
        assert!(!result.forward_to_bus);
    }

    #[test]
    fn test_queue_without_do_not_queue() {
        let mut mgr = NameQueueManager::new();
        mgr.request_name("org.example.Test", 1, ":mux.1", 0);

        let result = mgr.request_name("org.example.Test", 2, ":mux.2", 0);
        assert_eq!(result.reply_code, request_name_reply::IN_QUEUE);
        assert!(!result.forward_to_bus);
    }

    #[test]
    fn test_do_not_queue_flag() {
        let mut mgr = NameQueueManager::new();
        mgr.request_name("org.example.Test", 1, ":mux.1", 0);

        let result = mgr.request_name(
            "org.example.Test",
            2,
            ":mux.2",
            request_name_flags::DO_NOT_QUEUE,
        );
        assert_eq!(result.reply_code, request_name_reply::EXISTS);
        assert!(!result.forward_to_bus);
    }

    #[test]
    fn test_replacement_with_allow_replacement() {
        let mut mgr = NameQueueManager::new();
        // First owner allows replacement
        mgr.request_name(
            "org.example.Test",
            1,
            ":mux.1",
            request_name_flags::ALLOW_REPLACEMENT,
        );

        // Second client requests with REPLACE_EXISTING
        let result = mgr.request_name(
            "org.example.Test",
            2,
            ":mux.2",
            request_name_flags::REPLACE_EXISTING,
        );

        assert_eq!(result.reply_code, request_name_reply::PRIMARY_OWNER);
        assert!(result.forward_to_bus);
        assert_eq!(result.replaced_owner, Some(":mux.1".to_string()));

        // Old owner should now be in queue
        let owners = mgr.list_queued_owners("org.example.Test");
        assert_eq!(owners, vec![":mux.2", ":mux.1"]);
    }

    #[test]
    fn test_replacement_denied_without_allow() {
        let mut mgr = NameQueueManager::new();
        // First owner does NOT allow replacement
        mgr.request_name("org.example.Test", 1, ":mux.1", 0);

        // Second client tries to replace
        let result = mgr.request_name(
            "org.example.Test",
            2,
            ":mux.2",
            request_name_flags::REPLACE_EXISTING,
        );

        // Should just queue instead of replacing
        assert_eq!(result.reply_code, request_name_reply::IN_QUEUE);
        assert!(!result.forward_to_bus);
        assert!(result.replaced_owner.is_none());
    }

    #[test]
    fn test_release_by_owner_promotes_waiter() {
        let mut mgr = NameQueueManager::new();
        mgr.request_name("org.example.Test", 1, ":mux.1", 0);
        mgr.request_name("org.example.Test", 2, ":mux.2", 0);

        let result = mgr.release_name("org.example.Test", 1);

        assert_eq!(result.reply_code, release_name_reply::RELEASED);
        assert!(!result.forward_to_bus); // New owner takes over, don't forward release
        assert_eq!(result.new_owner, Some((2, ":mux.2".to_string())));

        // Verify new owner is now primary
        let owners = mgr.list_queued_owners("org.example.Test");
        assert_eq!(owners, vec![":mux.2"]);
    }

    #[test]
    fn test_release_last_owner() {
        let mut mgr = NameQueueManager::new();
        mgr.request_name("org.example.Test", 1, ":mux.1", 0);

        let result = mgr.release_name("org.example.Test", 1);

        assert_eq!(result.reply_code, release_name_reply::RELEASED);
        assert!(result.forward_to_bus); // No waiters, forward release to bus
        assert!(result.new_owner.is_none());
    }

    #[test]
    fn test_release_non_owner() {
        let mut mgr = NameQueueManager::new();
        mgr.request_name("org.example.Test", 1, ":mux.1", 0);

        let result = mgr.release_name("org.example.Test", 3);

        assert_eq!(result.reply_code, release_name_reply::NOT_OWNER);
        assert!(!result.forward_to_bus);
    }

    #[test]
    fn test_release_nonexistent() {
        let mut mgr = NameQueueManager::new();

        let result = mgr.release_name("org.example.Unknown", 1);

        assert_eq!(result.reply_code, release_name_reply::NON_EXISTENT);
        assert!(!result.forward_to_bus);
    }

    #[test]
    fn test_release_from_queue() {
        let mut mgr = NameQueueManager::new();
        mgr.request_name("org.example.Test", 1, ":mux.1", 0);
        mgr.request_name("org.example.Test", 2, ":mux.2", 0);
        mgr.request_name("org.example.Test", 3, ":mux.3", 0);

        // Release client 2 who is in queue
        let result = mgr.release_name("org.example.Test", 2);

        assert_eq!(result.reply_code, release_name_reply::RELEASED);
        assert!(!result.forward_to_bus);
        assert!(result.new_owner.is_none());

        // Verify queue state
        let owners = mgr.list_queued_owners("org.example.Test");
        assert_eq!(owners, vec![":mux.1", ":mux.3"]);
    }

    #[test]
    fn test_list_queued_owners() {
        let mut mgr = NameQueueManager::new();
        mgr.request_name("org.example.Test", 1, ":mux.1", 0);
        mgr.request_name("org.example.Test", 2, ":mux.2", 0);
        mgr.request_name("org.example.Test", 3, ":mux.3", 0);

        let owners = mgr.list_queued_owners("org.example.Test");
        assert_eq!(owners, vec![":mux.1", ":mux.2", ":mux.3"]);
    }

    #[test]
    fn test_list_queued_owners_empty() {
        let mgr = NameQueueManager::new();
        let owners = mgr.list_queued_owners("org.example.Unknown");
        assert!(owners.is_empty());
    }

    #[test]
    fn test_remove_client_primary_owner() {
        let mut mgr = NameQueueManager::new();
        mgr.request_name("org.example.Test", 1, ":mux.1", 0);
        mgr.request_name("org.example.Test", 2, ":mux.2", 0);

        let results = mgr.remove_client(1);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "org.example.Test");
        assert_eq!(results[0].1, Some((2, ":mux.2".to_string())));

        // Verify new owner
        let owners = mgr.list_queued_owners("org.example.Test");
        assert_eq!(owners, vec![":mux.2"]);
    }

    #[test]
    fn test_remove_client_from_queue() {
        let mut mgr = NameQueueManager::new();
        mgr.request_name("org.example.Test", 1, ":mux.1", 0);
        mgr.request_name("org.example.Test", 2, ":mux.2", 0);

        let results = mgr.remove_client(2);

        // No name ownership changed
        assert!(results.is_empty());

        // Verify queue state
        let owners = mgr.list_queued_owners("org.example.Test");
        assert_eq!(owners, vec![":mux.1"]);
    }

    #[test]
    fn test_multiple_names_per_client() {
        let mut mgr = NameQueueManager::new();
        mgr.request_name("org.example.Test1", 1, ":mux.1", 0);
        mgr.request_name("org.example.Test2", 1, ":mux.1", 0);
        mgr.request_name("org.example.Test1", 2, ":mux.2", 0);

        let results = mgr.remove_client(1);

        // Both names affected
        assert_eq!(results.len(), 2);

        // Test1 has a new owner, Test2 has no owner
        let test1_result = results.iter().find(|(n, _)| n == "org.example.Test1");
        let test2_result = results.iter().find(|(n, _)| n == "org.example.Test2");

        assert!(test1_result.is_some());
        assert!(test2_result.is_some());
        assert_eq!(test1_result.unwrap().1, Some((2, ":mux.2".to_string())));
        assert!(test2_result.unwrap().1.is_none());
    }
}
