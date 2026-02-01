//! Core multiplexer implementation.
//!
//! The multiplexer accepts client connections and routes messages between
//! the container's internal bus and the host session bus.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, error, info, trace, warn};
use zbus::message::Message;

use crate::auth::SaslAuth;
use crate::bus_connection::BusConnection;
use crate::client::ClientHandler;
use crate::error::Result;
use crate::match_rules::{ClientMatchRules, MatchRule};
use crate::message::{
    clone_reply_with_serial, parse_name_owner_changed, ErrorBuilder, MessageExt, 
    MethodReturnBuilder, error_names,
};
use crate::router::MessageRouter;
use crate::routing::{Route, RoutingTable};
use crate::serial_map::ClientId;

/// D-Bus request name flags.
#[allow(dead_code)]
mod request_name_flags {
    pub const ALLOW_REPLACEMENT: u32 = 0x1;
    pub const REPLACE_EXISTING: u32 = 0x2;
    pub const DO_NOT_QUEUE: u32 = 0x4;
}

/// D-Bus request name reply codes.
#[allow(dead_code)]
mod request_name_reply {
    pub const PRIMARY_OWNER: u32 = 1;
    pub const IN_QUEUE: u32 = 2;
    pub const EXISTS: u32 = 3;
    pub const ALREADY_OWNER: u32 = 4;
}

/// D-Bus release name reply codes.
#[allow(dead_code)]
mod release_name_reply {
    pub const RELEASED: u32 = 1;
    pub const NON_EXISTENT: u32 = 2;
    pub const NOT_OWNER: u32 = 3;
}

/// The D-Bus multiplexer.
pub struct Multiplexer {
    /// Socket we listen on for client connections.
    listener: UnixListener,
    /// Path to the listener socket (for cleanup).
    listen_path: PathBuf,
    /// Connection to the container's internal dbus-daemon.
    container_conn: Arc<BusConnection>,
    /// Connection to the host session bus.
    host_conn: Arc<BusConnection>,
    /// Routing table: name -> Route.
    routing_table: Arc<RwLock<RoutingTable>>,
    /// Active client connections.
    clients: Arc<RwLock<HashMap<ClientId, ClientInfo>>>,
    /// Message router.
    router: Arc<MessageRouter>,
    /// SASL authentication handler.
    auth: SaslAuth,
    /// Channel for client messages.
    client_msg_tx: mpsc::Sender<(ClientId, Message)>,
    /// Channel receiver for client messages.
    client_msg_rx: mpsc::Receiver<(ClientId, Message)>,
}

/// Information about a connected client.
struct ClientInfo {
    /// The client's unique name.
    unique_name: String,
    /// Channel to send messages to this client.
    tx: mpsc::Sender<Arc<Message>>,
    /// Names owned by this client.
    owned_names: std::collections::HashSet<String>,
    /// Match rules for signal filtering.
    match_rules: ClientMatchRules,
    /// Client credentials (UID, PID, GID) obtained from peer_cred.
    credentials: ClientCredentials,
}

/// Unix credentials for a connected client.
#[derive(Debug, Clone, Copy)]
struct ClientCredentials {
    /// User ID of the client process.
    uid: u32,
    /// Process ID of the client process.
    pid: Option<u32>,
    /// Group ID of the client process.
    gid: u32,
}

impl Multiplexer {
    /// Create a new multiplexer.
    ///
    /// # Arguments
    /// * `listen_path` - Path for the client socket.
    /// * `container_bus` - Address of the container's internal bus.
    /// * `host_bus` - Optional override for the host session bus address.
    pub async fn new(
        listen_path: PathBuf,
        container_bus: &str,
        host_bus: Option<&str>,
    ) -> Result<Self> {
        // Remove old socket if it exists
        if listen_path.exists() {
            std::fs::remove_file(&listen_path)?;
        }

        // Create parent directory if needed
        if let Some(parent) = listen_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Create listener socket
        let listener = UnixListener::bind(&listen_path)?;
        info!(path = %listen_path.display(), "Listening for client connections");

        // Connect to both buses
        let container_conn = Arc::new(
            BusConnection::connect(container_bus, "container").await?
        );
        let host_conn = Arc::new(
            BusConnection::connect_session(host_bus, "host").await?
        );

        // Initialize routing table
        let routing_table = Arc::new(RwLock::new(RoutingTable::new()));

        // Sync routing table from current bus state
        Self::sync_routing_table(&routing_table, &container_conn, &host_conn).await?;

        // Subscribe to NameOwnerChanged on both buses
        container_conn.subscribe_name_changes().await?;
        host_conn.subscribe_name_changes().await?;

        // Create message router
        let router = Arc::new(MessageRouter::new(
            container_conn.clone(),
            host_conn.clone(),
            routing_table.clone(),
        ));

        // Create client message channel
        let (client_msg_tx, client_msg_rx) = mpsc::channel(1024);

        Ok(Self {
            listener,
            listen_path,
            container_conn,
            host_conn,
            routing_table,
            clients: Arc::new(RwLock::new(HashMap::new())),
            router,
            auth: SaslAuth::new(),
            client_msg_tx,
            client_msg_rx,
        })
    }

