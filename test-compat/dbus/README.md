# kapsule-dbus-mux Test Suite

This directory contains a C-based test suite for kapsule-dbus-mux that follows the design patterns from [dbus-broker](https://github.com/bus1/dbus-broker)'s test infrastructure.

## Overview

The test suite spawns a backing `dbus-daemon` and then starts `kapsule-dbus-mux` connected to it. Tests then connect to the mux and verify correct D-Bus behavior. This tests the mux as a proxy/multiplexer in a realistic configuration.

## Requirements

- **libsystemd-dev** (Ubuntu/Debian) or **systemd-devel** (Fedora/RHEL)
  - Provides `sd_bus` and `sd_event` APIs
- **dbus** - The backing dbus-daemon
- **kapsule-dbus-mux** - Built from the parent project

### Installing Dependencies

Ubuntu/Debian:
```bash
sudo apt-get install libsystemd-dev dbus
```

Fedora/RHEL:
```bash
sudo dnf install systemd-devel dbus
```

## Building and Running

```bash
# First, build kapsule-dbus-mux from the project root
cd ../..
cargo build

# Return to test directory
cd test-compat/dbus

# Build tests
make

# Run all tests
make test

# Or specify a custom mux path
make test MUX_PATH=/path/to/kapsule-dbus-mux
```

## Test Coverage

| Test | Description |
|------|-------------|
| `hello` | Tests Hello method and unique name assignment |
| `get_id` | Tests GetId method (returns bus ID) |
| `unknown_interface` | Tests error handling for unknown interface |
| `unknown_method` | Tests error handling for unknown method |
| `request_name` | Tests RequestName and ReleaseName |
| `name_has_owner` | Tests NameHasOwner method |
| `list_names` | Tests ListNames method |
| `get_name_owner` | Tests GetNameOwner method |
| `match_rules` | Tests AddMatch and RemoveMatch |
| `signal_delivery` | Tests signal routing between clients |
| `multiple_connections` | Tests multiple simultaneous connections |

## Architecture

```
┌─────────────┐
│   Test      │
│  (sd_bus)   │
└──────┬──────┘
       │ connect
       ▼
┌─────────────────────┐
│  kapsule-dbus-mux   │
│  (under test)       │
└──────┬──────────────┘
       │ upstream
       ▼
┌─────────────────────┐
│    dbus-daemon      │
│  (backing bus)      │
└─────────────────────┘
```

The test harness (`util-broker-mux.h`) manages:
1. Starting a fresh dbus-daemon for each test
2. Starting kapsule-dbus-mux connected to that daemon
3. Waiting for sockets to be ready
4. Connecting test clients via sd_bus
5. Cleanup on test completion

## Differences from dbus-broker Tests

The original dbus-broker test suite has many internal dependencies that prevent standalone compilation. This test suite:

1. Uses a header-only utility (`util-broker-mux.h`) with minimal dependencies
2. Only requires libsystemd
3. Tests proxy/mux behavior rather than full broker behavior
4. Some edge cases may differ (e.g., interface validation)

## Adding New Tests

1. Add a new test function following the pattern:
```c
static void test_my_feature(void) {
    _c_cleanup_(util_broker_freep) Broker *broker = NULL;
    
    TEST_START("my_feature");
    
    util_broker_new(&broker);
    util_broker_spawn(broker);
    
    {
        _c_cleanup_(sd_bus_flush_close_unrefp) sd_bus *bus = NULL;
        util_broker_connect(broker, &bus);
        
        // Test logic here
        // Use TEST_PASS() or TEST_FAIL("reason")
    }
    
    util_broker_terminate(broker);
}
```

2. Call it from `main()`:
```c
test_my_feature();
```

## Troubleshooting

### "Client authentication failed: early eof"

This warning appears because sd_bus sometimes opens multiple connections. It's harmless - the successful connection is used.

### Tests hang waiting for socket

Check that:
- `dbus-daemon` is installed and working
- `kapsule-dbus-mux` binary path is correct
- Socket paths in `/tmp` are writable

### Build errors about missing systemd headers

Install libsystemd-dev:
```bash
sudo apt-get install libsystemd-dev
```
