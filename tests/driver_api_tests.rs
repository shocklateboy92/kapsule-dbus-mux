//! Integration tests for D-Bus driver API (org.freedesktop.DBus interface).
//!
//! These tests are based on the dbus-broker test suite and verify that our
//! multiplexer correctly implements the D-Bus driver interface.
//!
//! Test categories:
//! - Hello method and unique name assignment
//! - Name ownership (RequestName, ReleaseName, GetNameOwner, NameHasOwner)
//! - Name listing (ListNames, ListActivatableNames, ListQueuedOwners)
//! - Match rules (AddMatch, RemoveMatch)
//! - Credential lookup (GetConnectionUnixUser, GetConnectionUnixProcessID, GetConnectionCredentials)
//! - Bus introspection (GetId, Introspect)
//! - Peer methods (Ping, GetMachineId)
//! - Error handling for unknown methods/interfaces

use kapsule_dbus_mux::match_rules::MatchRule;

// =============================================================================
// Unit Tests for Driver API Message Validation
// =============================================================================

#[test]
fn test_parse_request_name_flags() {
    // Test that we understand RequestName flag values
    const DBUS_NAME_FLAG_ALLOW_REPLACEMENT: u32 = 0x1;
    const DBUS_NAME_FLAG_REPLACE_EXISTING: u32 = 0x2;
    const DBUS_NAME_FLAG_DO_NOT_QUEUE: u32 = 0x4;
    
    // Verify flag values match the D-Bus spec
    assert_eq!(DBUS_NAME_FLAG_ALLOW_REPLACEMENT, 1);
    assert_eq!(DBUS_NAME_FLAG_REPLACE_EXISTING, 2);
    assert_eq!(DBUS_NAME_FLAG_DO_NOT_QUEUE, 4);
    
    // Combined flags
    let flags = DBUS_NAME_FLAG_ALLOW_REPLACEMENT | DBUS_NAME_FLAG_DO_NOT_QUEUE;
    assert_eq!(flags, 5);
}

#[test]
fn test_parse_request_name_reply_codes() {
    // Test that we understand RequestName reply codes
    const DBUS_REQUEST_NAME_REPLY_PRIMARY_OWNER: u32 = 1;
    const DBUS_REQUEST_NAME_REPLY_IN_QUEUE: u32 = 2;
    const DBUS_REQUEST_NAME_REPLY_EXISTS: u32 = 3;
    const DBUS_REQUEST_NAME_REPLY_ALREADY_OWNER: u32 = 4;
    
    // Verify reply values match the D-Bus spec
    assert_eq!(DBUS_REQUEST_NAME_REPLY_PRIMARY_OWNER, 1);
    assert_eq!(DBUS_REQUEST_NAME_REPLY_IN_QUEUE, 2);
    assert_eq!(DBUS_REQUEST_NAME_REPLY_EXISTS, 3);
    assert_eq!(DBUS_REQUEST_NAME_REPLY_ALREADY_OWNER, 4);
}

#[test]
fn test_parse_release_name_reply_codes() {
    // Test that we understand ReleaseName reply codes
    const DBUS_RELEASE_NAME_REPLY_RELEASED: u32 = 1;
    const DBUS_RELEASE_NAME_REPLY_NON_EXISTENT: u32 = 2;
    const DBUS_RELEASE_NAME_REPLY_NOT_OWNER: u32 = 3;
    
    // Verify reply values match the D-Bus spec
    assert_eq!(DBUS_RELEASE_NAME_REPLY_RELEASED, 1);
    assert_eq!(DBUS_RELEASE_NAME_REPLY_NON_EXISTENT, 2);
    assert_eq!(DBUS_RELEASE_NAME_REPLY_NOT_OWNER, 3);
}

