# Rust D-Bus Multiplexer (`kapsule-dbus-mux`)

A D-Bus multiplexer that routes messages between a container's internal bus and the host session bus, enabling container applications to seamlessly access host services while allowing container-local services to take priority.

## Goals

1. **Forward all host services** - Container apps can call any service on the host session bus
2. **Container services take priority** - When a container service owns a name, route to container bus instead
3. **Dynamic routing** - When a container process calls `RequestName()`, update routing immediately
4. **Transparent to apps** - Apps see a single D-Bus session bus, unaware of the multiplexing

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────────────┐
│                           Container                                      │
│                                                                          │
│   ┌─────────────┐  ┌─────────────┐  ┌─────────────┐                     │
│   │   App A     │  │   App B     │  │  Service X  │                     │
│   └──────┬──────┘  └──────┬──────┘  └──────┬──────┘                     │
│          │                │                │                             │
│          └────────────────┼────────────────┘                             │
│                           │                                              │
│                           ▼                                              │
│          DBUS_SESSION_BUS_ADDRESS=unix:path=.../mux.socket              │
│                           │                                              │
└───────────────────────────┼──────────────────────────────────────────────┘
                            │
┌───────────────────────────▼──────────────────────────────────────────────┐
│                    kapsule-dbus-mux                                       │
│                                                                          │
│  ┌────────────────────────────────────────────────────────────────────┐ │
│  │                     Connection Manager                              │ │
│  │  • Accept client connections on mux.socket                         │ │
│  │  • Handle SASL authentication (EXTERNAL)                           │ │
│  │  • Create ClientConnection for each client                         │ │
│  └────────────────────────────────────────────────────────────────────┘ │
│                                                                          │
│  ┌────────────────────────────────────────────────────────────────────┐ │
│  │                     Routing Table                                   │ │
│  │  ┌─────────────────────────────┬─────────────┐                     │ │
│  │  │ Well-Known Name             │ Route       │                     │ │
│  │  ├─────────────────────────────┼─────────────┤                     │ │
│  │  │ org.freedesktop.Notifications│ HOST       │                     │ │
│  │  │ org.kde.kwalletd6           │ HOST        │                     │ │
│  │  │ com.example.ContainerApp    │ CONTAINER   │                     │ │
│  │  │ org.freedesktop.DBus        │ SPECIAL*    │                     │ │
│  │  │ <unknown>                   │ HOST        │ ← default fallback  │ │
│  │  └─────────────────────────────┴─────────────┘                     │ │
│  └────────────────────────────────────────────────────────────────────┘ │
│                                                                          │
│  ┌────────────────────────────────────────────────────────────────────┐ │
│  │                     Message Router                                  │ │
│  │  • Parse message headers (destination, serial, type)               │ │
│  │  • Lookup destination in routing table                             │ │
│  │  • Forward to appropriate bus connection                           │ │
│  │  • Correlate replies via serial mapping                            │ │
│  └────────────────────────────────────────────────────────────────────┘ │
│                                                                          │
│  ┌─────────────────────┐              ┌─────────────────────┐          │
│  │ Container Bus Conn  │              │   Host Bus Conn     │          │
│  │                     │              │                     │          │
│  │ unix:path=...       │              │ unix:path=/run/     │          │
│  │ /container-bus.sock │              │ user/1000/bus       │          │
│  └──────────┬──────────┘              └──────────┬──────────┘          │
│             │                                    │                      │
└─────────────┼────────────────────────────────────┼──────────────────────┘
              │                                    │
              ▼                                    ▼
    ┌─────────────────┐                  ┌─────────────────┐
    │ Container's     │                  │   Host Session  │
    │ dbus-daemon     │                  │   Bus           │
    │ (internal)      │                  │                 │
    └─────────────────┘                  └─────────────────┘
```

## Core Components

### 1. Main Entry Point

```rust
// src/main.rs

use clap::Parser;

#[derive(Parser)]
#[command(name = "kapsule-dbus-mux")]
#[command(about = "D-Bus multiplexer for container/host bus routing")]
struct Args {
    /// Socket path for clients to connect to
    #[arg(long)]
    listen: PathBuf,
    
    /// Container's internal D-Bus socket
    #[arg(long)]
    container_bus: String,
    
    /// Host session bus address (defaults to DBUS_SESSION_BUS_ADDRESS)
    #[arg(long)]
    host_bus: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    
    let mux = Multiplexer::new(
        args.listen,
        args.container_bus,
        args.host_bus,
    ).await?;
    
    mux.run().await
}
```

### 2. Multiplexer Core

```rust
// src/multiplexer.rs

pub struct Multiplexer {
    /// Socket we listen on for client connections
    listener: UnixListener,
    
    /// Connection to container's internal dbus-daemon
    container_conn: BusConnection,
    
