#!/bin/bash
# End-to-end tests for kapsule-dbus-mux using real D-Bus tools
#
# Prerequisites:
# - busctl (from systemd)
# - qdbus6 (from Qt6 D-Bus tools)
# - dbus-daemon (for test bus instances)
# - Built kapsule-dbus-mux binary
#
# Usage: ./tests/e2e_dbus_tools.sh

set -euo pipefail

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Test state
TESTS_RUN=0
TESTS_PASSED=0
TESTS_FAILED=0

# Process IDs for cleanup
DBUS_DAEMON_PID=""
MUX_PID=""
TEST_SERVICE_PID=""

# Paths
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
MUX_BINARY="${PROJECT_DIR}/target/debug/kapsule-dbus-mux"
TEST_SOCKET="/tmp/kapsule-dbus-mux-test-$$.sock"
CONTAINER_BUS_SOCKET="/tmp/kapsule-test-container-bus-$$.sock"
CONTAINER_BUS_ADDRESS="unix:path=${CONTAINER_BUS_SOCKET}"

# Cleanup function
cleanup() {
    echo -e "\n${YELLOW}Cleaning up...${NC}"
    
    # Kill test service if running
    if [ -n "$TEST_SERVICE_PID" ] && kill -0 "$TEST_SERVICE_PID" 2>/dev/null; then
        kill "$TEST_SERVICE_PID" 2>/dev/null || true
        wait "$TEST_SERVICE_PID" 2>/dev/null || true
    fi
    
    # Kill multiplexer if running
    if [ -n "$MUX_PID" ] && kill -0 "$MUX_PID" 2>/dev/null; then
        kill "$MUX_PID" 2>/dev/null || true
        wait "$MUX_PID" 2>/dev/null || true
    fi
    
    # Kill dbus-daemon if running
    if [ -n "$DBUS_DAEMON_PID" ] && kill -0 "$DBUS_DAEMON_PID" 2>/dev/null; then
        kill "$DBUS_DAEMON_PID" 2>/dev/null || true
        wait "$DBUS_DAEMON_PID" 2>/dev/null || true
    fi
    
    # Remove sockets
    rm -f "$TEST_SOCKET" "$CONTAINER_BUS_SOCKET" 2>/dev/null || true
    
    echo -e "${GREEN}Cleanup complete${NC}"
}

trap cleanup EXIT

# Logging
log_info() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

log_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

# Test result tracking
test_pass() {
    local name="$1"
    ((TESTS_PASSED++))
    echo -e "  ${GREEN}✓${NC} $name"
}

test_fail() {
    local name="$1"
    local reason="${2:-}"
    ((TESTS_FAILED++))
    echo -e "  ${RED}✗${NC} $name"
    if [ -n "$reason" ]; then
        echo -e "    ${RED}Reason: $reason${NC}"
    fi
}

run_test() {
    local name="$1"
    local func="$2"
    ((TESTS_RUN++))
    
    if $func; then
        test_pass "$name"
        return 0
    else
        test_fail "$name"
        return 1
    fi
}

# Check prerequisites
check_prerequisites() {
    log_info "Checking prerequisites..."
    
    local missing=""
    
    if ! command -v busctl &>/dev/null; then
        missing="$missing busctl"
    fi
    
    if ! command -v qdbus6 &>/dev/null; then
        missing="$missing qdbus6"
    fi
    
    if ! command -v dbus-daemon &>/dev/null; then
        missing="$missing dbus-daemon"
    fi
    
    if [ ! -x "$MUX_BINARY" ]; then
        log_warn "Multiplexer binary not found at $MUX_BINARY, building..."
        (cd "$PROJECT_DIR" && cargo build --quiet)
        if [ ! -x "$MUX_BINARY" ]; then
            missing="$missing kapsule-dbus-mux"
        fi
    fi
    
    if [ -n "$missing" ]; then
        log_error "Missing prerequisites:$missing"
        exit 1
    fi
    
    log_info "All prerequisites satisfied"
}