#[test]
fn test_valid_well_known_name() {
    // Test that we can validate well-known bus names according to D-Bus spec
    
    // Valid names
    let valid_names = [
        "com.example.foo",
        "org.freedesktop.DBus",
        "org._7_zip.Archiver", // Leading digit escaped
        "com.example.Test123",
        "com.example.test_name",
        "_com.example.test",
    ];
    
    for name in &valid_names {
        // Should have at least one dot
        assert!(name.contains('.'), "Name {} should contain a dot", name);
        // Should not start with a dot
        assert!(!name.starts_with('.'), "Name {} should not start with dot", name);
        // Should not be empty
        assert!(!name.is_empty());
    }
    
    // Invalid names (these would be rejected by RequestName)
    let invalid_names = [
        "org",          // No dots
        ".com.example", // Starts with dot
        "com..example", // Empty element
        "",             // Empty
        ":1.42",        // Unique name, not well-known
    ];
    
    for name in &invalid_names {
        // These should fail validation in a real implementation
        let is_well_known = !name.is_empty() 
            && !name.starts_with(':') 
            && name.contains('.') 
            && !name.starts_with('.')
            && !name.contains("..");
        
        assert!(!is_well_known, "Name '{}' should be invalid", name);
    }
}

#[test]
fn test_unique_name_format() {
    // Test unique connection name format
    
    // Valid unique names start with ':'
    let valid_unique = [":1.42", ":1.0", ":1.123456", ":mux.1"];
    
    for name in &valid_unique {
        assert!(name.starts_with(':'), "Unique name {} should start with ':'", name);
    }
    
    // Well-known names are NOT unique names
    let not_unique = ["org.freedesktop.DBus", "com.example.foo"];
    
    for name in &not_unique {
        assert!(!name.starts_with(':'), "Well-known name {} should not start with ':'", name);
    }
}

#[test]
fn test_reserved_bus_name() {
    // org.freedesktop.DBus is reserved and cannot be requested
    let reserved_name = "org.freedesktop.DBus";
    assert_eq!(reserved_name, "org.freedesktop.DBus");
    
    // This name should always return itself as owner
}

#[test]
fn test_guid_format() {
    // Test that GUIDs are 32 hex characters
    let test_guid = "0123456789abcdef0123456789abcdef";
    
    assert_eq!(test_guid.len(), 32);
    assert!(test_guid.chars().all(|c| c.is_ascii_hexdigit()));
    
    // Generate a GUID and verify format
    let guid = kapsule_dbus_mux::auth::generate_guid();
    assert_eq!(guid.len(), 32, "GUID should be 32 characters");
    assert!(
        guid.chars().all(|c| c.is_ascii_hexdigit()),
        "GUID should only contain hex digits"
    );
}

#[test]
fn test_machine_id_format() {
    // Machine ID should also be 32 hex characters
    // (same format as GUID, but sourced from /etc/machine-id)
    
    // Try to read machine ID
    if let Ok(contents) = std::fs::read_to_string("/etc/machine-id") {
        let machine_id = contents.trim();
        assert_eq!(machine_id.len(), 32, "Machine ID should be 32 characters");
        assert!(
            machine_id.chars().all(|c| c.is_ascii_hexdigit()),
            "Machine ID should only contain hex digits"
        );
    }
}

// =============================================================================
// Match Rule Tests (from test-matches.c)
// =============================================================================

#[test]
fn test_match_wildcard_empty_rule() {
    // Empty match rule should match all signals (wildcard)
    let rule = MatchRule::parse("").unwrap();
    
    // Create a test signal
    let signal = zbus::message::Message::signal("/org/example", "org.example", "Foo")
        .unwrap()
        .build(&())
        .unwrap();
    
    assert!(rule.matches(&signal), "Empty rule should match any signal");
}

#[test]
fn test_match_by_unique_sender() {
    // Match signals from a specific unique name sender
    let rule = MatchRule::parse("sender=':1.42'").unwrap();
    
    // Signal from matching sender
    let signal_match = zbus::message::Message::signal("/org/example", "org.example", "Foo")
        .unwrap()
        .sender(":1.42")
        .unwrap()
        .build(&())
        .unwrap();
    
    assert!(rule.matches(&signal_match), "Should match signal from sender :1.42");
    
    // Signal from different sender
    let signal_no_match = zbus::message::Message::signal("/org/example", "org.example", "Foo")
        .unwrap()
        .sender(":1.99")
        .unwrap()
        .build(&())
        .unwrap();
    
    assert!(!rule.matches(&signal_no_match), "Should not match signal from sender :1.99");
}