    /// Sync the routing table from current bus state.
    async fn sync_routing_table(
        routing_table: &Arc<RwLock<RoutingTable>>,
        container_conn: &BusConnection,
        host_conn: &BusConnection,
    ) -> Result<()> {
        let mut table = routing_table.write().await;

        // Get container names
        let container_names = container_conn.list_names().await?;
        table.set_container_names(container_names.into_iter().filter(|n| !n.starts_with(':')));

        // Get host names
        let host_names = host_conn.list_names().await?;
        table.set_host_names(host_names.into_iter().filter(|n| !n.starts_with(':')));

        // Get host activatable names
        let activatable = host_conn.list_activatable_names().await?;
        table.set_host_activatable(activatable);

        info!("Routing table synchronized");
        Ok(())
    }

    /// Run the multiplexer.
    pub async fn run(mut self) -> Result<()> {
        info!("Multiplexer starting");

        loop {
            tokio::select! {
                // Accept new client connections
                result = self.listener.accept() => {
                    match result {
                        Ok((stream, _addr)) => {
                            self.handle_new_client(stream).await;
                        }
                        Err(e) => {
                            error!(error = %e, "Error accepting client connection");
                        }
                    }
                }

                // Handle messages from container bus
                Some(msg) = self.container_conn.recv() => {
                    self.handle_bus_message(msg, Route::Container).await;
                }

                // Handle messages from host bus
                Some(msg) = self.host_conn.recv() => {
                    self.handle_bus_message(msg, Route::Host).await;
                }

                // Handle messages from clients
                Some((client_id, msg)) = self.client_msg_rx.recv() => {
                    self.handle_client_message(client_id, msg).await;
                }
            }
        }
    }

    /// Handle a new client connection.
    async fn handle_new_client(&self, stream: UnixStream) {
        let auth = SaslAuth::with_guid(self.auth.guid().to_string());
        let client_msg_tx = self.client_msg_tx.clone();
        let clients = self.clients.clone();
        let routing_table = self.routing_table.clone();
        let container_conn = self.container_conn.clone();

        tokio::spawn(async move {
            match ClientHandler::accept(stream, &auth).await {
                Ok(handler) => {
                    let client_id = handler.id();
                    let unique_name = handler.unique_name().to_string();
                    let credentials = ClientCredentials {
                        uid: handler.uid(),
                        pid: handler.pid(),
                        gid: handler.gid(),
                    };

                    // Create outbound channel for this client
                    let (outbound_tx, outbound_rx) = mpsc::channel(256);

                    // Register client
                    {
                        let mut clients = clients.write().await;
                        clients.insert(client_id, ClientInfo {
                            unique_name: unique_name.clone(),
                            tx: outbound_tx,
                            owned_names: std::collections::HashSet::new(),
                            match_rules: ClientMatchRules::new(),
                            credentials,
                        });
                    }

                    info!(client_id = client_id, unique_name = %unique_name, 
                          uid = credentials.uid, pid = ?credentials.pid, "Client registered");

                    // Run the client handler
                    if let Err(e) = handler.run(outbound_rx, client_msg_tx).await {
                        warn!(client_id = client_id, error = %e, "Client handler error");
                    }

                    // Unregister client
                    let owned_names: Vec<String>;
                    {
                        let mut clients = clients.write().await;
                        if let Some(info) = clients.remove(&client_id) {
                            owned_names = info.owned_names.into_iter().collect();
                        } else {
                            owned_names = Vec::new();
                        }
                    }

                    // Release any names owned by this client from both
                    // the routing table and the container bus
                    for name in owned_names {
                        // Update routing table
                        {
                            let mut table = routing_table.write().await;
                            table.on_container_name_change(&name, &unique_name, "");
                        }

                        // Release from container bus daemon
                        if let Err(e) = container_conn.connection()
                            .call_method(
                                Some("org.freedesktop.DBus"),
                                "/org/freedesktop/DBus",
                                Some("org.freedesktop.DBus"),
                                "ReleaseName",
                                &(&name,),
                            )
                            .await
                        {
                            warn!(client_id = client_id, name = %name, error = %e, 
                                  "Failed to release name on client disconnect");
                        } else {
                            debug!(client_id = client_id, name = %name, 
                                   "Released name on client disconnect");
                        }
                    }

                    info!(client_id = client_id, "Client disconnected");
                }
                Err(e) => {
                    warn!(error = %e, "Client authentication failed");
                }
            }
        });
    }

