#!/bin/bash
# Stress tests for kapsule-dbus-mux
#
# Based on research from:
# - D-Bus official test plan (https://dbus.freedesktop.org/doc/dbus-test-plan.html)
# - dbus-broker test suite (https://github.com/bus1/dbus-broker/tree/main/test/dbus)
# - D-Bus specification edge cases
#
# Categories tested:
# 1. Connection/disconnection storms
# 2. Serial number correlation under load
# 3. Name ownership race conditions
# 4. Signal flooding
# 5. Message size limits
# 6. Match rule stress
# 7. Authentication edge cases
# 8. Concurrent client operations
# 9. Malformed input handling
# 10. Long-running stability
#
# Prerequisites:
# - busctl (from systemd)
# - qdbus6 (from Qt6 D-Bus tools)
# - dbus-daemon (for test bus instances)
# - Built kapsule-dbus-mux binary
#
# Usage: ./tests/stress_tests.sh

set -euo pipefail

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Test state
TESTS_RUN=0
TESTS_PASSED=0
TESTS_FAILED=0
TESTS_SKIPPED=0

# Process IDs for cleanup
DBUS_DAEMON_PID=""
MUX_PID=""
BACKGROUND_PIDS=()

# Paths
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
MUX_BINARY="${PROJECT_DIR}/target/debug/kapsule-dbus-mux"
TEST_SOCKET="/tmp/kapsule-stress-test-$$.sock"
CONTAINER_BUS_SOCKET="/tmp/kapsule-stress-container-$$.sock"
CONTAINER_BUS_ADDRESS="unix:path=${CONTAINER_BUS_SOCKET}"
MUX_LOG="/tmp/mux-stress-log-$$.txt"

# Test parameters (can be adjusted for intensity)
CONCURRENT_CLIENTS=${CONCURRENT_CLIENTS:-50}
RAPID_REQUEST_COUNT=${RAPID_REQUEST_COUNT:-100}
NAME_CHURN_COUNT=${NAME_CHURN_COUNT:-50}
SIGNAL_FLOOD_COUNT=${SIGNAL_FLOOD_COUNT:-100}
LONG_RUN_SECONDS=${LONG_RUN_SECONDS:-10}