    /// Connection to host session bus
    host_conn: BusConnection,
    
    /// Routing table: name -> Route
    routing_table: Arc<RwLock<RoutingTable>>,
    
    /// Active client connections
    clients: Arc<RwLock<HashMap<ClientId, ClientConnection>>>,
    
    /// Serial number mappings for reply correlation
    serial_map: Arc<RwLock<SerialMap>>,
}

impl Multiplexer {
    pub async fn new(
        listen_path: PathBuf,
        container_bus: String,
        host_bus: Option<String>,
    ) -> Result<Self> {
        // Create listener socket
        let listener = UnixListener::bind(&listen_path)?;
        
        // Connect to both buses
        let container_conn = BusConnection::connect(&container_bus).await?;
        let host_conn = BusConnection::connect_session(host_bus.as_deref()).await?;
        
        // Initialize routing table from current bus state
        let routing_table = RoutingTable::new();
        routing_table.sync_from_buses(&container_conn, &host_conn).await?;
        
        // Subscribe to NameOwnerChanged on both buses
        container_conn.subscribe_name_changes().await?;
        host_conn.subscribe_name_changes().await?;
        
        Ok(Self {
            listener,
            container_conn,
            host_conn,
            routing_table: Arc::new(RwLock::new(routing_table)),
            clients: Arc::new(RwLock::new(HashMap::new())),
            serial_map: Arc::new(RwLock::new(SerialMap::new())),
        })
    }
    
    pub async fn run(self) -> Result<()> {
        // Spawn tasks for:
        // 1. Accepting new client connections
        // 2. Handling messages from container bus
        // 3. Handling messages from host bus
        // 4. Processing NameOwnerChanged signals
        
        tokio::select! {
            r = self.accept_clients() => r,
            r = self.handle_container_messages() => r,
            r = self.handle_host_messages() => r,
        }
    }
}
```

### 3. Routing Table

```rust
// src/routing.rs

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// Route to container's internal bus
    Container,
    /// Route to host session bus  
    Host,
}

pub struct RoutingTable {
    /// Explicit routes for well-known names
    routes: HashMap<String, Route>,
    
    /// Names owned on container bus (always route to Container)
    container_names: HashSet<String>,
    
    /// Names owned on host bus (route to Host if not in container_names)
    host_names: HashSet<String>,
    
    /// Activatable names on host (can be started on demand)
    host_activatable: HashSet<String>,
}

impl RoutingTable {
    /// Determine route for a destination
    pub fn route_for(&self, destination: &str) -> Route {
        // Unique names (":1.xxx") - need to track which bus they're on
        if destination.starts_with(':') {
            return self.route_for_unique_name(destination);
        }
        
        // Well-known names - container takes priority
        if self.container_names.contains(destination) {
            return Route::Container;
        }
        
        if self.host_names.contains(destination) {
            return Route::Host;
        }
        
        // Activatable on host?
        if self.host_activatable.contains(destination) {
            return Route::Host;
        }
        
        // Default: try host (allows unknown services to be activated)
        Route::Host
    }
    
    /// Handle NameOwnerChanged from container bus
    pub fn on_container_name_change(
        &mut self,
        name: &str,
        old_owner: &str,
        new_owner: &str,
    ) {
        if new_owner.is_empty() {
            // Name released on container
            self.container_names.remove(name);
        } else {
            // Name acquired on container - takes priority over host
            self.container_names.insert(name.to_string());
        }
    }
    
    /// Handle NameOwnerChanged from host bus
    pub fn on_host_name_change(
        &mut self,
        name: &str, 
        old_owner: &str,
        new_owner: &str,
    ) {
        if new_owner.is_empty() {
            self.host_names.remove(name);
        } else {
            self.host_names.insert(name.to_string());
        }
    }
}
```

### 4. Client Connection Handler

```rust
// src/client.rs

pub struct ClientConnection {
    id: ClientId,
    stream: UnixStream,
    
    /// Client's unique name on our virtual bus (e.g., ":mux.1")
    unique_name: String,
    
    /// Names this client has requested ownership of
    owned_names: HashSet<String>,
    
    /// Pending method calls: client_serial -> (target_bus, forwarded_serial)
    pending_calls: HashMap<u32, PendingCall>,
}

impl ClientConnection {
    pub async fn handle_auth(&mut self) -> Result<()> {
        // D-Bus SASL authentication
        // For EXTERNAL auth, we send our own credentials
        // Client sees mux's PID/UID, not the actual bus daemon's
        
        let auth = SaslAuth::new(&mut self.stream);
        auth.authenticate_as_server().await?;
        
        // Send Hello reply with unique name
        self.send_hello_reply().await
    }
    