    /// Handle a message from a client.
    async fn handle_client_message(&self, client_id: ClientId, msg: Message) {
        trace!(
            client_id = client_id,
            serial = msg.serial(),
            destination = ?msg.destination_str(),
            member = ?msg.member_str(),
            "Processing client message"
        );

        // Check if this is a D-Bus daemon method
        if msg.is_dbus_daemon_call() && msg.is_method_call() {
            self.handle_dbus_method(client_id, msg).await;
            return;
        }

        // Route to appropriate bus
        let destination = msg.destination_str().unwrap_or_default();
        let route = self.router.route_for(&destination).await;

        if msg.is_method_call() {
            if let Err(e) = self.router.forward_method_call(client_id, &msg, route).await {
                error!(client_id = client_id, error = %e, "Failed to forward method call");
                self.send_error_to_client(
                    client_id,
                    &msg,
                    error_names::FAILED,
                    &format!("Failed to forward: {}", e),
                ).await;
            }
        }
    }

    /// Handle a D-Bus daemon method call.
    async fn handle_dbus_method(&self, client_id: ClientId, msg: Message) {
        let member = msg.member_str().unwrap_or_default();

        debug!(client_id = client_id, method = %member, "Handling D-Bus method");

        match member.as_str() {
            "Hello" => self.handle_hello(client_id, &msg).await,
            "RequestName" => self.handle_request_name(client_id, &msg).await,
            "ReleaseName" => self.handle_release_name(client_id, &msg).await,
            "ListNames" => self.handle_list_names(client_id, &msg).await,
            "ListActivatableNames" => self.handle_list_activatable_names(client_id, &msg).await,
            "GetNameOwner" => self.handle_get_name_owner(client_id, &msg).await,
            "NameHasOwner" => self.handle_name_has_owner(client_id, &msg).await,
            "GetId" => self.handle_get_id(client_id, &msg).await,
            "Ping" => self.handle_ping(client_id, &msg).await,
            "GetMachineId" => self.handle_get_machine_id(client_id, &msg).await,
            "GetConnectionUnixUser" => self.handle_get_connection_unix_user(client_id, &msg).await,
            "GetConnectionUnixProcessID" => self.handle_get_connection_unix_process_id(client_id, &msg).await,
            "GetConnectionCredentials" => self.handle_get_connection_credentials(client_id, &msg).await,
            "ListQueuedOwners" => self.handle_list_queued_owners(client_id, &msg).await,
            "AddMatch" => self.handle_add_match(client_id, &msg).await,
            "RemoveMatch" => self.handle_remove_match(client_id, &msg).await,
            "Introspect" => self.handle_introspect(client_id, &msg).await,
            "GetAll" => self.handle_get_all(client_id, &msg).await,
            _ => {
                // Forward unknown methods to the container bus
                debug!(method = %member, "Forwarding unknown D-Bus method to container");
                if let Err(e) = self.router.forward_method_call(client_id, &msg, Route::Container).await {
                    error!(client_id = client_id, error = %e, "Failed to forward D-Bus method");
                    self.send_error_to_client(
                        client_id,
                        &msg,
                        error_names::UNKNOWN_METHOD,
                        &format!("Unknown method: {}", member),
                    ).await;
                }
            }
        }
    }

    /// Find a client's credentials by their bus name.
    /// 
    /// The name can be either:
    /// - A unique name like `:mux.1`
    /// - A well-known name owned by a client
    /// 
    /// Returns None if the name doesn't match any connected client.
    async fn find_client_credentials(&self, name: &str) -> Option<ClientCredentials> {
        let clients = self.clients.read().await;
        
        // First, check if it's a unique name
        for info in clients.values() {
            if info.unique_name == name {
                return Some(info.credentials);
            }
        }
        
        // Then, check if it's a well-known name owned by one of our clients
        for info in clients.values() {
            if info.owned_names.contains(name) {
                return Some(info.credentials);
            }
        }
        
        None
    }

    /// Handle the Hello method.
    async fn handle_hello(&self, client_id: ClientId, msg: &Message) {
        let clients = self.clients.read().await;
        if let Some(info) = clients.get(&client_id) {
            let reply = MethodReturnBuilder::new(msg)
                .destination(&info.unique_name)
                .build(&info.unique_name);

            match reply {
                Ok(reply_msg) => {
                    if let Err(e) = info.tx.send(Arc::new(reply_msg)).await {
                        error!(client_id = client_id, error = %e, "Failed to send Hello reply");
                    }
                }
                Err(e) => {
                    error!(client_id = client_id, error = %e, "Failed to create Hello reply");
                }
            }
        }
    }