# Cleanup function
cleanup() {
    echo -e "\n${YELLOW}Cleaning up...${NC}"
    
    # Kill background processes
    for pid in "${BACKGROUND_PIDS[@]:-}"; do
        if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
            kill "$pid" 2>/dev/null || true
            wait "$pid" 2>/dev/null || true
        fi
    done
    
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

log_section() {
    echo ""
    echo -e "${BLUE}========================================${NC}"
    echo -e "${BLUE}$1${NC}"
    echo -e "${BLUE}========================================${NC}"
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

test_skip() {
    local name="$1"
    local reason="${2:-}"
    ((TESTS_SKIPPED++))
    echo -e "  ${YELLOW}⊘${NC} $name (skipped: $reason)"
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
    
    local config_file="/tmp/kapsule-stress-dbus-config-$$.xml"
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
    
    sed -i "s|/tmp/SOCKET_PLACEHOLDER|${CONTAINER_BUS_SOCKET}|g" "$config_file"
    
    dbus-daemon --config-file="$config_file" --print-pid --fork > /tmp/dbus-pid-$$.txt
    DBUS_DAEMON_PID=$(cat /tmp/dbus-pid-$$.txt)
    rm -f /tmp/dbus-pid-$$.txt "$config_file"
    
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
    
    "$MUX_BINARY" \
        --listen "$TEST_SOCKET" \
        --container-bus "$CONTAINER_BUS_ADDRESS" \
        --log-level debug \
        2>"$MUX_LOG" &
    MUX_PID=$!
    
    local retries=30
    while [ ! -S "$TEST_SOCKET" ] && [ $retries -gt 0 ]; do
        sleep 0.1
        ((retries--))
    done
    
    if [ ! -S "$TEST_SOCKET" ]; then
        log_error "Multiplexer failed to start. Log output:"
        cat "$MUX_LOG"
        exit 1
    fi
    
    log_info "Multiplexer started (PID: $MUX_PID)"
}

export_mux_address() {
    export DBUS_SESSION_BUS_ADDRESS="unix:path=$TEST_SOCKET"
    log_info "Exported DBUS_SESSION_BUS_ADDRESS=$DBUS_SESSION_BUS_ADDRESS"
}

# Verify mux is still responding
verify_mux_alive() {
    busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
        org.freedesktop.DBus.Peer Ping &>/dev/null
}

# ============================================================================
# STRESS TEST CATEGORY 1: Connection/Disconnection Storms
# Based on: dbus-broker bench-connect.c
# ============================================================================

test_stress_rapid_connect_disconnect() {
    # Rapidly connect and disconnect clients
    # Tests cleanup code paths when clients vanish
    local success=0
    local total=$CONCURRENT_CLIENTS
    
    for i in $(seq 1 $total); do
        if busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
            org.freedesktop.DBus.Peer Ping &>/dev/null; then
            ((success++))
        fi
    done
    
    # Allow some failures due to timing
    [ "$success" -gt $((total * 90 / 100)) ]
}

test_stress_concurrent_connections() {
    # Many simultaneous connections
    # Based on: dbus-broker test-broker.c test_connect()
    local pids=()
    local total=$CONCURRENT_CLIENTS
    
    for i in $(seq 1 $total); do
        (busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
            org.freedesktop.DBus GetId &>/dev/null) &
        pids+=($!)
    done
    
    local failed=0
    for pid in "${pids[@]}"; do
        wait "$pid" || ((failed++))
    done
    
    # Verify mux still alive after storm
    verify_mux_alive || return 1
    
    # Allow up to 10% failure rate
    [ "$failed" -lt $((total / 10)) ]
}

test_stress_disconnect_during_request() {
    # Disconnect mid-request (tests serial map cleanup)
    # Critical edge case: reply arrives after client gone
    for i in $(seq 1 20); do
        timeout 0.01 busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
            org.freedesktop.DBus ListNames 2>/dev/null || true
    done
    
    # Verify mux still responds
    verify_mux_alive
}

# ============================================================================
# STRESS TEST CATEGORY 2: Serial Number Correlation
# Based on: D-Bus spec serial handling requirements
# ============================================================================

test_stress_serial_exhaustion() {
    # Fire many parallel requests to stress serial correlation
    # Tests: serial_map capacity, correlation under load
    local pids=()
    
    for i in $(seq 1 $RAPID_REQUEST_COUNT); do
        busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
            org.freedesktop.DBus.Peer Ping &>/dev/null &
        pids+=($!)
    done
    
    # Wait for all (don't care about individual failures)
    for pid in "${pids[@]}"; do
        wait "$pid" 2>/dev/null || true
    done
    
    # Verify mux still alive (didn't hang or crash)
    verify_mux_alive
}

test_stress_interleaved_requests() {
    # Multiple clients sending interleaved requests
    # Tests serial correlation doesn't mix up clients
    local pids=()
    
    for client in $(seq 1 10); do
        (
            for req in $(seq 1 20); do
                busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
                    org.freedesktop.DBus GetId &>/dev/null || exit 1
            done
        ) &
        pids+=($!)
    done
    
    local failed=0
    for pid in "${pids[@]}"; do
        wait "$pid" || ((failed++))
    done
    
    [ "$failed" -eq 0 ]
}

# ============================================================================
# STRESS TEST CATEGORY 3: Name Ownership Race Conditions
# Based on: dbus-broker test-driver.c test_request_name()
# ============================================================================

test_stress_name_ownership_race() {
    # Two processes compete for same name simultaneously
    # Tests RoutingTable synchronization
    local test_name="com.kapsule.stress.RaceTest.P$$"
    local pids=()
    
    for i in $(seq 1 5); do
        (
            busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
                org.freedesktop.DBus RequestName su "$test_name" 2 &>/dev/null
        ) &
        pids+=($!)
    done
    
    for pid in "${pids[@]}"; do
        wait "$pid" 2>/dev/null || true
    done
    
    # Cleanup
    busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
        org.freedesktop.DBus ReleaseName s "$test_name" &>/dev/null || true
    
    verify_mux_alive
}

test_stress_rapid_name_churn() {
    # Rapidly request/release names
    # Tests NameOwnerChanged signal handling
    local success=0
    
    for i in $(seq 1 $NAME_CHURN_COUNT); do
        local name="com.kapsule.stress.Churn${i}.P$$"
        if busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
            org.freedesktop.DBus RequestName su "$name" 0 &>/dev/null; then
            busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
                org.freedesktop.DBus ReleaseName s "$name" &>/dev/null
            ((success++))
        fi
    done
    
    [ "$success" -gt $((NAME_CHURN_COUNT * 90 / 100)) ]
}

test_stress_name_queue() {
    # Multiple clients queue for same name
    # Based on: D-Bus spec queued owners
    local test_name="com.kapsule.stress.Queue.P$$"
    local pids=()
    
    # First owner (no replacement allowed)
    busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
        org.freedesktop.DBus RequestName su "$test_name" 0 &>/dev/null
    
    # Queue up more owners
    for i in $(seq 1 5); do
        (
            busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
                org.freedesktop.DBus RequestName su "$test_name" 0 &>/dev/null
            sleep 1  # Keep connection alive briefly
        ) &
        pids+=($!)
    done
    
    sleep 0.2
    
    # Verify ListQueuedOwners works
    busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
        org.freedesktop.DBus ListQueuedOwners s "$test_name" &>/dev/null || return 1
    
    # Cleanup
    for pid in "${pids[@]}"; do
        kill "$pid" 2>/dev/null || true
    done
    
    busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
        org.freedesktop.DBus ReleaseName s "$test_name" &>/dev/null || true
    
    return 0
}

# ============================================================================
# STRESS TEST CATEGORY 4: Signal Flooding
# Based on: dbus-broker test-matches.c
# ============================================================================

test_stress_signal_subscription_flood() {
    # Add many match rules then trigger signals
    # Tests match rule evaluation performance
    local test_name="com.kapsule.stress.Signal.P$$"
    
    # Request a name so we can send signals
    busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
        org.freedesktop.DBus RequestName su "$test_name" 0 &>/dev/null
    
    # Add multiple match rules
    for i in $(seq 1 20); do
        busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
            org.freedesktop.DBus AddMatch s \
            "type='signal',sender='$test_name',member='TestSignal$i'" &>/dev/null || true
    done
    
    # Cleanup
    busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
        org.freedesktop.DBus ReleaseName s "$test_name" &>/dev/null || true
    
    verify_mux_alive
}

test_stress_name_change_signals() {
    # Trigger many NameOwnerChanged signals
    # Tests signal broadcast under load
    for i in $(seq 1 $SIGNAL_FLOOD_COUNT); do
        local name="com.kapsule.stress.SignalTest${i}.P$$"
        busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
            org.freedesktop.DBus RequestName su "$name" 0 &>/dev/null
        busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
            org.freedesktop.DBus ReleaseName s "$name" &>/dev/null
    done
    
    verify_mux_alive
}

# ============================================================================
# STRESS TEST CATEGORY 5: Match Rule Stress
# Based on: dbus-broker test-matches.c test_arg(), test_argpath()
# ============================================================================

test_stress_complex_match_rules() {
    # Add complex match rules with arg matchers
    # Tests match rule parsing and evaluation
    
    busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
        org.freedesktop.DBus AddMatch s \
        "type='signal',interface='org.example',member='Test',arg0='value'" &>/dev/null || true
    
    busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
        org.freedesktop.DBus AddMatch s \
        "type='signal',arg0path='/org/example/'" &>/dev/null || true
    
    busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
        org.freedesktop.DBus AddMatch s \
        "type='signal',arg0namespace='com.example'" &>/dev/null || true
    
    verify_mux_alive
}

test_stress_many_match_rules() {
    # Add many match rules
    # Tests match registry performance
    for i in $(seq 1 50); do
        busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
            org.freedesktop.DBus AddMatch s \
            "type='signal',member='TestMethod$i'" &>/dev/null || true
    done
    
    verify_mux_alive
}

# ============================================================================
# STRESS TEST CATEGORY 6: Error Handling
# Based on: D-Bus spec "Invalid Protocol and Spec Extensions"
# ============================================================================

test_stress_invalid_destinations() {
    # Call non-existent services repeatedly
    # Tests error path performance
    for i in $(seq 1 20); do
        busctl --user call "com.nonexistent.Service${i}.P$$" /SomePath \
            some.Interface SomeMethod 2>&1 || true
    done
    
    verify_mux_alive
}

test_stress_invalid_methods() {
    # Call invalid methods on known service
    for i in $(seq 1 20); do
        busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
            org.freedesktop.DBus "InvalidMethod$i" 2>&1 || true
    done
    
    verify_mux_alive
}

test_stress_invalid_arguments() {
    # Call methods with wrong argument types
    # Tests argument validation
    for i in $(seq 1 10); do
        busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
            org.freedesktop.DBus GetNameOwner i 42 2>&1 || true
        busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
            org.freedesktop.DBus NameHasOwner i 42 2>&1 || true
    done
    
    verify_mux_alive
}

# ============================================================================
# STRESS TEST CATEGORY 7: Long-Running Stability
# Based on: D-Bus test plan "Memory leaks and code robustness"
# ============================================================================

test_stress_long_running_client() {
    # Keep connection open with periodic activity
    # Tests for memory leaks and connection degradation
    local start_time=$(date +%s)
    local end_time=$((start_time + LONG_RUN_SECONDS))
    local count=0
    
    while [ $(date +%s) -lt $end_time ]; do
        busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
            org.freedesktop.DBus.Peer Ping &>/dev/null
        ((count++))
        sleep 0.05
    done
    
    log_info "Completed $count requests over ${LONG_RUN_SECONDS}s"
    verify_mux_alive
}

test_stress_mixed_operations() {
    # Mix of different operations over time
    # Simulates realistic usage patterns
    local start_time=$(date +%s)
    local end_time=$((start_time + LONG_RUN_SECONDS))
    local op=0
    
    while [ $(date +%s) -lt $end_time ]; do
        case $((op % 5)) in
            0)
                busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
                    org.freedesktop.DBus.Peer Ping &>/dev/null || true
                ;;
            1)
                busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
                    org.freedesktop.DBus GetId &>/dev/null || true
                ;;
            2)
                busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
                    org.freedesktop.DBus ListNames &>/dev/null || true
                ;;
            3)
                local name="com.kapsule.stress.Mixed${op}.P$$"
                busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
                    org.freedesktop.DBus RequestName su "$name" 0 &>/dev/null || true
                busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
                    org.freedesktop.DBus ReleaseName s "$name" &>/dev/null || true
                ;;
            4)
                busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
                    org.freedesktop.DBus NameHasOwner s "org.freedesktop.DBus" &>/dev/null || true
                ;;
        esac
        ((op++))
        sleep 0.02
    done
    
    log_info "Completed $op mixed operations over ${LONG_RUN_SECONDS}s"
    verify_mux_alive
}