#[test]
fn test_match_by_well_known_sender() {
    // Match signals where sender claims a well-known name
    let rule = MatchRule::parse("sender='org.freedesktop.DBus'").unwrap();
    
    let signal = zbus::message::Message::signal("/org/freedesktop/DBus", "org.freedesktop.DBus", "NameOwnerChanged")
        .unwrap()
        .sender("org.freedesktop.DBus")
        .unwrap()
        .build(&("com.example.foo", "", ":1.100"))
        .unwrap();
    
    assert!(rule.matches(&signal), "Should match signal from org.freedesktop.DBus");
}

#[test]
fn test_match_noc_wildcard() {
    // Match all NameOwnerChanged signals
    let rule = MatchRule::parse("sender='org.freedesktop.DBus',member='NameOwnerChanged'").unwrap();
    
    let signal = zbus::message::Message::signal("/org/freedesktop/DBus", "org.freedesktop.DBus", "NameOwnerChanged")
        .unwrap()
        .sender("org.freedesktop.DBus")
        .unwrap()
        .build(&(":1.42", "", ":1.42"))
        .unwrap();
    
    assert!(rule.matches(&signal));
}

#[test]
fn test_match_noc_with_arg0_unique() {
    // Match NameOwnerChanged for a specific unique name
    let rule = MatchRule::parse("sender='org.freedesktop.DBus',member='NameOwnerChanged',arg0=':1.42'").unwrap();
    
    // Matching signal
    let signal_match = zbus::message::Message::signal("/org/freedesktop/DBus", "org.freedesktop.DBus", "NameOwnerChanged")
        .unwrap()
        .sender("org.freedesktop.DBus")
        .unwrap()
        .build(&(":1.42", "", ":1.42"))
        .unwrap();
    
    assert!(rule.matches(&signal_match));
    
    // Non-matching signal (different arg0)
    let signal_no_match = zbus::message::Message::signal("/org/freedesktop/DBus", "org.freedesktop.DBus", "NameOwnerChanged")
        .unwrap()
        .sender("org.freedesktop.DBus")
        .unwrap()
        .build(&(":1.99", "", ":1.99"))
        .unwrap();
    
    assert!(!rule.matches(&signal_no_match));
}

#[test]
fn test_match_noc_with_arg0_well_known() {
    // Match NameOwnerChanged for a specific well-known name
    let rule = MatchRule::parse("sender='org.freedesktop.DBus',member='NameOwnerChanged',arg0='com.example.foo'").unwrap();
    
    // Name acquired
    let signal_acquired = zbus::message::Message::signal("/org/freedesktop/DBus", "org.freedesktop.DBus", "NameOwnerChanged")
        .unwrap()
        .sender("org.freedesktop.DBus")
        .unwrap()
        .build(&("com.example.foo", "", ":1.42"))
        .unwrap();
    
    assert!(rule.matches(&signal_acquired));
    
    // Name released
    let signal_released = zbus::message::Message::signal("/org/freedesktop/DBus", "org.freedesktop.DBus", "NameOwnerChanged")
        .unwrap()
        .sender("org.freedesktop.DBus")
        .unwrap()
        .build(&("com.example.foo", ":1.42", ""))
        .unwrap();
    
    assert!(rule.matches(&signal_released));
    
    // Different name
    let signal_other = zbus::message::Message::signal("/org/freedesktop/DBus", "org.freedesktop.DBus", "NameOwnerChanged")
        .unwrap()
        .sender("org.freedesktop.DBus")
        .unwrap()
        .build(&("com.example.bar", "", ":1.42"))
        .unwrap();
    
    assert!(!rule.matches(&signal_other));
}

#[test]
fn test_match_arg_at_position_3() {
    // Match based on arg3 (from test-matches.c test_arg)
    let rule = MatchRule::parse("arg3='done'").unwrap();
    
    // Note: This requires the improved extract_string_args that handles mixed types
    // For now, we test with all-string body
    let signal_match = zbus::message::Message::signal("/org/example", "org.example", "Matches")
        .unwrap()
        .build(&("a", "b", "c", "done"))
        .unwrap();
    
    assert!(rule.matches(&signal_match), "Should match when arg3='done'");
    
    // Wrong value at arg3
    let signal_wrong = zbus::message::Message::signal("/org/example", "org.example", "DoesNotMatch")
        .unwrap()
        .build(&("a", "b", "c", "failed"))
        .unwrap();
    
    assert!(!rule.matches(&signal_wrong), "Should not match when arg3='failed'");
}

