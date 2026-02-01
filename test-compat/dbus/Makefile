# Makefile for kapsule-dbus-mux test suite
#
# Usage:
#   make                    # Build all tests
#   make test               # Run tests against kapsule-dbus-mux
#   make clean              # Clean build artifacts
#
# Environment:
#   DBUS_BROKER_TEST_MUX    # Path to kapsule-dbus-mux binary (auto-detected)
#   CC                      # C compiler (default: gcc)
#   CFLAGS                  # Additional compiler flags
#
# Requirements:
#   - libsystemd-dev (for sd-bus)
#   - dbus-daemon (backing bus)
#   - kapsule-dbus-mux binary

CC ?= gcc
CFLAGS ?= -O2 -g -Wall -Wextra

# Required packages: libsystemd-dev
PKG_CFLAGS := $(shell pkg-config --cflags libsystemd 2>/dev/null)
PKG_LIBS := $(shell pkg-config --libs libsystemd 2>/dev/null)

ifeq ($(PKG_LIBS),)
$(error libsystemd not found. Install libsystemd-dev)
endif

COMMON_CFLAGS := $(CFLAGS) $(PKG_CFLAGS) -pthread
COMMON_LDFLAGS := $(PKG_LIBS) -pthread

# Test executables
TEST_BINS := test-mux-driver

# Default MUX path (can be overridden)
MUX_PATH ?= $(shell pwd)/../../target/debug/kapsule-dbus-mux

.PHONY: all clean test check help

all: $(TEST_BINS)

# Build test executable (header-only utility, no separate object file needed)
test-mux-driver: test-mux-driver.c util-broker-mux.h
	$(CC) $(COMMON_CFLAGS) $< $(COMMON_LDFLAGS) -o $@

# Run tests
test: all
	@echo "=========================================="
	@echo "Running kapsule-dbus-mux test suite"
	@echo "=========================================="
	@echo ""
	@if [ ! -x "$(MUX_PATH)" ]; then \
		echo "ERROR: kapsule-dbus-mux not found at $(MUX_PATH)"; \
		echo ""; \
		echo "Build it first:"; \
		echo "  cd ../.. && cargo build"; \
		echo ""; \
		echo "Or specify path:"; \
		echo "  make test MUX_PATH=/path/to/kapsule-dbus-mux"; \
		exit 1; \
	fi
	@echo "Using MUX binary: $(MUX_PATH)"
	@echo ""
	DBUS_BROKER_TEST_MUX="$(MUX_PATH)" ./test-mux-driver

check: test

clean:
	rm -f $(TEST_BINS)
	rm -f /tmp/dbus-mux-*.sock /tmp/dbus-backing-*.sock
	rm -rf /tmp/dbus-mux-test-*

help:
	@echo "kapsule-dbus-mux Test Suite"
	@echo ""
	@echo "This test suite tests kapsule-dbus-mux by running it as a proxy"
	@echo "in front of a backing dbus-daemon, similar to how dbus-broker's"
	@echo "test suite operates."
	@echo ""
	@echo "Targets:"
	@echo "  all      - Build test executables"
	@echo "  test     - Build and run all tests"
	@echo "  check    - Same as test"
	@echo "  clean    - Remove build artifacts and temp files"
	@echo "  help     - Show this help"
	@echo ""
	@echo "Options:"
	@echo "  MUX_PATH - Path to kapsule-dbus-mux binary"
	@echo "             (default: ../../target/debug/kapsule-dbus-mux)"
	@echo ""
	@echo "Requirements:"
	@echo "  - libsystemd-dev (Ubuntu/Debian) or systemd-devel (Fedora/RHEL)"
	@echo "  - dbus-daemon"
	@echo "  - kapsule-dbus-mux built from parent project"
	@echo ""
	@echo "Example:"
	@echo "  # Build kapsule-dbus-mux first"
	@echo "  cd ../.. && cargo build"
	@echo "  cd test-compat/dbus"
	@echo ""
	@echo "  # Run tests"
	@echo "  make test"