# ============================================================================
# STRESS TEST CATEGORY 8: D-Bus Method Completeness
# Based on: dbus-broker test-driver.c comprehensive driver API tests
# ============================================================================

test_stress_all_driver_methods() {
    # Exercise all driver methods - each busctl is a separate connection,
    # so we test methods that don't depend on prior state or use
    # org.freedesktop.DBus which is always available.
    
    # Hello is implicit in connection
    
    # ListNames - works without prior state
    busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
        org.freedesktop.DBus ListNames &>/dev/null || return 1
    
    # ListActivatableNames - works without prior state  
    busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
        org.freedesktop.DBus ListActivatableNames &>/dev/null || return 1
    
    # NameHasOwner - test with org.freedesktop.DBus which always exists
    busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
        org.freedesktop.DBus NameHasOwner s "org.freedesktop.DBus" &>/dev/null || return 1
    
    # GetNameOwner - test with org.freedesktop.DBus which always exists
    busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
        org.freedesktop.DBus GetNameOwner s "org.freedesktop.DBus" &>/dev/null || return 1
    
    # GetConnectionUnixUser
    busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
        org.freedesktop.DBus GetConnectionUnixUser s "org.freedesktop.DBus" &>/dev/null || return 1
    
    # GetId
    busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
        org.freedesktop.DBus GetId &>/dev/null || return 1
    
    # Peer.Ping
    busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
        org.freedesktop.DBus.Peer Ping &>/dev/null || return 1
    
    # Peer.GetMachineId
    busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
        org.freedesktop.DBus.Peer GetMachineId &>/dev/null || return 1
    
    # Test RequestName + ReleaseName in one connection isn't possible with busctl,
    # but we can test that RequestName succeeds (name released on disconnect)
    local test_name="com.kapsule.stress.AllMethods.P$$"
    busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
        org.freedesktop.DBus RequestName su "$test_name" 0 &>/dev/null || return 1
    
    # AddMatch (connection closes, match is removed automatically)
    busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
        org.freedesktop.DBus AddMatch s "type='signal'" &>/dev/null || return 1
    
    # ListQueuedOwners with org.freedesktop.DBus (always exists)
    busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
        org.freedesktop.DBus ListQueuedOwners s "org.freedesktop.DBus" &>/dev/null || return 1
    
    return 0
}