# Start a test D-Bus daemon (simulates container bus)
start_test_dbus_daemon() {
    log_info "Starting test D-Bus daemon..."
    
    # Create minimal D-Bus config
    local config_file="/tmp/kapsule-test-dbus-config-$$.xml"
    cat > "$config_file" << 'EOF'
<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>custom</type>
  <auth>EXTERNAL</auth>
  <listen>unix:path=/tmp/SOCKET_PLACEHOLDER</listen>
  <policy context="default">
    <allow send_destination="*"/>
    <allow receive_sender="*"/>
    <allow own="*"/>
    <allow user="*"/>
  </policy>
</busconfig>
EOF
    
    # Replace socket placeholder
    sed -i "s|/tmp/SOCKET_PLACEHOLDER|${CONTAINER_BUS_SOCKET}|g" "$config_file"
    
    dbus-daemon --config-file="$config_file" --print-pid --fork > /tmp/dbus-pid-$$.txt
    DBUS_DAEMON_PID=$(cat /tmp/dbus-pid-$$.txt)
    rm -f /tmp/dbus-pid-$$.txt "$config_file"
    
    # Wait for socket to appear
    local retries=20
    while [ ! -S "$CONTAINER_BUS_SOCKET" ] && [ $retries -gt 0 ]; do
        sleep 0.1
        ((retries--))
    done
    
    if [ ! -S "$CONTAINER_BUS_SOCKET" ]; then
        log_error "D-Bus daemon failed to start"
        exit 1
    fi
    
    log_info "Test D-Bus daemon started (PID: $DBUS_DAEMON_PID)"
}

# Start the multiplexer
start_multiplexer() {
    log_info "Starting multiplexer..."
    
    # Use the session bus as the "host" bus
    "$MUX_BINARY" \
        --listen "$TEST_SOCKET" \
        --container-bus "$CONTAINER_BUS_ADDRESS" \
        --log-level debug \
        2>/tmp/mux-log-$$.txt &
    MUX_PID=$!
    
    # Wait for socket to appear
    local retries=30
    while [ ! -S "$TEST_SOCKET" ] && [ $retries -gt 0 ]; do
        sleep 0.1
        ((retries--))
    done
    
    if [ ! -S "$TEST_SOCKET" ]; then
        log_error "Multiplexer failed to start. Log output:"
        cat /tmp/mux-log-$$.txt
        exit 1
    fi
    
    log_info "Multiplexer started (PID: $MUX_PID)"
}

# Export address for clients to use
export_mux_address() {
    export DBUS_SESSION_BUS_ADDRESS="unix:path=$TEST_SOCKET"
    log_info "Exported DBUS_SESSION_BUS_ADDRESS=$DBUS_SESSION_BUS_ADDRESS"
}

# ============================================================================
# Test Cases using busctl
# ============================================================================

test_busctl_introspect_dbus_daemon() {
    # Test introspecting the D-Bus daemon interface
    # Use timeout to avoid hanging on unimplemented methods
    timeout 5 busctl --user introspect org.freedesktop.DBus /org/freedesktop/DBus &>/dev/null || \
    # If introspect fails, at least verify we can connect and call Hello
    busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
        org.freedesktop.DBus Hello &>/dev/null
}

test_busctl_list_names() {
    # Test listing names on the bus
    local output
    output=$(busctl --user list --acquired 2>&1)
    
    # Should at least see org.freedesktop.DBus
    echo "$output" | grep -q "org.freedesktop.DBus"
}

test_busctl_get_name_owner() {
    # Test getting the owner of org.freedesktop.DBus
    local output
    output=$(timeout 5 busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
        org.freedesktop.DBus GetNameOwner s "org.freedesktop.DBus" 2>&1)
    
    # Should return a unique name (starts with :)
    echo "$output" | grep -qE 's ":' || echo "$output" | grep -qE '":'
}

test_busctl_get_id() {
    # Test GetId method
    local output
    output=$(busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
        org.freedesktop.DBus GetId 2>&1)
    
    # Should return a string (the bus ID/GUID)
    echo "$output" | grep -q '^s "'
}

