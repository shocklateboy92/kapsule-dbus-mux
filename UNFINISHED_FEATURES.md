# Unfinished Features and Technical Debt

This document tracks incomplete features, stub implementations, and potential improvements identified in the kapsule-dbus-mux codebase.

## Unfinished Features

### 1. Name Queue Management

**Status:** Not implemented

**Description:** The D-Bus specification supports name ownership queuing, where multiple clients can request the same name and be placed in a queue. The mux currently forwards `RequestName`/`ReleaseName` to the container bus but doesn't manage a unified queue across both buses.

**What's missing:**
- Local tracking of name ownership queues
- Proper handling of `RequestName` flags:
  - `ALLOW_REPLACEMENT` (0x1) - Client allows being replaced
  - `REPLACE_EXISTING` (0x2) - Try to replace current owner
  - `DO_NOT_QUEUE` (0x4) - Don't wait in queue if name taken
- Proper handling of reply codes:
  - `IN_QUEUE` (2) - Client is queued waiting for name
  - `EXISTS` (3) - Name exists, DO_NOT_QUEUE was set
  - `NON_EXISTENT` (2) - Name doesn't exist (for ReleaseName)
  - `NOT_OWNER` (3) - Caller doesn't own the name

**Impact:** `ListQueuedOwners` returns a stub response (see `src/multiplexer.rs` around line 800).

**Recommendation:** For most container use cases, this is low priority. Name queuing is rarely used by applications. Consider implementing only if real-world usage requires it.

---

### 2. Match Rule Reference Counting

**Status:** Partial implementation

**Location:** `src/multiplexer.rs` in `handle_add_match` and `handle_remove_match`

**Description:** Match rules are added to both upstream buses when a client calls `AddMatch`, but they're never removed from the buses when clients call `RemoveMatch` or disconnect.

**Current behavior:**
```rust
// Note: We don't remove from the actual buses because other clients
// might still be using the same match rule. A production implementation
// would ref-count match rules across clients.
```

**What's missing:**
- Reference counting of match rules across all clients
- Removal from upstream buses when refcount reaches zero
- Cleanup of match rules when clients disconnect

**Impact:** Potential resource leak over long-running sessions with many client connections.

**Recommendation:** Medium priority. Implement refcounting for match rules to prevent resource leaks.

---

### 3. Periodic Cleanup of Expired Pending Calls

**Status:** Not implemented

**Description:** `SerialMap::cleanup_expired()` exists but is never called periodically.

**What's missing:**
- A background task that periodically calls `cleanup_expired()`
- Configuration for cleanup interval

**Impact:** Long-running instances could accumulate stale pending call entries if replies are never received (e.g., destination crashed).

**Recommendation:** Low priority. Add a periodic cleanup task (e.g., every 30 seconds).

---

### 4. Credential Lookup for Host Bus Names

**Status:** Partial implementation

**Location:** `src/multiplexer.rs` in `handle_get_connection_credentials`

**Description:** When looking up credentials for a name, the mux checks local clients first, then forwards to the container bus. However, for names owned on the host bus, this wouldn't return correct credentials.

**What's missing:**
- Query host bus for credentials when the name is routed to host
- Proper routing decision for credential lookups

**Recommendation:** Medium priority if applications rely on credential checks for host services.

---

## Not Implemented (By Design)

These D-Bus daemon methods are not implemented and would require significant work:

| Method | Description | Priority |
|--------|-------------|----------|
| `StartServiceByName` | Activate services on demand | Low - containers typically pre-start services |
| `UpdateActivationEnvironment` | Set environment for activated services | Low |
| `GetAdtAuditSessionData` | Security auditing | Low |
| `GetConnectionSELinuxSecurityContext` | SELinux context retrieval | Low |
| `BecomeMonitor` | D-Bus monitoring mode | Low |
| `ReloadConfig` | Reload daemon configuration | N/A - mux has no config file |

---

## Removed Dead Code (Reference)

The following dead code was removed and is documented here for reference:

### Superseded Designs (Not Bugs)

1. **`ClientHandler.rx` field** - An unused receive channel. Messages to clients go through `ClientInfo.tx` in the multiplexer instead.

2. **`forward_signal()` in router.rs** - Signal forwarding was implemented directly in multiplexer.rs using `ClientMatchRules` instead of this router function.

3. **`RoutedMessage` struct in router.rs** - Part of an earlier design that was superseded.

### Removed Constants

These constants were defined but never used (related to unfinished name queuing feature above):

```rust
// request_name_flags (entire module removed)
ALLOW_REPLACEMENT: u32 = 0x1
REPLACE_EXISTING: u32 = 0x2
DO_NOT_QUEUE: u32 = 0x4

// request_name_reply (unused constants)
IN_QUEUE: u32 = 2
EXISTS: u32 = 3

// release_name_reply (unused constants)
NON_EXISTENT: u32 = 2
NOT_OWNER: u32 = 3
```

---

## Testing Gaps

The plan document mentioned these tests that don't exist:

- `routing_tests.rs` - Unit tests for routing logic
- `integration_tests.rs` - Full integration tests

Current tests focus on:
- Match rule parsing (`match_rules_tests.rs`)
- Auth integration (`auth_integration.rs`)
- Credential tests (`credential_tests.rs`)
- Shell-based stress/e2e tests