    /// Handle the RequestName method.
    async fn handle_request_name(&self, client_id: ClientId, msg: &Message) {
        // Parse the request
        let (name, flags): (String, u32) = match msg.body().deserialize() {
            Ok(v) => v,
            Err(e) => {
                self.send_error_to_client(
                    client_id,
                    msg,
                    error_names::INVALID_ARGS,
                    &format!("Invalid arguments: {}", e),
                ).await;
                return;
            }
        };

        debug!(client_id = client_id, name = %name, flags = flags, "RequestName");

        // Forward to container bus to actually acquire the name
        let result = self.container_conn.connection()
            .call_method(
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                Some("org.freedesktop.DBus"),
                "RequestName",
                &(&name, flags),
            )
            .await;

        match result {
            Ok(reply) => {
                let reply_code: u32 = reply.body().deserialize().unwrap_or(0);

                // Update routing table if successful
                if reply_code == request_name_reply::PRIMARY_OWNER
                    || reply_code == request_name_reply::ALREADY_OWNER
                {
                    let unique_name = {
                        let clients = self.clients.read().await;
                        clients.get(&client_id).map(|c| c.unique_name.clone())
                    };

                    if let Some(unique_name) = unique_name {
                        let mut table = self.routing_table.write().await;
                        table.on_container_name_change(&name, "", &unique_name);

                        // Track owned name for cleanup
                        let mut clients = self.clients.write().await;
                        if let Some(info) = clients.get_mut(&client_id) {
                            info.owned_names.insert(name.clone());
                        }
                    }
                }

                self.send_reply_to_client(client_id, msg, &reply_code).await;
            }
            Err(e) => {
                error!(client_id = client_id, error = %e, "RequestName failed");
                self.send_error_to_client(
                    client_id,
                    msg,
                    error_names::FAILED,
                    &format!("RequestName failed: {}", e),
                ).await;
            }
        }
    }

    /// Handle the ReleaseName method.
    async fn handle_release_name(&self, client_id: ClientId, msg: &Message) {
        let name: String = match msg.body().deserialize() {
            Ok(v) => v,
            Err(e) => {
                self.send_error_to_client(
                    client_id,
                    msg,
                    error_names::INVALID_ARGS,
                    &format!("Invalid arguments: {}", e),
                ).await;
                return;
            }
        };

        debug!(client_id = client_id, name = %name, "ReleaseName");

        // Forward to container bus
        let result = self.container_conn.connection()
            .call_method(
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                Some("org.freedesktop.DBus"),
                "ReleaseName",
                &(&name,),
            )
            .await;

        match result {
            Ok(reply) => {
                let reply_code: u32 = reply.body().deserialize().unwrap_or(0);

                if reply_code == release_name_reply::RELEASED {
                    let unique_name = {
                        let clients = self.clients.read().await;
                        clients.get(&client_id).map(|c| c.unique_name.clone())
                    };

                    if let Some(unique_name) = unique_name {
                        let mut table = self.routing_table.write().await;
                        table.on_container_name_change(&name, &unique_name, "");

                        let mut clients = self.clients.write().await;
                        if let Some(info) = clients.get_mut(&client_id) {
                            info.owned_names.remove(&name);
                        }
                    }
                }

                self.send_reply_to_client(client_id, msg, &reply_code).await;
            }
            Err(e) => {
                error!(client_id = client_id, error = %e, "ReleaseName failed");
                self.send_error_to_client(
                    client_id,
                    msg,
                    error_names::FAILED,
                    &format!("ReleaseName failed: {}", e),
                ).await;
            }
        }
    }

    /// Handle the ListNames method.
    async fn handle_list_names(&self, client_id: ClientId, msg: &Message) {
        // Merge names from both buses
        let names = self.routing_table.read().await.all_names();
        self.send_reply_to_client(client_id, msg, &names).await;
    }

    /// Handle the ListActivatableNames method.
    async fn handle_list_activatable_names(&self, client_id: ClientId, msg: &Message) {
        // Get activatable names from host
        match self.host_conn.list_activatable_names().await {
            Ok(names) => self.send_reply_to_client(client_id, msg, &names).await,
            Err(e) => {
                self.send_error_to_client(
                    client_id,
                    msg,
                    error_names::FAILED,
                    &format!("ListActivatableNames failed: {}", e),
                ).await;
            }
        }
    }

    /// Handle the GetNameOwner method.
    async fn handle_get_name_owner(&self, client_id: ClientId, msg: &Message) {
        let name: String = match msg.body().deserialize() {
            Ok(v) => v,
            Err(e) => {
                self.send_error_to_client(
                    client_id,
                    msg,
                    error_names::INVALID_ARGS,
                    &format!("Invalid arguments: {}", e),
                ).await;
                return;
            }
        };

        // Check routing table to determine which bus to query
        let route = self.routing_table.read().await.get_name_owner_route(&name);

        let conn = match route {
            Some(Route::Container) => &self.container_conn,
            Some(Route::Host) | None => &self.host_conn,
        };

        match conn.get_name_owner(&name).await {
            Ok(Some(owner)) => self.send_reply_to_client(client_id, msg, &owner).await,
            Ok(None) => {
                self.send_error_to_client(
                    client_id,
                    msg,
                    error_names::NAME_HAS_NO_OWNER,
                    &format!("Name {} has no owner", name),
                ).await;
            }
            Err(e) => {
                self.send_error_to_client(
                    client_id,
                    msg,
                    error_names::FAILED,
                    &format!("GetNameOwner failed: {}", e),
                ).await;
            }
        }
    }

