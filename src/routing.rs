//! Routing table for D-Bus message routing.
//!
//! The routing table determines whether messages should be sent to the
//! container's internal bus or the host session bus. Container services
//! always take priority over host services with the same name.

use std::collections::{HashMap, HashSet};

use tracing::{debug, trace};

/// The target bus for a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Route {
    /// Route to the container's internal D-Bus.
    Container,
    /// Route to the host session bus.
    Host,
}

impl std::fmt::Display for Route {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Route::Container => write!(f, "Container"),
            Route::Host => write!(f, "Host"),
        }
    }
}

/// Tracks unique name to route mappings.
#[derive(Debug, Default)]
struct UniqueNameRegistry {
    /// Maps unique names (":1.xxx") to their originating bus.
    names: HashMap<String, Route>,
}

impl UniqueNameRegistry {
    fn new() -> Self {
        Self::default()
    }

    fn register(&mut self, name: String, route: Route) {
        if let Some(existing_route) = self.names.get(&name) {
            if *existing_route != route {
                tracing::warn!(
                    name = %name,
                    existing_route = %existing_route,
                    new_route = %route,
                    "Unique name collision: overwriting route for existing name! \
                     This may cause message misrouting if both buses have clients \
                     with the same unique name."
                );
            } else {
                debug!(name = %name, route = %route, "Re-registering unique name with same route");
            }
        } else {
            debug!(name = %name, route = %route, "Registered unique name");
        }
        self.names.insert(name, route);
    }

    fn unregister(&mut self, name: &str) {
        if self.names.remove(name).is_some() {
            debug!(name = %name, "Unregistered unique name");
        }
    }

    fn route_for(&self, name: &str) -> Option<Route> {
        self.names.get(name).copied()
    }
}

/// Routing table that determines message destinations.
///
/// The routing priority is:
/// 1. Container-owned names (always route to container)
/// 2. Host-owned names (route to host if not owned by container)
/// 3. Host-activatable names (can be started on demand)
/// 4. Default: route to host (allows unknown services to be activated)
#[derive(Debug)]
pub struct RoutingTable {
    /// Names currently owned on the container bus.
    container_names: HashSet<String>,

    /// Names currently owned on the host bus.
    host_names: HashSet<String>,

    /// Names that can be activated (started on demand) on the host.
    host_activatable: HashSet<String>,

    /// Registry for unique name routing.
    unique_names: UniqueNameRegistry,
}

impl Default for RoutingTable {
    fn default() -> Self {
        Self::new()
    }
}

impl RoutingTable {
    /// Create a new empty routing table.
    pub fn new() -> Self {
        Self {
            container_names: HashSet::new(),
            host_names: HashSet::new(),
            host_activatable: HashSet::new(),
            unique_names: UniqueNameRegistry::new(),
        }
    }

    /// Determine the route for a destination.
    ///
    /// # Arguments
    /// * `destination` - The destination name (well-known or unique).
    ///
    /// # Returns
    /// The route to use for this destination.
    pub fn route_for(&self, destination: &str) -> Route {
        // Handle unique names (":1.xxx")
        if destination.starts_with(':') {
            return self.route_for_unique_name(destination);
        }

        // Well-known names - container takes priority
        if self.container_names.contains(destination) {
            trace!(destination = %destination, "Routing to container (owned)");
            return Route::Container;
        }

        if self.host_names.contains(destination) {
            trace!(destination = %destination, "Routing to host (owned)");
            return Route::Host;
        }

        // Activatable on host?
        if self.host_activatable.contains(destination) {
            trace!(destination = %destination, "Routing to host (activatable)");
            return Route::Host;
        }

        // Default: try host (allows unknown services to be activated)
        trace!(destination = %destination, "Routing to host (default)");
        Route::Host
    }

    /// Route a unique name to the appropriate bus.
    fn route_for_unique_name(&self, name: &str) -> Route {
        self.unique_names
            .route_for(name)
            .unwrap_or_else(|| {
                // If we don't know the unique name, default to host
                trace!(name = %name, "Unknown unique name, defaulting to host");
                Route::Host
            })
    }

    /// Register a unique name with a route.
    pub fn register_unique_name(&mut self, name: String, route: Route) {
        self.unique_names.register(name, route);
    }

    /// Unregister a unique name.
    pub fn unregister_unique_name(&mut self, name: &str) {
        self.unique_names.unregister(name);
    }

    /// Handle a NameOwnerChanged signal from the container bus.
    ///
    /// # Arguments
    /// * `name` - The well-known name that changed.
    /// * `old_owner` - The previous owner (empty if newly acquired).
    /// * `new_owner` - The new owner (empty if released).
    pub fn on_container_name_change(&mut self, name: &str, old_owner: &str, new_owner: &str) {
        if !old_owner.is_empty() {
            // Unregister old unique name mapping
            self.unique_names.unregister(old_owner);
        }

        if new_owner.is_empty() {
            // Name released on container
            if self.container_names.remove(name) {
                debug!(name = %name, "Name released on container");
                // Check if this name is now only available on host
                if self.host_names.contains(name) {
                    debug!(name = %name, "Name now routes to host (container released)");
                }
            }
        } else {
            // Name acquired on container - takes priority over host
            if self.host_names.contains(name) {
                tracing::warn!(
                    name = %name,
                    container_owner = %new_owner,
                    "Well-known name shadowing: container is acquiring a name that \
                     also exists on host. Container will take priority."
                );
            }
            self.container_names.insert(name.to_string());
            self.unique_names.register(new_owner.to_string(), Route::Container);
            debug!(name = %name, owner = %new_owner, "Name acquired on container");
        }
    }