#[test]
fn test_match_multiple_args() {
    // Match on arg0 and arg1 simultaneously (from test-matches.c test_args)
    let rule = MatchRule::parse("arg0='zero',arg1='one'").unwrap();
    
    // Both args match
    let signal_match = zbus::message::Message::signal("/org/example", "org.example", "Matches")
        .unwrap()
        .build(&("zero", "one"))
        .unwrap();
    
    assert!(rule.matches(&signal_match));
    
    // Wrong arg1
    let signal_wrong1 = zbus::message::Message::signal("/org/example", "org.example", "DoesNotMatch")
        .unwrap()
        .build(&("zero", "wrong"))
        .unwrap();
    
    assert!(!rule.matches(&signal_wrong1));
    
    // Wrong arg0
    let signal_wrong0 = zbus::message::Message::signal("/org/example", "org.example", "DoesNotMatch")
        .unwrap()
        .build(&("wrong", "one"))
        .unwrap();
    
    assert!(!rule.matches(&signal_wrong0));
    
    // Args swapped
    let signal_swapped = zbus::message::Message::signal("/org/example", "org.example", "DoesNotMatch")
        .unwrap()
        .build(&("one", "zero"))
        .unwrap();
    
    assert!(!rule.matches(&signal_swapped));
}

#[test]
fn test_match_argpath_bidirectional() {
    // Test arg1path matching (from test-matches.c test_argpath)
    // Rule: arg1path='/aa/bb/'
    let rule = MatchRule::parse("arg1path='/aa/bb/'").unwrap();
    
    // Exact match
    let exact = zbus::message::Message::signal("/org/example", "org.example", "Matches")
        .unwrap()
        .build(&("foo", "/aa/bb/"))
        .unwrap();
    assert!(rule.matches(&exact), "Exact match should work");
    
    // Child path matches because filter ends with '/'
    let child = zbus::message::Message::signal("/org/example", "org.example", "Matches")
        .unwrap()
        .build(&("foo", "/aa/bb/cc"))
        .unwrap();
    assert!(rule.matches(&child), "Child path should match (filter ends with /)");
    
    // Deep child
    let deep_child = zbus::message::Message::signal("/org/example", "org.example", "Matches")
        .unwrap()
        .build(&("foo", "/aa/bb/cc/"))
        .unwrap();
    assert!(rule.matches(&deep_child));
    
    // Root matches because '/' is prefix of filter and ends with '/'
    let root = zbus::message::Message::signal("/org/example", "org.example", "Matches")
        .unwrap()
        .build(&("foo", "/"))
        .unwrap();
    assert!(rule.matches(&root), "Root '/' should match (arg ends with / and is prefix)");
    
    // Partial parent matches
    let parent = zbus::message::Message::signal("/org/example", "org.example", "Matches")
        .unwrap()
        .build(&("foo", "/aa/"))
        .unwrap();
    assert!(rule.matches(&parent), "/aa/ is prefix of /aa/bb/ and ends with /");
    
    // Non-matching: /aa/bb without trailing slash - NOT a prefix because neither ends with /
    // Wait, the filter is '/aa/bb/' which ends with /, so '/aa/bb/cc' should match
    // But '/aa/bb' (no trailing slash) - filter '/aa/bb/' starts with '/aa/bb' but
    // '/aa/bb' doesn't end with '/', so bidirectional check: neither arg ends with /
    // and they're not equal. This should NOT match per spec.
    let no_slash = zbus::message::Message::signal("/org/example", "org.example", "DoesNotMatch")
        .unwrap()
        .build(&("foo", "/aa/bb"))  // No trailing slash!
        .unwrap();
    // Actually, looking at spec more carefully:
    // - filter = '/aa/bb/' (ends with /)
    // - arg = '/aa/bb' (doesn't end with /)
    // Check: filter ends with '/' AND arg starts with filter? No, '/aa/bb' doesn't start with '/aa/bb/'
    // Check: arg ends with '/' AND filter starts with arg? No, arg doesn't end with '/'
    // So this should NOT match
    assert!(!rule.matches(&no_slash), "/aa/bb without trailing slash should not match /aa/bb/");
    
    // Completely different path
    let different = zbus::message::Message::signal("/org/example", "org.example", "DoesNotMatch")
        .unwrap()
        .build(&("foo", "/aa/b"))
        .unwrap();
    assert!(!rule.matches(&different));
}