    /// Handle the NameHasOwner method.
    async fn handle_name_has_owner(&self, client_id: ClientId, msg: &Message) {
        let name: String = match msg.body().deserialize() {
            Ok(v) => v,
            Err(e) => {
                self.send_error_to_client(
                    client_id,
                    msg,
                    error_names::INVALID_ARGS,
                    &format!("Invalid arguments: {}", e),
                ).await;
                return;
            }
        };

        let has_owner = self.routing_table.read().await.name_has_owner(&name);
        self.send_reply_to_client(client_id, msg, &has_owner).await;
    }

    /// Handle the GetId method.
    async fn handle_get_id(&self, client_id: ClientId, msg: &Message) {
        // Return our server GUID
        self.send_reply_to_client(client_id, msg, &self.auth.guid()).await;
    }

    /// Handle the Ping method (org.freedesktop.DBus.Peer).
    async fn handle_ping(&self, client_id: ClientId, msg: &Message) {
        // Ping just returns an empty reply
        self.send_reply_to_client(client_id, msg, &()).await;
    }

    /// Handle the GetMachineId method (org.freedesktop.DBus.Peer).
    async fn handle_get_machine_id(&self, client_id: ClientId, msg: &Message) {
        // Return the machine ID from /etc/machine-id
        let machine_id = std::fs::read_to_string("/etc/machine-id")
            .unwrap_or_else(|_| "00000000000000000000000000000000".to_string())
            .trim()
            .to_string();
        self.send_reply_to_client(client_id, msg, &machine_id).await;
    }

    /// Handle the GetConnectionUnixUser method.
    async fn handle_get_connection_unix_user(&self, client_id: ClientId, msg: &Message) {
        let name: String = match msg.body().deserialize() {
            Ok(v) => v,
            Err(e) => {
                self.send_error_to_client(client_id, msg, error_names::INVALID_ARGS, &format!("Invalid arguments: {}", e)).await;
                return;
            }
        };

        // For org.freedesktop.DBus, return 0 (root)
        if name == "org.freedesktop.DBus" {
            self.send_reply_to_client(client_id, msg, &0u32).await;
            return;
        }

        // Look up the client by name
        if let Some(creds) = self.find_client_credentials(&name).await {
            self.send_reply_to_client(client_id, msg, &creds.uid).await;
        } else {
            // Name not found among our clients
            self.send_error_to_client(
                client_id,
                msg,
                error_names::NAME_HAS_NO_OWNER,
                &format!("Could not get UID of name '{}': no such name", name),
            ).await;
        }
    }

    /// Handle the GetConnectionUnixProcessID method.
    async fn handle_get_connection_unix_process_id(&self, client_id: ClientId, msg: &Message) {
        let name: String = match msg.body().deserialize() {
            Ok(v) => v,
            Err(e) => {
                self.send_error_to_client(client_id, msg, error_names::INVALID_ARGS, &format!("Invalid arguments: {}", e)).await;
                return;
            }
        };

        // For org.freedesktop.DBus, return our PID (the multiplexer's PID)
        if name == "org.freedesktop.DBus" {
            let pid = std::process::id();
            self.send_reply_to_client(client_id, msg, &pid).await;
            return;
        }

        // Look up the client by name
        if let Some(creds) = self.find_client_credentials(&name).await {
            if let Some(pid) = creds.pid {
                self.send_reply_to_client(client_id, msg, &pid).await;
            } else {
                // PID not available (shouldn't happen on Linux, but handle it)
                self.send_error_to_client(
                    client_id,
                    msg,
                    error_names::UNIX_PROCESS_ID_UNKNOWN,
                    &format!("Could not get PID of name '{}': not available", name),
                ).await;
            }
        } else {
            // Name not found among our clients
            self.send_error_to_client(
                client_id,
                msg,
                error_names::NAME_HAS_NO_OWNER,
                &format!("Could not get PID of name '{}': no such name", name),
            ).await;
        }
    }