    /// Handle a NameOwnerChanged signal from the host bus.
    ///
    /// # Arguments
    /// * `name` - The well-known name that changed.
    /// * `old_owner` - The previous owner (empty if newly acquired).
    /// * `new_owner` - The new owner (empty if released).
    pub fn on_host_name_change(&mut self, name: &str, old_owner: &str, new_owner: &str) {
        if !old_owner.is_empty() {
            self.unique_names.unregister(old_owner);
        }

        if new_owner.is_empty() {
            if self.host_names.remove(name) {
                debug!(name = %name, "Name released on host");
            }
        } else {
            self.host_names.insert(name.to_string());
            self.unique_names.register(new_owner.to_string(), Route::Host);
            if self.container_names.contains(name) {
                tracing::warn!(
                    name = %name,
                    host_owner = %new_owner,
                    "Well-known name exists on both buses: host acquired a name that \
                     container already owns. Messages will route to container."
                );
            }
            debug!(name = %name, owner = %new_owner, "Name acquired on host");
        }
    }

    /// Set the list of currently owned names on the container bus.
    pub fn set_container_names(&mut self, names: impl IntoIterator<Item = String>) {
        self.container_names = names.into_iter().collect();
        debug!(count = self.container_names.len(), "Set container names");
    }

    /// Set the list of currently owned names on the host bus.
    pub fn set_host_names(&mut self, names: impl IntoIterator<Item = String>) {
        self.host_names = names.into_iter().collect();
        debug!(count = self.host_names.len(), "Set host names");
    }

    /// Set the list of activatable names on the host bus.
    pub fn set_host_activatable(&mut self, names: impl IntoIterator<Item = String>) {
        self.host_activatable = names.into_iter().collect();
        debug!(count = self.host_activatable.len(), "Set host activatable names");
    }

    /// Get all names visible to clients (merged from both buses).
    ///
    /// Container names take priority, so duplicates show the container version.
    pub fn all_names(&self) -> Vec<String> {
        let mut names: HashSet<String> = self.container_names.clone();
        names.extend(self.host_names.iter().cloned());
        names.into_iter().collect()
    }

    /// Check if a name has an owner on either bus.
    ///
    /// Container takes priority.
    pub fn name_has_owner(&self, name: &str) -> bool {
        self.container_names.contains(name) || self.host_names.contains(name)
    }

    /// Get the owner of a name, preferring container.
    ///
    /// Returns the route where the name is owned.
    pub fn get_name_owner_route(&self, name: &str) -> Option<Route> {
        if self.container_names.contains(name) {
            Some(Route::Container)
        } else if self.host_names.contains(name) {
            Some(Route::Host)
        } else {
            None
        }
    }

    /// Check if a name is owned on the container bus.
    pub fn is_container_name(&self, name: &str) -> bool {
        self.container_names.contains(name)
    }

    /// Check if a name is owned on the host bus.
    pub fn is_host_name(&self, name: &str) -> bool {
        self.host_names.contains(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_container_priority() {
        let mut table = RoutingTable::new();
        
        // Add name to both buses
        table.on_host_name_change("org.example.Service", "", ":1.100");
        table.on_container_name_change("org.example.Service", "", ":1.50");
        
        // Container should take priority
        assert_eq!(table.route_for("org.example.Service"), Route::Container);
    }

    #[test]
    fn test_host_fallback() {
        let mut table = RoutingTable::new();
        
        // Add name only to host
        table.on_host_name_change("org.example.HostOnly", "", ":1.100");
        
        assert_eq!(table.route_for("org.example.HostOnly"), Route::Host);
    }

    #[test]
    fn test_unknown_defaults_to_host() {
        let table = RoutingTable::new();
        
        // Unknown names should route to host (for activation)
        assert_eq!(table.route_for("org.example.Unknown"), Route::Host);
    }

    #[test]
    fn test_unique_name_routing() {
        let mut table = RoutingTable::new();
        
        table.register_unique_name(":1.50".to_string(), Route::Container);
        table.register_unique_name(":1.100".to_string(), Route::Host);
        
        assert_eq!(table.route_for(":1.50"), Route::Container);
        assert_eq!(table.route_for(":1.100"), Route::Host);
    }

    #[test]
    fn test_name_release() {
        let mut table = RoutingTable::new();
        
        table.on_container_name_change("org.example.Service", "", ":1.50");
        assert_eq!(table.route_for("org.example.Service"), Route::Container);
        
        table.on_container_name_change("org.example.Service", ":1.50", "");
        // After release, unknown names go to host
        assert_eq!(table.route_for("org.example.Service"), Route::Host);
    }
}