# ============================================================================
# STRESS TEST CATEGORY 9: Resource Exhaustion Simulation
# Based on: D-Bus test plan "Out-Of-Memory robustness"
# ============================================================================

test_stress_max_names_per_client() {
    # Request many names from single client
    # Tests per-client resource limits
    local names=()
    local success=0
    
    for i in $(seq 1 100); do
        local name="com.kapsule.stress.MaxNames${i}.P$$"
        if busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
            org.freedesktop.DBus RequestName su "$name" 0 &>/dev/null; then
            names+=("$name")
            ((success++))
        fi
    done
    
    # Cleanup
    for name in "${names[@]}"; do
        busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
            org.freedesktop.DBus ReleaseName s "$name" &>/dev/null || true
    done
    
    verify_mux_alive && [ "$success" -gt 50 ]
}

test_stress_max_match_rules_per_client() {
    # Add many match rules from single client
    local success=0
    
    for i in $(seq 1 100); do
        if busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
            org.freedesktop.DBus AddMatch s \
            "type='signal',member='StressMatch$i'" &>/dev/null; then
            ((success++))
        fi
    done
    
    verify_mux_alive && [ "$success" -gt 50 ]
}

# ============================================================================
# Main
# ============================================================================

main() {
    echo "========================================"
    echo "kapsule-dbus-mux Stress Tests"
    echo "========================================"
    echo ""
    echo "Test Parameters:"
    echo "  CONCURRENT_CLIENTS=$CONCURRENT_CLIENTS"
    echo "  RAPID_REQUEST_COUNT=$RAPID_REQUEST_COUNT"
    echo "  NAME_CHURN_COUNT=$NAME_CHURN_COUNT"
    echo "  SIGNAL_FLOOD_COUNT=$SIGNAL_FLOOD_COUNT"
    echo "  LONG_RUN_SECONDS=$LONG_RUN_SECONDS"
    echo ""
    
    check_prerequisites
    
    # Start test infrastructure
    start_test_dbus_daemon
    start_multiplexer
    export_mux_address
    
    # Give everything a moment to settle
    sleep 0.5
    
    log_section "Connection/Disconnection Storms"
    run_test "Rapid connect/disconnect" test_stress_rapid_connect_disconnect || true
    run_test "Concurrent connections ($CONCURRENT_CLIENTS clients)" test_stress_concurrent_connections || true
    run_test "Disconnect during request" test_stress_disconnect_during_request || true
    
    log_section "Serial Number Correlation"
    run_test "Serial exhaustion ($RAPID_REQUEST_COUNT parallel)" test_stress_serial_exhaustion || true
    run_test "Interleaved requests (10 clients x 20 req)" test_stress_interleaved_requests || true
    
    log_section "Name Ownership Race Conditions"
    run_test "Name ownership race" test_stress_name_ownership_race || true
    run_test "Rapid name churn ($NAME_CHURN_COUNT names)" test_stress_rapid_name_churn || true
    run_test "Name queue stress" test_stress_name_queue || true
    
    log_section "Signal & Match Rule Stress"
    run_test "Signal subscription flood" test_stress_signal_subscription_flood || true
    run_test "Name change signal flood ($SIGNAL_FLOOD_COUNT)" test_stress_name_change_signals || true
    run_test "Complex match rules" test_stress_complex_match_rules || true
    run_test "Many match rules (50)" test_stress_many_match_rules || true
    
    log_section "Error Handling"
    run_test "Invalid destinations (20x)" test_stress_invalid_destinations || true
    run_test "Invalid methods (20x)" test_stress_invalid_methods || true
    run_test "Invalid arguments (10x)" test_stress_invalid_arguments || true
    
    log_section "Long-Running Stability"
    run_test "Long-running client (${LONG_RUN_SECONDS}s)" test_stress_long_running_client || true
    run_test "Mixed operations (${LONG_RUN_SECONDS}s)" test_stress_mixed_operations || true
    
    log_section "D-Bus Method Completeness"
    run_test "All driver methods" test_stress_all_driver_methods || true
    
    log_section "Resource Exhaustion"
    run_test "Max names per client (100)" test_stress_max_names_per_client || true
    run_test "Max match rules per client (100)" test_stress_max_match_rules_per_client || true
    
    # Print summary
    echo ""
    echo "========================================"
    echo "Stress Test Summary"
    echo "========================================"
    echo -e "Total:   ${TESTS_RUN}"
    echo -e "Passed:  ${GREEN}${TESTS_PASSED}${NC}"
    echo -e "Failed:  ${RED}${TESTS_FAILED}${NC}"
    echo -e "Skipped: ${YELLOW}${TESTS_SKIPPED}${NC}"
    echo ""
    
    if [ "$TESTS_FAILED" -gt 0 ]; then
        echo -e "${RED}Some stress tests failed!${NC}"
        echo ""
        echo "Multiplexer log (last 100 lines):"
        echo "----------------------------------------"
        tail -100 "$MUX_LOG" 2>/dev/null || echo "(no log available)"
        exit 1
    else
        echo -e "${GREEN}All stress tests passed!${NC}"
        exit 0
    fi
}

main "$@"
