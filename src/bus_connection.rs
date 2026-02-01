//! D-Bus connection management using zbus.
//!
//! This module wraps zbus connections to provide a consistent interface
//! for both container and host bus connections.

use std::sync::Arc;

use tokio::sync::Mutex;
use tracing::{debug, info, trace};
use zbus::connection::Builder as ConnectionBuilder;
use zbus::{Connection, Message, MessageStream};

use crate::error::Result;
use crate::serial_map::SerialGenerator;

/// A connection to a D-Bus bus.
#[derive(Debug)]
pub struct BusConnection {
    /// The underlying zbus connection.
    connection: Connection,
    /// Message stream for receiving messages.
    stream: Arc<Mutex<MessageStream>>,
    /// Serial number generator.
    serial_gen: SerialGenerator,
    /// Human-readable name for logging.
    name: String,
}

impl BusConnection {
    /// Connect to a D-Bus bus at the given address.
    ///
    /// # Arguments
    /// * `address` - The D-Bus address (e.g., "unix:path=/path/to/socket").
    /// * `name` - A human-readable name for logging.
    pub async fn connect(address: &str, name: impl Into<String>) -> Result<Self> {
        let name = name.into();
        info!(address = %address, name = %name, "Connecting to D-Bus");
        
        let connection = ConnectionBuilder::address(address)?
            .build()
            .await?;
        
        let stream = MessageStream::from(&connection);
        
        debug!(
            name = %name,
            unique_name = ?connection.unique_name(),
            "Connected to D-Bus"
        );
        
        Ok(Self {
            connection,
            stream: Arc::new(Mutex::new(stream)),
            serial_gen: SerialGenerator::new(),
            name,
        })
    }

    /// Connect to the session bus.
    ///
    /// # Arguments
    /// * `address` - Optional override for the session bus address.
    ///   If None, uses DBUS_SESSION_BUS_ADDRESS env var.
    /// * `name` - A human-readable name for logging.
    pub async fn connect_session(address: Option<&str>, name: impl Into<String>) -> Result<Self> {
        let name = name.into();
        
        let connection = if let Some(addr) = address {
            info!(address = %addr, name = %name, "Connecting to session bus at custom address");
            ConnectionBuilder::address(addr)?
                .build()
                .await?
        } else {
            info!(name = %name, "Connecting to default session bus");
            Connection::session().await?
        };
        
        let stream = MessageStream::from(&connection);
        
        debug!(
            name = %name,
            unique_name = ?connection.unique_name(),
            "Connected to session bus"
        );
        
        Ok(Self {
            connection,
            stream: Arc::new(Mutex::new(stream)),
            serial_gen: SerialGenerator::new(),
            name,
        })
    }

    /// Get the connection's unique name on the bus.
    pub fn unique_name(&self) -> Option<&str> {
        self.connection.unique_name().map(|n| n.as_str())
    }

    /// Get a reference to the underlying connection.
    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    /// Get the next serial number for this connection.
    pub fn next_serial(&self) -> u32 {
        self.serial_gen.next()
    }

    /// Subscribe to NameOwnerChanged signals.
    pub async fn subscribe_name_changes(&self) -> Result<()> {
        // Use AddMatch via the D-Bus daemon
        self.connection.call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "AddMatch",
            &("type='signal',sender='org.freedesktop.DBus',interface='org.freedesktop.DBus',member='NameOwnerChanged'",),
        ).await?;
        
        debug!(bus = %self.name, "Subscribed to NameOwnerChanged");
        Ok(())
    }

    /// List all currently owned names on this bus.
    pub async fn list_names(&self) -> Result<Vec<String>> {
        let reply: Vec<String> = self.connection
            .call_method(
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                Some("org.freedesktop.DBus"),
                "ListNames",
                &(),
            )
            .await?
            .body()
            .deserialize()?;
        
        trace!(bus = %self.name, count = reply.len(), "Listed names");
        Ok(reply)
    }

    /// List all activatable (but not necessarily running) names.
    pub async fn list_activatable_names(&self) -> Result<Vec<String>> {
        let reply: Vec<String> = self.connection
            .call_method(
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                Some("org.freedesktop.DBus"),
                "ListActivatableNames",
                &(),
            )
            .await?
            .body()
            .deserialize()?;
        
        trace!(bus = %self.name, count = reply.len(), "Listed activatable names");
        Ok(reply)
    }

    /// Get the owner of a specific name.
    pub async fn get_name_owner(&self, name: &str) -> Result<Option<String>> {
        let result = self.connection
            .call_method(
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                Some("org.freedesktop.DBus"),
                "GetNameOwner",
                &(name,),
            )
            .await;
        
        match result {
            Ok(reply) => {
                let owner: String = reply.body().deserialize()?;
                Ok(Some(owner))
            }
            Err(zbus::Error::MethodError(name, _, _)) 
                if name.as_str() == "org.freedesktop.DBus.Error.NameHasNoOwner" => {
                Ok(None)
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Check if a name has an owner.
    pub async fn name_has_owner(&self, name: &str) -> Result<bool> {
        let reply: bool = self.connection
            .call_method(
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                Some("org.freedesktop.DBus"),
                "NameHasOwner",
                &(name,),
            )
            .await?
            .body()
            .deserialize()?;
        
        Ok(reply)
    }

    /// Send a message on this connection.
    pub async fn send(&self, msg: &Message) -> Result<()> {
        self.connection.send(msg).await?;
        trace!(
            bus = %self.name,
            serial = msg.primary_header().serial_num(),
            "Sent message"
        );
        Ok(())
    }

    /// Receive the next message from this connection.
    pub async fn recv(&self) -> Option<Arc<Message>> {
        use futures::StreamExt;
        let mut stream = self.stream.lock().await;
        match stream.next().await {
            Some(Ok(msg)) => Some(Arc::new(msg)),
            Some(Err(e)) => {
                tracing::error!(error = %e, "Error receiving message");
                None
            }
            None => None,
        }
    }

    /// Get the human-readable name of this connection.
    pub fn name(&self) -> &str {
        &self.name
    }
}