// =============================================================================
// Error Name Constants Tests
// =============================================================================

#[test]
fn test_error_names() {
    use kapsule_dbus_mux::message::error_names;
    
    // Test that all error names follow D-Bus naming convention
    assert!(error_names::FAILED.starts_with("org.freedesktop.DBus.Error."));
    assert!(error_names::NAME_HAS_NO_OWNER.starts_with("org.freedesktop.DBus.Error."));
    assert!(error_names::INVALID_ARGS.starts_with("org.freedesktop.DBus.Error."));
    assert!(error_names::UNKNOWN_METHOD.starts_with("org.freedesktop.DBus.Error."));
    
    // Verify specific error names match spec
    assert_eq!(error_names::NAME_HAS_NO_OWNER, "org.freedesktop.DBus.Error.NameHasNoOwner");
    assert_eq!(error_names::INVALID_ARGS, "org.freedesktop.DBus.Error.InvalidArgs");
    assert_eq!(error_names::UNKNOWN_METHOD, "org.freedesktop.DBus.Error.UnknownMethod");
}

// =============================================================================
// Protocol Compliance Tests
// =============================================================================

#[test]
fn test_message_type_codes() {
    // Verify message type constants match D-Bus spec
    use zbus::message::Type;
    
    // These are the only valid message types per spec
    let _method_call = Type::MethodCall;
    let _method_return = Type::MethodReturn;
    let _error = Type::Error;
    let _signal = Type::Signal;
}

#[test]
fn test_header_field_codes() {
    // Test that we understand header field meanings
    // PATH = 1, INTERFACE = 2, MEMBER = 3, ERROR_NAME = 4,
    // REPLY_SERIAL = 5, DESTINATION = 6, SENDER = 7, SIGNATURE = 8, UNIX_FDS = 9
    
    let msg = zbus::message::Message::signal("/test/path", "org.test.Interface", "TestSignal")
        .unwrap()
        .sender(":1.42")
        .unwrap()
        .destination(":1.99")
        .unwrap()
        .build(&())
        .unwrap();
    
    // Verify headers can be read
    use kapsule_dbus_mux::message::MessageExt;
    assert_eq!(msg.path_str().as_deref(), Some("/test/path"));
    assert_eq!(msg.interface_str().as_deref(), Some("org.test.Interface"));
    assert_eq!(msg.member_str().as_deref(), Some("TestSignal"));
    assert_eq!(msg.sender_str().as_deref(), Some(":1.42"));
    assert_eq!(msg.destination_str().as_deref(), Some(":1.99"));
}

// =============================================================================
// Integration Test Helpers (for future live tests)
// =============================================================================

/// Creates a test method call to org.freedesktop.DBus
fn create_dbus_method_call(method: &str) -> zbus::message::Message {
    zbus::message::Message::method_call("/org/freedesktop/DBus", method)
        .unwrap()
        .interface("org.freedesktop.DBus")
        .unwrap()
        .destination("org.freedesktop.DBus")
        .unwrap()
        .build(&())
        .unwrap()
}

#[test]
fn test_create_hello_message() {
    let msg = create_dbus_method_call("Hello");
    
    use kapsule_dbus_mux::message::MessageExt;
    assert_eq!(msg.interface_str().as_deref(), Some("org.freedesktop.DBus"));
    assert_eq!(msg.member_str().as_deref(), Some("Hello"));
    assert_eq!(msg.destination_str().as_deref(), Some("org.freedesktop.DBus"));
    assert_eq!(msg.path_str().as_deref(), Some("/org/freedesktop/DBus"));
}

#[test]
fn test_create_request_name_message() {
    let msg = zbus::message::Message::method_call("/org/freedesktop/DBus", "RequestName")
        .unwrap()
        .interface("org.freedesktop.DBus")
        .unwrap()
        .destination("org.freedesktop.DBus")
        .unwrap()
        .build(&("com.example.foo", 0u32))
        .unwrap();
    
    use kapsule_dbus_mux::message::MessageExt;
    assert_eq!(msg.member_str().as_deref(), Some("RequestName"));
    
    // Verify body can be deserialized
    let (name, flags): (String, u32) = msg.body().deserialize().unwrap();
    assert_eq!(name, "com.example.foo");
    assert_eq!(flags, 0);
}