    /// Handle the GetConnectionCredentials method.
    async fn handle_get_connection_credentials(&self, client_id: ClientId, msg: &Message) {
        let name: String = match msg.body().deserialize() {
            Ok(v) => v,
            Err(e) => {
                self.send_error_to_client(client_id, msg, error_names::INVALID_ARGS, &format!("Invalid arguments: {}", e)).await;
                return;
            }
        };

        use std::collections::HashMap;
        use zbus::zvariant::{OwnedValue, Value};

        // For org.freedesktop.DBus, return our own credentials
        if name == "org.freedesktop.DBus" {
            let uid = nix::unistd::getuid().as_raw();
            let pid = std::process::id();
            let gid = nix::unistd::getgid().as_raw();
            
            let mut creds: HashMap<String, OwnedValue> = HashMap::new();
            creds.insert("UnixUserID".to_string(), Value::from(uid).try_into().unwrap());
            creds.insert("ProcessID".to_string(), Value::from(pid).try_into().unwrap());
            creds.insert("UnixGroupID".to_string(), Value::from(gid).try_into().unwrap());
            
            self.send_reply_to_client(client_id, msg, &creds).await;
            return;
        }

        // Look up the client by name
        if let Some(client_creds) = self.find_client_credentials(&name).await {
            let mut creds: HashMap<String, OwnedValue> = HashMap::new();
            creds.insert("UnixUserID".to_string(), Value::from(client_creds.uid).try_into().unwrap());
            if let Some(pid) = client_creds.pid {
                creds.insert("ProcessID".to_string(), Value::from(pid).try_into().unwrap());
            }
            creds.insert("UnixGroupID".to_string(), Value::from(client_creds.gid).try_into().unwrap());
            
            self.send_reply_to_client(client_id, msg, &creds).await;
        } else {
            // Name not found among our clients
            self.send_error_to_client(
                client_id,
                msg,
                error_names::NAME_HAS_NO_OWNER,
                &format!("Could not get credentials of name '{}': no such name", name),
            ).await;
        }
    }

    /// Handle the ListQueuedOwners method.
    async fn handle_list_queued_owners(&self, client_id: ClientId, msg: &Message) {
        let name: String = match msg.body().deserialize() {
            Ok(v) => v,
            Err(e) => {
                self.send_error_to_client(client_id, msg, error_names::INVALID_ARGS, &format!("Invalid arguments: {}", e)).await;
                return;
            }
        };

        // For now, just return empty list - we don't track queued owners
        // The routing table only tracks which bus owns each name, not specific owners
        let table = self.routing_table.read().await;
        let owners: Vec<String> = if table.get_name_owner_route(&name).is_some() {
            // Name exists but we don't track the actual owner string
            vec![name.clone()]
        } else {
            Vec::new()
        };
        
        self.send_reply_to_client(client_id, msg, &owners).await;
    }

    /// Handle the Introspect method (org.freedesktop.DBus.Introspectable).
    async fn handle_introspect(&self, client_id: ClientId, msg: &Message) {
        let introspect_xml = r#"<!DOCTYPE node PUBLIC "-//freedesktop//DTD D-BUS Object Introspection 1.0//EN"
"http://www.freedesktop.org/standards/dbus/1.0/introspect.dtd">
<node>
  <interface name="org.freedesktop.DBus">
    <method name="Hello">
      <arg direction="out" type="s"/>
    </method>
    <method name="RequestName">
      <arg direction="in" type="s"/>
      <arg direction="in" type="u"/>
      <arg direction="out" type="u"/>
    </method>
    <method name="ReleaseName">
      <arg direction="in" type="s"/>
      <arg direction="out" type="u"/>
    </method>
    <method name="ListNames">
      <arg direction="out" type="as"/>
    </method>
    <method name="ListActivatableNames">
      <arg direction="out" type="as"/>
    </method>
    <method name="GetNameOwner">
      <arg direction="in" type="s"/>
      <arg direction="out" type="s"/>
    </method>
    <method name="NameHasOwner">
      <arg direction="in" type="s"/>
      <arg direction="out" type="b"/>
    </method>
    <method name="GetId">
      <arg direction="out" type="s"/>
    </method>
    <method name="GetConnectionUnixUser">
      <arg direction="in" type="s"/>
      <arg direction="out" type="u"/>
    </method>
    <method name="GetConnectionCredentials">
      <arg direction="in" type="s"/>
      <arg direction="out" type="a{sv}"/>
    </method>
    <method name="ListQueuedOwners">
      <arg direction="in" type="s"/>
      <arg direction="out" type="as"/>
    </method>
    <method name="AddMatch">
      <arg direction="in" type="s"/>
    </method>
    <method name="RemoveMatch">
      <arg direction="in" type="s"/>
    </method>
    <signal name="NameOwnerChanged">
      <arg type="s"/>
      <arg type="s"/>
      <arg type="s"/>
    </signal>
    <signal name="NameAcquired">
      <arg type="s"/>
    </signal>
    <signal name="NameLost">
      <arg type="s"/>
    </signal>
  </interface>
  <interface name="org.freedesktop.DBus.Peer">
    <method name="Ping"/>
    <method name="GetMachineId">
      <arg direction="out" type="s"/>
    </method>
  </interface>
  <interface name="org.freedesktop.DBus.Introspectable">
    <method name="Introspect">
      <arg direction="out" type="s"/>
    </method>
  </interface>
</node>"#;
        self.send_reply_to_client(client_id, msg, &introspect_xml).await;
    }