    pub async fn process_message(&mut self, msg: Message) -> Result<MessageAction> {
        // Special handling for org.freedesktop.DBus methods
        if msg.destination() == Some("org.freedesktop.DBus") {
            return self.handle_dbus_method(msg).await;
        }
        
        // Determine routing
        let route = self.routing_table.read().await.route_for(
            msg.destination().unwrap_or("")
        );
        
        Ok(MessageAction::Forward { route, message: msg })
    }
    
    async fn handle_dbus_method(&mut self, msg: Message) -> Result<MessageAction> {
        match msg.member() {
            Some("RequestName") => self.handle_request_name(msg).await,
            Some("ReleaseName") => self.handle_release_name(msg).await,
            Some("ListNames") => self.handle_list_names(msg).await,
            Some("GetNameOwner") => self.handle_get_name_owner(msg).await,
            Some("NameHasOwner") => self.handle_name_has_owner(msg).await,
            // Forward other methods to appropriate bus
            _ => Ok(MessageAction::Forward { 
                route: Route::Container, // or merge from both?
                message: msg 
            }),
        }
    }
    
    async fn handle_request_name(&mut self, msg: Message) -> Result<MessageAction> {
        // Client wants to own a name
        // 1. Forward to container bus (container services register there)
        // 2. Update routing table on success
        // 3. Return result to client
        
        let name: &str = msg.body().deserialize()?;
        
        // Forward to container bus
        let reply = self.container_conn.call(msg.clone()).await?;
        
        if reply.is_success() {
            // Update routing table
            self.routing_table.write().await
                .container_names.insert(name.to_string());
            self.owned_names.insert(name.to_string());
        }
        
        Ok(MessageAction::Reply(reply))
    }
}
```

### 5. Message Router

```rust
// src/router.rs

pub struct MessageRouter {
    container_conn: BusConnection,
    host_conn: BusConnection,
    routing_table: Arc<RwLock<RoutingTable>>,
    serial_map: Arc<RwLock<SerialMap>>,
}

impl MessageRouter {
    /// Forward a message to the appropriate bus
    pub async fn forward(
        &self,
        client_id: ClientId,
        msg: Message,
        route: Route,
    ) -> Result<()> {
        let conn = match route {
            Route::Container => &self.container_conn,
            Route::Host => &self.host_conn,
        };
        
        // Rewrite serial number
        let original_serial = msg.serial();
        let new_serial = conn.next_serial();
        
        // Track for reply correlation
        self.serial_map.write().await.insert(
            (route, new_serial),
            SerialMapping {
                client_id,
                client_serial: original_serial,
            },
        );
        
        // Forward with file descriptors
        conn.send_with_fds(msg.with_serial(new_serial)).await
    }
    
    /// Handle a reply from one of the buses
    pub async fn handle_reply(
        &self,
        route: Route,
        reply: Message,
    ) -> Result<Option<(ClientId, Message)>> {
        let reply_serial = reply.reply_serial().ok_or(Error::NoReplySerial)?;
        
        // Look up original client
        let mapping = self.serial_map.write().await
            .remove(&(route, reply_serial));
        
        if let Some(mapping) = mapping {
            // Rewrite reply serial to client's original
            let client_reply = reply.with_reply_serial(mapping.client_serial);
            Ok(Some((mapping.client_id, client_reply)))
        } else {
            Ok(None)
        }
    }
}
```

### 6. Bus Connection (using zbus)

```rust
// src/bus_connection.rs

use zbus::{Connection, MessageStream};

pub struct BusConnection {
    connection: Connection,
    stream: MessageStream,
    serial_counter: AtomicU32,
}

impl BusConnection {
    pub async fn connect(address: &str) -> Result<Self> {
        let connection = Connection::session().await?;
        // Or for custom address:
        // let connection = ConnectionBuilder::address(address)?.build().await?;
        
        let stream = MessageStream::from(&connection);
        
        Ok(Self {
            connection,
            stream,
            serial_counter: AtomicU32::new(1),
        })
    }
    
    pub async fn subscribe_name_changes(&self) -> Result<()> {
        self.connection.call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "AddMatch",
            &("type='signal',member='NameOwnerChanged'",),
        ).await?;
        Ok(())
    }
    
    pub async fn list_names(&self) -> Result<Vec<String>> {
        let reply = self.connection.call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus", 
            Some("org.freedesktop.DBus"),
            "ListNames",
            &(),
        ).await?;
        Ok(reply.body().deserialize()?)
    }
    
    pub async fn list_activatable_names(&self) -> Result<Vec<String>> {
        let reply = self.connection.call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "ListActivatableNames", 
            &(),
        ).await?;
        Ok(reply.body().deserialize()?)
    }
}
```

### 7. SASL Authentication

```rust
// src/auth.rs

pub struct SaslAuth<'a> {
    stream: &'a mut UnixStream,
}

