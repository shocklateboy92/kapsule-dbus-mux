//! Message routing logic.
//!
//! This module handles routing D-Bus messages between clients and the
//! container/host buses, including serial number correlation for replies.

use std::sync::Arc;

use tokio::sync::RwLock;
use tracing::{debug, trace};
use zbus::message::Message;

use crate::bus_connection::BusConnection;
use crate::error::{MessageError, Result};
use crate::message::MessageExt;
use crate::routing::{Route, RoutingTable};
use crate::serial_map::{ClientId, SerialMap};

/// Routes messages between clients and buses.
pub struct MessageRouter {
    /// Connection to the container's internal bus.
    container_conn: Arc<BusConnection>,
    /// Connection to the host session bus.
    host_conn: Arc<BusConnection>,
    /// Routing table.
    routing_table: Arc<RwLock<RoutingTable>>,
    /// Serial number mapping for reply correlation.
    serial_map: Arc<RwLock<SerialMap>>,
}

impl MessageRouter {
    /// Create a new message router.
    pub fn new(
        container_conn: Arc<BusConnection>,
        host_conn: Arc<BusConnection>,
        routing_table: Arc<RwLock<RoutingTable>>,
    ) -> Self {
        Self {
            container_conn,
            host_conn,
            routing_table,
            serial_map: Arc::new(RwLock::new(SerialMap::new())),
        }
    }

    /// Get the routing table.
    pub fn routing_table(&self) -> &Arc<RwLock<RoutingTable>> {
        &self.routing_table
    }

    /// Get the serial map.
    pub fn serial_map(&self) -> &Arc<RwLock<SerialMap>> {
        &self.serial_map
    }

    /// Determine the route for a destination.
    pub async fn route_for(&self, destination: &str) -> Route {
        self.routing_table.read().await.route_for(destination)
    }

    /// Get the connection for a route.
    pub fn connection_for(&self, route: Route) -> &Arc<BusConnection> {
        match route {
            Route::Container => &self.container_conn,
            Route::Host => &self.host_conn,
        }
    }

    /// Forward a method call to the appropriate bus.
    ///
    /// Records the serial mapping for reply correlation.
    pub async fn forward_method_call(
        &self,
        client_id: ClientId,
        msg: &Message,
        route: Route,
    ) -> Result<()> {
        let conn = self.connection_for(route);
        let client_serial = msg.serial();
        let forwarded_serial = conn.next_serial();

        // Record mapping for reply correlation
        self.serial_map.write().await.insert(
            route,
            forwarded_serial,
            client_id,
            client_serial,
        );

        // Forward the message
        // Note: We need to create a new message with the new serial
        // For now, we just send the original message - zbus will assign its own serial
        conn.send(msg).await?;

        trace!(
            client_id = client_id,
            client_serial = client_serial,
            forwarded_serial = forwarded_serial,
            route = %route,
            "Forwarded method call"
        );

        Ok(())
    }

    /// Forward a signal to clients.
    ///
    /// Signals are typically broadcast and don't need reply correlation.
    pub async fn forward_signal(&self, msg: &Message, route: Route) -> Result<()> {
        // For now, just log the signal
        // In a full implementation, we'd track which clients are interested
        // in which signals (via AddMatch) and forward accordingly
        trace!(
            interface = ?msg.interface_str(),
            member = ?msg.member_str(),
            route = %route,
            "Received signal"
        );
        Ok(())
    }

    /// Handle a reply from one of the buses.
    ///
    /// Returns the client ID and modified message if we have a pending call.
    pub async fn handle_reply(
        &self,
        msg: &Message,
        route: Route,
    ) -> Result<Option<(ClientId, u32)>> {
        let reply_serial = msg.reply_serial()
            .ok_or(MessageError::NoReplySerial)?;

        // Look up the original client
        let pending = self.serial_map.write().await.remove(route, reply_serial);

        if let Some(pending) = pending {
            trace!(
                client_id = pending.client_id,
                client_serial = pending.client_serial,
                reply_serial = reply_serial,
                route = %route,
                "Correlated reply"
            );
            Ok(Some((pending.client_id, pending.client_serial)))
        } else {
            // No pending call found - might be a stale reply or for a different client
            trace!(
                reply_serial = reply_serial,
                route = %route,
                "No pending call for reply"
            );
            Ok(None)
        }
    }

    /// Clean up expired pending calls.
    pub async fn cleanup_expired(&self) -> usize {
        let count = self.serial_map.write().await.cleanup_expired();
        if count > 0 {
            debug!(count = count, "Cleaned up expired pending calls");
        }
        count
    }
}

/// Information about a message and where it should go.
#[derive(Debug)]
pub struct RoutedMessage {
    /// The message to send.
    pub message: Arc<Message>,
    /// Where to send it.
    pub route: Route,
    /// The client that sent this message (for reply correlation).
    pub client_id: ClientId,
    /// The original serial number from the client.
    pub client_serial: u32,
}

impl RoutedMessage {
    /// Create a new routed message.
    pub fn new(
        message: Arc<Message>,
        route: Route,
        client_id: ClientId,
        client_serial: u32,
    ) -> Self {
        Self {
            message,
            route,
            client_id,
            client_serial,
        }
    }
}