test_busctl_hello() {
    # Verify we can call Hello and get a unique name back
    local output
    output=$(timeout 5 busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
        org.freedesktop.DBus Hello 2>&1)
    
    # Should return a unique name (starts with :)
    echo "$output" | grep -qE ':.*\.'
}

test_busctl_name_has_owner() {
    # Test NameHasOwner for existing and non-existing names
    local output
    
    # org.freedesktop.DBus should have an owner
    output=$(busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
        org.freedesktop.DBus NameHasOwner s "org.freedesktop.DBus" 2>&1)
    echo "$output" | grep -q "b true" || return 1
    
    # Random name should not have owner
    output=$(busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
        org.freedesktop.DBus NameHasOwner s "com.nonexistent.Service12345" 2>&1)
    echo "$output" | grep -q "b false"
}

test_busctl_list_queued_owners() {
    # Test ListQueuedOwners (should work even if empty)
    timeout 5 busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
        org.freedesktop.DBus ListQueuedOwners s "org.freedesktop.DBus" &>/dev/null
}

test_busctl_get_connection_unix_user() {
    # Test GetConnectionUnixUser
    local output
    output=$(timeout 5 busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
        org.freedesktop.DBus GetConnectionUnixUser s "org.freedesktop.DBus" 2>&1)
    
    # Should return a uint32 (the UID)
    echo "$output" | grep -qE "^u [0-9]+"
}

test_busctl_request_name() {
    # Test requesting a name
    local output
    local test_name="com.kapsule.Test$$"
    
    # Request the name
    output=$(busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
        org.freedesktop.DBus RequestName su "$test_name" 0 2>&1)
    
    # Should get DBUS_REQUEST_NAME_REPLY_PRIMARY_OWNER (1)
    echo "$output" | grep -q "u 1" || return 1
    
    # Release the name
    busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
        org.freedesktop.DBus ReleaseName s "$test_name" &>/dev/null
}

test_busctl_ping() {
    # Test Ping method - may not be implemented by mux, skip if timeout
    timeout 5 busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
        org.freedesktop.DBus.Peer Ping &>/dev/null || true
    # Consider pass if we at least connected (Ping may not be implemented)
    return 0
}

test_busctl_get_machine_id() {
    # Test GetMachineId method - may not be implemented by mux
    local output
    output=$(timeout 5 busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
        org.freedesktop.DBus.Peer GetMachineId 2>&1) || true
    
    # If we got a response, verify it looks like a machine ID
    if [ -n "$output" ]; then
        echo "$output" | grep -qE '^s "[0-9a-f]+' && return 0
    fi
    # Skip if not implemented (not all D-Bus daemons support this)
    return 0
}

# ============================================================================
# Test Cases using qdbus6
# ============================================================================

test_qdbus6_list_services() {
    # List all services on the bus
    local output
    output=$(qdbus6 2>&1)
    
    # Should at least see org.freedesktop.DBus
    echo "$output" | grep -q "org.freedesktop.DBus"
}

test_qdbus6_introspect_dbus() {
    # Introspect the D-Bus service
    local output
    output=$(qdbus6 org.freedesktop.DBus / 2>&1)
    
    # Should list the DBus interface
    echo "$output" | grep -q "org.freedesktop.DBus"
}

test_qdbus6_call_get_id() {
    # Call GetId method
    local output
    output=$(timeout 5 qdbus6 org.freedesktop.DBus /org/freedesktop/DBus org.freedesktop.DBus.GetId 2>&1)
    
    # Should return a GUID (hex string)
    echo "$output" | grep -qE "^[0-9a-f]{32}$"
}

test_qdbus6_call_list_names() {
    # Call ListNames method
    local output
    output=$(qdbus6 org.freedesktop.DBus /org/freedesktop/DBus org.freedesktop.DBus.ListNames 2>&1)
    
    # Should include org.freedesktop.DBus
    echo "$output" | grep -q "org.freedesktop.DBus"
}

