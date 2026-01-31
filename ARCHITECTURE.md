# kapsule-dbus-mux Architecture Reference

## Overview
D-Bus multiplexer routing between container and host buses. Container names take priority.

## Module Map

```
main.rs          CLI (clap): --listen, --container-bus, --host-bus, --log-level
lib.rs           Re-exports: Multiplexer, Route, RoutingTable, Error, Result
multiplexer.rs   Main event loop, D-Bus daemon method handling
routing.rs       RoutingTable, Route enum, UniqueNameRegistry
client.rs        ClientConnection, ClientHandler (per-client task)
router.rs        MessageRouter, serial correlation
bus_connection.rs BusConnection (zbus wrapper)
auth.rs          SaslAuth (EXTERNAL auth)
serial_map.rs    SerialMap, PendingCall, ClientId
message.rs       MessageExt trait, MethodReturnBuilder, ErrorBuilder
error.rs         Error, AuthError, RoutingError, ClientError, MessageError
```

## Key Types

```rust
// routing.rs
enum Route { Container, Host }
type ClientId = u64;  // from serial_map.rs

// multiplexer.rs
struct ClientInfo { unique_name: String, tx: mpsc::Sender<Arc<Message>>, owned_names: HashSet<String> }

// Client naming: ":mux.{id}" where id is atomic counter
```

## Multiplexer State

```rust
listener: UnixListener                              // Client socket
listen_path: PathBuf                                // For cleanup on drop
container_conn: Arc<BusConnection>                  // Container bus
host_conn: Arc<BusConnection>                       // Host bus  
routing_table: Arc<RwLock<RoutingTable>>           // Name -> Route
clients: Arc<RwLock<HashMap<ClientId, ClientInfo>>> // Active clients
router: Arc<MessageRouter>                          // Forwarding + serial map
auth: SaslAuth                                      // Shared GUID
client_msg_tx/rx: mpsc::channel                     // Client -> main loop
```

## Event Loop (tokio::select!)

1. `listener.accept()` → spawn ClientHandler task
2. `container_conn.recv()` → handle_bus_message(Route::Container)
3. `host_conn.recv()` → handle_bus_message(Route::Host)
4. `client_msg_rx.recv()` → handle_client_message()

## Routing Logic

```rust
// RoutingTable::route_for(destination)
if destination.starts_with(':') → unique_names registry lookup (default Host)
if container_names.contains(dest) → Container
if host_names.contains(dest) → Host
if host_activatable.contains(dest) → Host
else → Host (default)
```

## D-Bus Methods Handled Locally

| Method | Action |
|--------|--------|
| Hello | Return client's `:mux.N` name |
| RequestName | Forward to container, update routing on success |
| ReleaseName | Forward to container, update routing |
| ListNames | Return merged container + host names |
| ListActivatableNames | Return host activatable |
| GetNameOwner | Query bus based on routing |
| NameHasOwner | Check routing table |
| GetId | Return server GUID |
| Other | Forward to container bus |

## Message Flow

**Client → Bus:**
```
ClientHandler.read_message() → client_msg_tx → handle_client_message()
  if is_dbus_daemon_call() → handle_dbus_method()
  else → router.forward_method_call(client_id, msg, route)
    → serial_map.insert((route, new_serial), PendingCall{client_id, client_serial})
    → bus_conn.send(msg)
```

**Bus → Client:**
```
bus_conn.recv() → handle_bus_message(route)
  if is_name_owner_changed() → update routing_table
  if is_method_return/error() → router.handle_reply()
    → serial_map.remove((route, reply_serial)) → get client_id
    → clients[client_id].tx.send(msg)
```

## Key APIs

```rust
// BusConnection
async fn connect(address: &str, name: &str) -> Result<Self>
async fn connect_session(address: Option<&str>, name: &str) -> Result<Self>
async fn send(&self, msg: &Message) -> Result<()>
async fn recv(&self) -> Option<Arc<Message>>
async fn list_names(&self) -> Result<Vec<String>>
async fn get_name_owner(&self, name: &str) -> Result<Option<String>>
fn connection(&self) -> &Connection  // Access underlying zbus

// RoutingTable  
fn route_for(&self, destination: &str) -> Route
fn on_container_name_change(&mut self, name, old_owner, new_owner)
fn on_host_name_change(&mut self, name, old_owner, new_owner)
fn all_names(&self) -> Vec<String>
fn name_has_owner(&self, name: &str) -> bool

// MessageRouter
async fn route_for(&self, destination: &str) -> Route
async fn forward_method_call(&self, client_id, msg, route) -> Result<()>
async fn handle_reply(&self, msg, route) -> Result<Option<(ClientId, u32)>>

// SaslAuth
async fn authenticate_server(&self, stream: &mut UnixStream) -> Result<u32>  // Returns UID
fn guid(&self) -> &str

// MessageExt (trait on Message)
fn destination_str(&self) -> Option<String>
fn member_str(&self) -> Option<String>
fn is_method_call(&self) -> bool
fn is_dbus_daemon_call(&self) -> bool
fn is_name_owner_changed(&self) -> bool
fn serial(&self) -> u32
fn reply_serial(&self) -> Option<u32>

// message.rs helpers
fn parse_name_owner_changed(&Message) -> Result<(String, String, String)>
MethodReturnBuilder::new(request).destination(dest).build(&body) -> Result<Message>
ErrorBuilder::new(request, error_name).destination(dest).build(msg) -> Result<Message>
```

## Error Names (message.rs::error_names)

```rust
FAILED, INVALID_ARGS, UNKNOWN_METHOD, NAME_HAS_NO_OWNER, NO_REPLY, etc.
```

## Dependencies

- zbus 5 (tokio) - D-Bus protocol
- tokio (full) - async runtime
- clap 4 - CLI
- tracing/tracing-subscriber - logging
- thiserror 2 - errors
- uuid 1 - GUID generation
- hex 0.4 - UID encoding