    /// Handle the AddMatch method.
    async fn handle_add_match(&self, client_id: ClientId, msg: &Message) {
        let rule_string: String = match msg.body().deserialize() {
            Ok(v) => v,
            Err(e) => {
                self.send_error_to_client(
                    client_id,
                    msg,
                    error_names::INVALID_ARGS,
                    &format!("Invalid arguments: {}", e),
                ).await;
                return;
            }
        };

        debug!(client_id = client_id, rule = %rule_string, "AddMatch");

        // Parse the match rule
        let rule = match MatchRule::parse(&rule_string) {
            Ok(r) => r,
            Err(e) => {
                self.send_error_to_client(
                    client_id,
                    msg,
                    error_names::MATCH_RULE_INVALID,
                    &format!("Invalid match rule: {}", e),
                ).await;
                return;
            }
        };

        // Add to client's match rules
        {
            let mut clients = self.clients.write().await;
            if let Some(info) = clients.get_mut(&client_id) {
                info.match_rules.add(rule);
                debug!(
                    client_id = client_id, 
                    rule_count = info.match_rules.len(),
                    "Added match rule"
                );
            }
        }

        // Also forward to both buses to receive the signals
        // This ensures the mux itself receives signals that match the rule
        let _ = self.container_conn.connection()
            .call_method(
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                Some("org.freedesktop.DBus"),
                "AddMatch",
                &(&rule_string,),
            )
            .await;
        
        let _ = self.host_conn.connection()
            .call_method(
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                Some("org.freedesktop.DBus"),
                "AddMatch",
                &(&rule_string,),
            )
            .await;

        // AddMatch returns void on success
        self.send_reply_to_client(client_id, msg, &()).await;
    }

    /// Handle the RemoveMatch method.
    async fn handle_remove_match(&self, client_id: ClientId, msg: &Message) {
        let rule_string: String = match msg.body().deserialize() {
            Ok(v) => v,
            Err(e) => {
                self.send_error_to_client(
                    client_id,
                    msg,
                    error_names::INVALID_ARGS,
                    &format!("Invalid arguments: {}", e),
                ).await;
                return;
            }
        };

        debug!(client_id = client_id, rule = %rule_string, "RemoveMatch");

        // Remove from client's match rules
        let removed = {
            let mut clients = self.clients.write().await;
            if let Some(info) = clients.get_mut(&client_id) {
                let removed = info.match_rules.remove(&rule_string);
                if removed {
                    debug!(
                        client_id = client_id,
                        rule_count = info.match_rules.len(),
                        "Removed match rule"
                    );
                }
                removed
            } else {
                false
            }
        };

        if !removed {
            self.send_error_to_client(
                client_id,
                msg,
                error_names::MATCH_RULE_NOT_FOUND,
                "Match rule not found",
            ).await;
            return;
        }

        // Note: We don't remove from the actual buses because other clients
        // might still be using the same match rule. A production implementation
        // would ref-count match rules across clients.

        // RemoveMatch returns void on success
        self.send_reply_to_client(client_id, msg, &()).await;
    }

    /// Handle the GetAll method (org.freedesktop.DBus.Properties).
    async fn handle_get_all(&self, client_id: ClientId, msg: &Message) {
        // For the D-Bus daemon, there are no standard properties, so return empty dict
        use std::collections::HashMap;
        use zbus::zvariant::OwnedValue;
        
        let props: HashMap<String, OwnedValue> = HashMap::new();
        self.send_reply_to_client(client_id, msg, &props).await;
    }