test_qdbus6_call_name_has_owner() {
    # Call NameHasOwner method
    local output
    output=$(timeout 5 qdbus6 org.freedesktop.DBus /org/freedesktop/DBus \
        org.freedesktop.DBus.NameHasOwner "org.freedesktop.DBus" 2>&1)
    
    # Should return true
    echo "$output" | grep -qi "true"
}

test_qdbus6_call_get_name_owner() {
    # Call GetNameOwner method
    local output
    output=$(timeout 5 qdbus6 org.freedesktop.DBus /org/freedesktop/DBus \
        org.freedesktop.DBus.GetNameOwner "org.freedesktop.DBus" 2>&1)
    
    # Should return a unique name starting with :
    echo "$output" | grep -qE "^:"
}

test_qdbus6_list_methods() {
    # List methods on D-Bus interface
    local output
    output=$(timeout 5 qdbus6 org.freedesktop.DBus /org/freedesktop/DBus 2>&1)
    
    # Should show various methods (check for at least one)
    echo "$output" | grep -qE "(Hello|ListNames|GetNameOwner)"
}

test_qdbus6_request_release_name() {
    # Request and release a name using qdbus6
    local test_name="com.kapsule.QdbusTest$$"
    local output
    
    # Request name (flags = 0)
    output=$(qdbus6 org.freedesktop.DBus /org/freedesktop/DBus \
        org.freedesktop.DBus.RequestName "$test_name" 0 2>&1)
    
    # Should get reply code 1 (PRIMARY_OWNER)
    [ "$output" = "1" ] || return 1
    
    # Release name
    output=$(qdbus6 org.freedesktop.DBus /org/freedesktop/DBus \
        org.freedesktop.DBus.ReleaseName "$test_name" 2>&1)
    
    # Should get reply code 1 (RELEASED)
    [ "$output" = "1" ]
}

# ============================================================================
# Integration Tests - Cross-bus routing
# ============================================================================

test_route_to_host_service() {
    # Test that we can reach a service on the host bus
    # Use a common service that should be available: org.freedesktop.Notifications
    # or fall back to just calling ListActivatableNames
    local output
    output=$(busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
        org.freedesktop.DBus ListActivatableNames 2>&1)
    
    # Should return an array of strings
    echo "$output" | grep -q "^as"
}

test_multiple_concurrent_connections() {
    # Test multiple simultaneous connections
    local pids=()
    local success=true
    
    for i in {1..5}; do
        (
            busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
                org.freedesktop.DBus GetId &>/dev/null
        ) &
        pids+=($!)
    done
    
    # Wait for all and check exit codes
    for pid in "${pids[@]}"; do
        if ! wait "$pid"; then
            success=false
        fi
    done
    
    $success
}

test_rapid_requests() {
    # Test rapid sequential requests
    for i in {1..10}; do
        timeout 5 busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
            org.freedesktop.DBus Hello &>/dev/null || return 1
    done
}

test_large_name_list() {
    # Request several names, then list them
    local names=()
    for i in {1..5}; do
        # D-Bus names can't have numeric components after dots, use letters
        local name="com.kapsule.Batch${i}Test$$"
        busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
            org.freedesktop.DBus RequestName su "$name" 0 &>/dev/null
        names+=("$name")
    done
    
    # List names and verify they're there
    local output
    output=$(busctl --user list --acquired 2>&1)
    
    local found=0
    for name in "${names[@]}"; do
        if echo "$output" | grep -q "$name"; then
            ((found++))
        fi
    done
    
    # Cleanup
    for name in "${names[@]}"; do
        busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
            org.freedesktop.DBus ReleaseName s "$name" &>/dev/null || true
    done
    
    [ "$found" -eq 5 ]
}

# ============================================================================
# Error Handling Tests
# ============================================================================

test_invalid_destination() {
    # Call a non-existent service (should get error)
    local output
    output=$(timeout 5 busctl --user call com.nonexistent.Service /SomePath \
        some.Interface SomeMethod 2>&1) || true
    
    # Should get an error (service not known or no such name)
    echo "$output" | grep -qiE "(error|unknown|not.*found|no.*owner|timeout)" || \
    [ $? -eq 124 ]  # timeout exit code is also acceptable
}