impl<'a> SaslAuth<'a> {
    /// Authenticate as a D-Bus server
    pub async fn authenticate_as_server(&mut self) -> Result<()> {
        // Read null byte
        let mut buf = [0u8; 1];
        self.stream.read_exact(&mut buf).await?;
        
        // Read AUTH command
        let line = self.read_line().await?;
        
        if line.starts_with("AUTH EXTERNAL") {
            // Client sends hex-encoded UID
            // We accept it and send OK with our server GUID
            let guid = generate_guid();
            self.write_line(&format!("OK {}", guid)).await?;
        } else if line == "AUTH" {
            // Client asking what mechanisms we support
            self.write_line("REJECTED EXTERNAL").await?;
            return self.authenticate_as_server().await;
        } else {
            return Err(Error::UnsupportedAuth);
        }
        
        // Wait for BEGIN
        let line = self.read_line().await?;
        if line != "BEGIN" {
            return Err(Error::ExpectedBegin);
        }
        
        Ok(())
    }
}
```

## File Structure

```
kapsule-dbus-mux/
├── Cargo.toml
├── src/
│   ├── main.rs              # Entry point, CLI args
│   ├── multiplexer.rs       # Core multiplexer logic
│   ├── routing.rs           # Routing table
│   ├── client.rs            # Client connection handling
│   ├── router.rs            # Message routing/forwarding
│   ├── bus_connection.rs    # zbus wrapper
│   ├── auth.rs              # SASL authentication
│   ├── serial_map.rs        # Serial number correlation
│   ├── message.rs           # Message parsing helpers
│   └── error.rs             # Error types
└── tests/
    ├── routing_tests.rs
    ├── auth_tests.rs
    └── integration_tests.rs
```

## Cargo.toml

```toml
[package]
name = "kapsule-dbus-mux"
version = "0.1.0"
edition = "2021"

[dependencies]
zbus = "4"                    # D-Bus library
tokio = { version = "1", features = ["full"] }
clap = { version = "4", features = ["derive"] }
tracing = "0.1"
tracing-subscriber = "0.3"
thiserror = "1"
futures = "0.3"

[dev-dependencies]
tempfile = "3"
```

## Key Challenges & Solutions

| Challenge | Solution |
|-----------|----------|
| **SASL Authentication** | zbus handles this for bus connections; for client connections, implement simple EXTERNAL auth |
| **Serial Number Correlation** | HashMap mapping `(Route, serial)` → `(ClientId, original_serial)` |
| **File Descriptor Passing** | zbus supports FDs natively; pass through unchanged |
| **Unique Name Tracking** | Track which unique names belong to which bus to route replies correctly |
| **ListNames Merging** | Return union of container + host names (deduplicated) |
| **Race Conditions** | Name acquired between routing decision and forward? Retry with updated route |

## Estimated Effort

| Component | Lines | Time |
|-----------|-------|------|
| Main + CLI | ~50 | 1 day |
| Multiplexer core | ~150 | 2 days |
| Routing table | ~100 | 1 day |
| Client handling | ~200 | 3 days |
| Message router | ~150 | 2 days |
| SASL auth | ~100 | 2 days |
| Bus connection | ~80 | 1 day |
| Tests | ~200 | 3 days |
| **Total** | **~1,000** | **~3 weeks** |

## Integration with Kapsule

```python
# In kapsule CLI/daemon - spawn the mux

async def start_dbus_mux(container_name: str) -> str:
    """Start D-Bus multiplexer for a container."""
    runtime_dir = Path(f"/run/user/{os.getuid()}/kapsule/{container_name}")
    runtime_dir.mkdir(parents=True, exist_ok=True)
    
    mux_socket = runtime_dir / "bus"
    container_internal_bus = runtime_dir / "internal-bus.socket"
    
    # Start the container's internal dbus-daemon first
    # (separate process, listens on container_internal_bus)
    
    # Then start the multiplexer
    process = await asyncio.create_subprocess_exec(
        "kapsule-dbus-mux",
        "--listen", str(mux_socket),
        "--container-bus", f"unix:path={container_internal_bus}",
        # host-bus defaults to DBUS_SESSION_BUS_ADDRESS
    )
    
    return f"unix:path={mux_socket}"
```

## Development Phases

### Phase 1: Minimal Viable Proxy (1 week)

- Accept client connections
- SASL EXTERNAL authentication
- Forward method calls to host bus only
- Reply correlation
- No container bus, no routing

### Phase 2: Dual-Bus Routing (1 week)

- Connect to both buses
- Routing table with container priority
- NameOwnerChanged monitoring
- Dynamic route updates

### Phase 3: Full Feature Set (1 week)

- File descriptor passing
- Signal forwarding
- Merged ListNames/GetNameOwner
- Error handling and edge cases

### Phase 4: Testing & Integration (1 week)

- Unit tests for each component
- Integration tests with real D-Bus
- Test with KDE apps (Dolphin, Kate, etc.)
- Performance profiling