    /// Handle a message from one of the buses.
    async fn handle_bus_message(&self, msg: Arc<Message>, route: Route) {
        trace!(
            route = %route,
            msg_type = ?msg.as_ref().primary_header().msg_type(),
            sender = ?msg.sender_str(),
            destination = ?msg.destination_str(),
            reply_serial = ?msg.reply_serial(),
            "Received message from bus"
        );

        // Handle NameOwnerChanged signals
        if msg.is_name_owner_changed() {
            if let Ok((name, old_owner, new_owner)) = parse_name_owner_changed(&msg) {
                debug!(
                    route = %route,
                    name = %name,
                    old_owner = %old_owner,
                    new_owner = %new_owner,
                    "NameOwnerChanged"
                );

                let mut table = self.routing_table.write().await;
                match route {
                    Route::Container => table.on_container_name_change(&name, &old_owner, &new_owner),
                    Route::Host => table.on_host_name_change(&name, &old_owner, &new_owner),
                }
            }
            return;
        }

        // Handle replies
        if msg.is_method_return() || msg.is_error() {
            if let Ok(Some((client_id, client_serial))) = self.router.handle_reply(&msg, route).await {
                // Rewrite the reply_serial to match the client's original serial
                let reply_msg = match clone_reply_with_serial(&msg, client_serial) {
                    Ok(m) => Arc::new(m),
                    Err(e) => {
                        warn!(
                            client_id = client_id,
                            error = %e,
                            "Failed to rewrite reply serial, forwarding original"
                        );
                        msg.clone()
                    }
                };
                
                // Send reply to client
                let clients = self.clients.read().await;
                if let Some(info) = clients.get(&client_id) {
                    if let Err(e) = info.tx.send(reply_msg).await {
                        warn!(client_id = client_id, error = %e, "Failed to send reply to client");
                    }
                }
            }
        }

        // Handle signals - filter by match rules
        if msg.is_signal() {
            // Forward signals only to clients whose match rules match this signal
            let clients = self.clients.read().await;
            let mut forward_count = 0;
            
            for (client_id, info) in clients.iter() {
                // Check if any of the client's match rules match this signal
                if info.match_rules.matches(&msg) {
                    if let Err(e) = info.tx.send(msg.clone()).await {
                        trace!(client_id = client_id, error = %e, "Failed to send signal to client");
                    } else {
                        forward_count += 1;
                    }
                }
            }
            
            if forward_count > 0 {
                trace!(
                    interface = ?msg.interface_str(),
                    member = ?msg.member_str(),
                    route = %route,
                    forward_count = forward_count,
                    "Forwarded signal to matching clients"
                );
            }
        }
    }

    /// Send a reply to a client for a given original message.
    async fn send_reply_to_client<T: serde::Serialize + zbus::zvariant::Type + std::fmt::Debug>(
        &self,
        client_id: ClientId,
        original_msg: &Message,
        body: &T,
    ) {
        let clients = self.clients.read().await;
        if let Some(info) = clients.get(&client_id) {
            let reply = MethodReturnBuilder::new(original_msg)
                .destination(&info.unique_name)
                .build(body);

            match reply {
                Ok(msg) => {
                    trace!(
                        client_id = client_id,
                        serial = msg.serial(),
                        reply_serial = msg.reply_serial(),
                        "Sending reply to client"
                    );
                    if let Err(e) = info.tx.send(Arc::new(msg)).await {
                        warn!(client_id = client_id, error = %e, "Failed to send reply to client");
                    }
                }
                Err(e) => {
                    error!(client_id = client_id, error = %e, "Failed to create reply");
                }
            }
        } else {
            warn!(client_id = client_id, "Client not found when sending reply");
        }
    }

    /// Send an error to a client for a given original message.
    async fn send_error_to_client(
        &self,
        client_id: ClientId,
        original_msg: &Message,
        error_name: &str,
        message: &str,
    ) {
        let clients = self.clients.read().await;
        if let Some(info) = clients.get(&client_id) {
            let error = ErrorBuilder::new(original_msg, error_name)
                .destination(&info.unique_name)
                .build(message);

            match error {
                Ok(msg) => {
                    if let Err(e) = info.tx.send(Arc::new(msg)).await {
                        warn!(client_id = client_id, error = %e, "Failed to send error to client");
                    }
                }
                Err(e) => {
                    error!(client_id = client_id, error = %e, "Failed to create error");
                }
            }
        }
    }
}

impl Drop for Multiplexer {
    fn drop(&mut self) {
        // Clean up the socket file
        if self.listen_path.exists() {
            let _ = std::fs::remove_file(&self.listen_path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test ClientCredentials struct.
    #[test]
    fn test_client_credentials_clone() {
        let creds = ClientCredentials {
            uid: 1000,
            pid: Some(12345),
            gid: 1000,
        };
        let cloned = creds;
        assert_eq!(cloned.uid, 1000);
        assert_eq!(cloned.pid, Some(12345));
        assert_eq!(cloned.gid, 1000);
    }

    #[test]
    fn test_client_credentials_without_pid() {
        let creds = ClientCredentials {
            uid: 1000,
            pid: None,
            gid: 1000,
        };
        assert_eq!(creds.uid, 1000);
        assert!(creds.pid.is_none());
        assert_eq!(creds.gid, 1000);
    }

    #[test]
    fn test_client_credentials_debug() {
        let creds = ClientCredentials {
            uid: 1000,
            pid: Some(12345),
            gid: 1000,
        };
        let debug_str = format!("{:?}", creds);
        assert!(debug_str.contains("uid: 1000"));
        assert!(debug_str.contains("pid: Some(12345)"));
        assert!(debug_str.contains("gid: 1000"));
    }
}