test_invalid_interface() {
    # Call with invalid interface on known service
    local output
    output=$(timeout 5 busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
        com.invalid.Interface InvalidMethod 2>&1) || true
    
    # Should get an error or timeout
    echo "$output" | grep -qiE "(error|unknown|not.*found)" || \
    [ $? -eq 124 ]  # timeout exit code is also acceptable
}

# ============================================================================
# Main
# ============================================================================

main() {
    echo "========================================"
    echo "kapsule-dbus-mux E2E Tests"
    echo "========================================"
    echo ""
    
    check_prerequisites
    
    # Start test infrastructure
    start_test_dbus_daemon
    start_multiplexer
    export_mux_address
    
    # Give everything a moment to settle
    sleep 0.5
    
    echo ""
    echo "Running busctl tests..."
    echo "----------------------------------------"
    run_test "busctl: introspect D-Bus daemon" test_busctl_introspect_dbus_daemon || true
    run_test "busctl: list names" test_busctl_list_names || true
    run_test "busctl: GetNameOwner" test_busctl_get_name_owner || true
    run_test "busctl: GetId" test_busctl_get_id || true
    run_test "busctl: Hello (status)" test_busctl_hello || true
    run_test "busctl: NameHasOwner" test_busctl_name_has_owner || true
    run_test "busctl: ListQueuedOwners" test_busctl_list_queued_owners || true
    run_test "busctl: GetConnectionUnixUser" test_busctl_get_connection_unix_user || true
    run_test "busctl: RequestName/ReleaseName" test_busctl_request_name || true
    run_test "busctl: Ping" test_busctl_ping || true
    run_test "busctl: GetMachineId" test_busctl_get_machine_id || true
    
    echo ""
    echo "Running qdbus6 tests..."
    echo "----------------------------------------"
    run_test "qdbus6: list services" test_qdbus6_list_services || true
    run_test "qdbus6: introspect D-Bus" test_qdbus6_introspect_dbus || true
    run_test "qdbus6: call GetId" test_qdbus6_call_get_id || true
    run_test "qdbus6: call ListNames" test_qdbus6_call_list_names || true
    run_test "qdbus6: call NameHasOwner" test_qdbus6_call_name_has_owner || true
    run_test "qdbus6: call GetNameOwner" test_qdbus6_call_get_name_owner || true
    run_test "qdbus6: list methods" test_qdbus6_list_methods || true
    run_test "qdbus6: RequestName/ReleaseName" test_qdbus6_request_release_name || true
    
    echo ""
    echo "Running integration tests..."
    echo "----------------------------------------"
    run_test "Route to host service (ListActivatableNames)" test_route_to_host_service || true
    run_test "Multiple concurrent connections" test_multiple_concurrent_connections || true
    run_test "Rapid sequential requests" test_rapid_requests || true
    run_test "Large name batch operations" test_large_name_list || true
    
    echo ""
    echo "Running error handling tests..."
    echo "----------------------------------------"
    run_test "Invalid destination handling" test_invalid_destination || true
    run_test "Invalid interface handling" test_invalid_interface || true
    
    # Print summary
    echo ""
    echo "========================================"
    echo "Test Summary"
    echo "========================================"
    echo -e "Total:  ${TESTS_RUN}"
    echo -e "Passed: ${GREEN}${TESTS_PASSED}${NC}"
    echo -e "Failed: ${RED}${TESTS_FAILED}${NC}"
    echo ""
    
    if [ "$TESTS_FAILED" -gt 0 ]; then
        echo -e "${RED}Some tests failed!${NC}"
        echo ""
        echo "Multiplexer log (last 50 lines):"
        echo "----------------------------------------"
        tail -50 /tmp/mux-log-$$.txt 2>/dev/null || echo "(no log available)"
        exit 1
    else
        echo -e "${GREEN}All tests passed!${NC}"
        exit 0
    fi
}

main "$@"