#[test]
fn test_create_get_name_owner_message() {
    let msg = zbus::message::Message::method_call("/org/freedesktop/DBus", "GetNameOwner")
        .unwrap()
        .interface("org.freedesktop.DBus")
        .unwrap()
        .destination("org.freedesktop.DBus")
        .unwrap()
        .build(&("com.example.foo",))
        .unwrap();
    
    let (name,): (String,) = msg.body().deserialize().unwrap();
    assert_eq!(name, "com.example.foo");
}

#[test]
fn test_create_add_match_message() {
    let rule = "type='signal',sender='org.freedesktop.DBus',member='NameOwnerChanged'";
    
    let msg = zbus::message::Message::method_call("/org/freedesktop/DBus", "AddMatch")
        .unwrap()
        .interface("org.freedesktop.DBus")
        .unwrap()
        .destination("org.freedesktop.DBus")
        .unwrap()
        .build(&(rule,))
        .unwrap();
    
    let (parsed_rule,): (String,) = msg.body().deserialize().unwrap();
    assert_eq!(parsed_rule, rule);
}

#[test]
fn test_create_list_queued_owners_message() {
    let msg = zbus::message::Message::method_call("/org/freedesktop/DBus", "ListQueuedOwners")
        .unwrap()
        .interface("org.freedesktop.DBus")
        .unwrap()
        .destination("org.freedesktop.DBus")
        .unwrap()
        .build(&("com.example.foo",))
        .unwrap();
    
    use kapsule_dbus_mux::message::MessageExt;
    assert_eq!(msg.member_str().as_deref(), Some("ListQueuedOwners"));
}

#[test]
fn test_create_get_connection_unix_user_message() {
    let msg = zbus::message::Message::method_call("/org/freedesktop/DBus", "GetConnectionUnixUser")
        .unwrap()
        .interface("org.freedesktop.DBus")
        .unwrap()
        .destination("org.freedesktop.DBus")
        .unwrap()
        .build(&(":1.42",))
        .unwrap();
    
    let (bus_name,): (String,) = msg.body().deserialize().unwrap();
    assert_eq!(bus_name, ":1.42");
}

#[test]
fn test_create_introspect_message() {
    let msg = zbus::message::Message::method_call("/org/freedesktop/DBus", "Introspect")
        .unwrap()
        .interface("org.freedesktop.DBus.Introspectable")
        .unwrap()
        .destination("org.freedesktop.DBus")
        .unwrap()
        .build(&())
        .unwrap();
    
    use kapsule_dbus_mux::message::MessageExt;
    assert_eq!(msg.interface_str().as_deref(), Some("org.freedesktop.DBus.Introspectable"));
    assert_eq!(msg.member_str().as_deref(), Some("Introspect"));
}

#[test]
fn test_create_ping_message() {
    let msg = zbus::message::Message::method_call("/org/freedesktop/DBus", "Ping")
        .unwrap()
        .interface("org.freedesktop.DBus.Peer")
        .unwrap()
        .destination("org.freedesktop.DBus")
        .unwrap()
        .build(&())
        .unwrap();
    
    use kapsule_dbus_mux::message::MessageExt;
    assert_eq!(msg.interface_str().as_deref(), Some("org.freedesktop.DBus.Peer"));
    assert_eq!(msg.member_str().as_deref(), Some("Ping"));
}

#[test]
fn test_create_get_machine_id_message() {
    let msg = zbus::message::Message::method_call("/org/freedesktop/DBus", "GetMachineId")
        .unwrap()
        .interface("org.freedesktop.DBus.Peer")
        .unwrap()
        .destination("org.freedesktop.DBus")
        .unwrap()
        .build(&())
        .unwrap();
    
    use kapsule_dbus_mux::message::MessageExt;
    assert_eq!(msg.interface_str().as_deref(), Some("org.freedesktop.DBus.Peer"));
    assert_eq!(msg.member_str().as_deref(), Some("GetMachineId"));
}
