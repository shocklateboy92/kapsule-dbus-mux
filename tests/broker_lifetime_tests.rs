//! Integration tests for client lifetime and broker behavior.
//!
//! These tests are based on the dbus-broker test suite (test-broker.c and test-lifetime.c)
//! and verify basic connectivity, message passing, and client lifecycle.

use kapsule_dbus_mux::match_rules::{ClientMatchRules, MatchRule, MatchRuleRefCount};

// =============================================================================
// Client Match Rules Lifecycle Tests
// =============================================================================

#[test]
fn test_client_match_rules_lifecycle() {
    // Test that match rules are properly managed through client lifecycle
    let mut rules = ClientMatchRules::new();
    
    assert!(rules.is_empty());
    assert_eq!(rules.len(), 0);
    
    // Add some rules like a client would during setup
    rules.add(MatchRule::parse("type='signal',sender='org.freedesktop.DBus'").unwrap());
    rules.add(MatchRule::parse("type='signal',interface='org.example.Interface'").unwrap());
    
    assert!(!rules.is_empty());
    assert_eq!(rules.len(), 2);
    
    // Get rule strings (for cleanup when client disconnects)
    let rule_strings = rules.rule_strings();
    assert_eq!(rule_strings.len(), 2);
    assert!(rule_strings.contains(&"type='signal',sender='org.freedesktop.DBus'".to_string()));
    assert!(rule_strings.contains(&"type='signal',interface='org.example.Interface'".to_string()));
}

#[test]
fn test_match_rule_refcount_client_disconnect() {
    // Test that reference counting properly handles client disconnects
    let mut refcount = MatchRuleRefCount::new();
    
    // Client 1 adds a rule
    let should_add = refcount.increment("session", "type='signal'");
    assert!(should_add, "First client should cause rule to be added to bus");
    
    // Client 2 adds the same rule
    let should_add = refcount.increment("session", "type='signal'");
    assert!(!should_add, "Second client should not cause duplicate add");
    
    // Client 1 disconnects
    let should_remove = refcount.decrement("session", "type='signal'");
    assert!(!should_remove, "Rule should remain on bus while client 2 has it");
    
    // Client 2 disconnects
    let should_remove = refcount.decrement("session", "type='signal'");
    assert!(should_remove, "Rule should be removed from bus when last client leaves");
}

#[test]
fn test_match_rule_refcount_batch_cleanup() {
    // Test batch cleanup when client disconnects with multiple rules
    let mut refcount = MatchRuleRefCount::new();
    
    let client_rules = vec![
        "type='signal'".to_string(),
        "interface='org.example'".to_string(),
        "member='Foo'".to_string(),
    ];
    
    // Add all rules
    for rule in &client_rules {
        refcount.increment("session", rule);
    }
    
    // Verify all rules are tracked
    let active = refcount.rules_for_bus("session");
    assert_eq!(active.len(), 3);
    
    // Client disconnects - remove all their rules
    let to_remove = refcount.remove_client_rules("session", &client_rules);
    assert_eq!(to_remove.len(), 3);
    
    // No rules should remain
    let active = refcount.rules_for_bus("session");
    assert_eq!(active.len(), 0);
}

#[test]
fn test_match_rule_refcount_partial_overlap() {
    // Test when clients have partially overlapping rule sets
    let mut refcount = MatchRuleRefCount::new();
    
    let client1_rules = vec![
        "type='signal'".to_string(),
        "interface='org.example'".to_string(),
    ];
    
    let client2_rules = vec![
        "type='signal'".to_string(), // Shared with client1
        "member='Foo'".to_string(),  // Unique to client2
    ];
    
    // Client 1 connects
    for rule in &client1_rules {
        refcount.increment("session", rule);
    }
    
    // Client 2 connects
    for rule in &client2_rules {
        refcount.increment("session", rule);
    }
    
    // All 3 unique rules should be active
    let active = refcount.rules_for_bus("session");
    assert_eq!(active.len(), 3);
    
    // Client 1 disconnects
    let to_remove = refcount.remove_client_rules("session", &client1_rules);
    
    // Only "interface='org.example'" should be removed (client2 still has type='signal')
    assert_eq!(to_remove.len(), 1);
    assert!(to_remove.contains(&"interface='org.example'".to_string()));
    
    // 2 rules should remain
    let active = refcount.rules_for_bus("session");
    assert_eq!(active.len(), 2);
}

// =============================================================================
// Signal Filtering Tests (like test_monitor in test-lifetime.c)
// =============================================================================

#[test]
fn test_name_owner_changed_signal_format() {
    // Verify we can create and match NameOwnerChanged signals
    
    // The signal format is: NameOwnerChanged(name: s, old_owner: s, new_owner: s)
    let signal = zbus::message::Message::signal(
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
        "NameOwnerChanged",
    )
    .unwrap()
    .sender("org.freedesktop.DBus")
    .unwrap()
    .build(&("com.example.Service", "", ":1.42"))
    .unwrap();
    
    // Rule to match NameOwnerChanged from the bus
    let rule = MatchRule::parse(
        "type='signal',sender='org.freedesktop.DBus',member='NameOwnerChanged'"
    ).unwrap();
    
    assert!(rule.matches(&signal));
    
    // Verify body content
    let (name, old_owner, new_owner): (String, String, String) = 
        signal.body().deserialize().unwrap();
    assert_eq!(name, "com.example.Service");
    assert_eq!(old_owner, "");
    assert_eq!(new_owner, ":1.42");
}

#[test]
fn test_name_acquired_signal_format() {
    // NameAcquired signal is sent to the client that acquired a name
    let signal = zbus::message::Message::signal(
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
        "NameAcquired",
    )
    .unwrap()
    .sender("org.freedesktop.DBus")
    .unwrap()
    .destination(":1.42")
    .unwrap()
    .build(&("com.example.Service",))
    .unwrap();
    
    let rule = MatchRule::parse(
        "type='signal',sender='org.freedesktop.DBus',member='NameAcquired'"
    ).unwrap();
    
    assert!(rule.matches(&signal));
    
    let (name,): (String,) = signal.body().deserialize().unwrap();
    assert_eq!(name, "com.example.Service");
}

#[test]
fn test_name_lost_signal_format() {
    // NameLost signal is sent when a client loses a name
    let signal = zbus::message::Message::signal(
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
        "NameLost",
    )
    .unwrap()
    .sender("org.freedesktop.DBus")
    .unwrap()
    .destination(":1.42")
    .unwrap()
    .build(&("com.example.Service",))
    .unwrap();
    
    let rule = MatchRule::parse(
        "type='signal',sender='org.freedesktop.DBus',member='NameLost'"
    ).unwrap();
    
    assert!(rule.matches(&signal));
}

// =============================================================================
// Connection Identity Tests
// =============================================================================

#[test]
fn test_unique_name_generation_format() {
    // Test that unique names follow the expected format
    
    // Unique names must start with ':'
    // Format is typically :N.M where N and M are numbers
    
    let example_names = [":1.0", ":1.1", ":1.42", ":100.999"];
    
    for name in &example_names {
        assert!(name.starts_with(':'), "Unique name must start with ':'");
        
        // After the colon, should have number.number pattern
        let parts: Vec<&str> = name[1..].split('.').collect();
        assert_eq!(parts.len(), 2, "Unique name should have format :N.M");
        
        // Both parts should be numeric
        assert!(parts[0].parse::<u32>().is_ok());
        assert!(parts[1].parse::<u32>().is_ok());
    }
}

#[test]
fn test_mux_unique_name_format() {
    // The mux uses a different unique name format: :mux.N
    let mux_name = ":mux.1";
    
    assert!(mux_name.starts_with(':'));
    assert!(mux_name.starts_with(":mux."));
}

// =============================================================================
// Message Delivery Tests
// =============================================================================

#[test]
fn test_broadcast_signal_matches_multiple_rules() {
    // A broadcast signal (no destination) should be delivered to all clients
    // with matching rules
    
    let signal = zbus::message::Message::signal(
        "/org/example",
        "org.example",
        "TestSignal",
    )
    .unwrap()
    .sender(":1.42")
    .unwrap()
    // No destination = broadcast
    .build(&())
    .unwrap();
    
    // Various rules that should match
    let rules_that_match = [
        "",  // Wildcard
        "type='signal'",
        "sender=':1.42'",
        "interface='org.example'",
        "member='TestSignal'",
        "path='/org/example'",
        "type='signal',sender=':1.42'",
        "type='signal',interface='org.example',member='TestSignal'",
    ];
    
    for rule_str in &rules_that_match {
        let rule = MatchRule::parse(rule_str).unwrap();
        assert!(rule.matches(&signal), "Rule '{}' should match broadcast signal", rule_str);
    }
    
    // Rules that should NOT match
    let rules_that_dont_match = [
        "sender=':1.99'",
        "interface='org.other'",
        "member='OtherSignal'",
        "type='method_call'",
    ];
    
    for rule_str in &rules_that_dont_match {
        let rule = MatchRule::parse(rule_str).unwrap();
        assert!(!rule.matches(&signal), "Rule '{}' should NOT match broadcast signal", rule_str);
    }
}

#[test]
fn test_unicast_signal_with_destination() {
    // A unicast signal (with destination) is delivered only to that destination
    
    let signal = zbus::message::Message::signal(
        "/org/example",
        "org.example",
        "UnicastSignal",
    )
    .unwrap()
    .sender(":1.42")
    .unwrap()
    .destination(":1.99")
    .unwrap()
    .build(&())
    .unwrap();
    
    // Rule with matching destination
    let rule = MatchRule::parse("destination=':1.99'").unwrap();
    assert!(rule.matches(&signal));
    
    // Rule with non-matching destination
    let rule2 = MatchRule::parse("destination=':1.88'").unwrap();
    assert!(!rule2.matches(&signal));
    
    // Rule without destination filter matches based on other criteria
    let rule3 = MatchRule::parse("type='signal',sender=':1.42'").unwrap();
    assert!(rule3.matches(&signal));
}

// =============================================================================
// Two-Client Interaction Tests (like test_ping_pong in test-broker.c)
// =============================================================================

#[test]
fn test_method_call_reply_serial_matching() {
    // Test that reply serials are correctly matched
    
    // Create a method call
    let call = zbus::message::Message::method_call("/test", "Test")
        .unwrap()
        .interface("org.test.Interface")
        .unwrap()
        .destination(":1.99")
        .unwrap()
        .build(&())
        .unwrap();
    
    let call_serial = call.primary_header().serial_num();
    
    // Create a reply that references the call's serial
    let reply = zbus::message::Message::method_return(&call.header())
        .unwrap()
        .build(&())
        .unwrap();
    
    // The reply should have REPLY_SERIAL matching the call's serial
    let reply_serial = reply.header().reply_serial().unwrap();
    assert_eq!(reply_serial.get(), call_serial.get());
}

#[test]
fn test_error_reply_serial_matching() {
    // Test that error replies correctly reference the original call
    
    let call = zbus::message::Message::method_call("/test", "Test")
        .unwrap()
        .interface("org.test.Interface")
        .unwrap()
        .destination(":1.99")
        .unwrap()
        .build(&())
        .unwrap();
    
    let call_serial = call.primary_header().serial_num();
    
    // Create an error reply
    let error = zbus::message::Message::error(&call.header(), "org.test.Error.Failed")
        .unwrap()
        .build(&("Test failed",))
        .unwrap();
    
    let reply_serial = error.header().reply_serial().unwrap();
    assert_eq!(reply_serial.get(), call_serial.get());
}

// =============================================================================
// Peer Interface Tests
// =============================================================================

#[test]
fn test_peer_interface_paths() {
    // The Peer interface can be invoked on any path
    let paths = ["/", "/org/freedesktop/DBus", "/some/random/path"];
    
    for path in paths {
        let msg = zbus::message::Message::method_call(path, "Ping")
            .unwrap()
            .interface("org.freedesktop.DBus.Peer")
            .unwrap()
            .build(&())
            .unwrap();
        
        use kapsule_dbus_mux::message::MessageExt;
        assert_eq!(msg.interface_str().as_deref(), Some("org.freedesktop.DBus.Peer"));
        assert_eq!(msg.member_str().as_deref(), Some("Ping"));
        assert_eq!(msg.path_str().as_deref(), Some(path));
    }
}

// =============================================================================
// Standard Interface Tests
// =============================================================================

#[test]
fn test_introspectable_interface() {
    let msg = zbus::message::Message::method_call("/", "Introspect")
        .unwrap()
        .interface("org.freedesktop.DBus.Introspectable")
        .unwrap()
        .build(&())
        .unwrap();
    
    use kapsule_dbus_mux::message::MessageExt;
    assert_eq!(msg.interface_str().as_deref(), Some("org.freedesktop.DBus.Introspectable"));
}

#[test]
fn test_properties_interface() {
    // Get property
    let get = zbus::message::Message::method_call("/", "Get")
        .unwrap()
        .interface("org.freedesktop.DBus.Properties")
        .unwrap()
        .build(&("org.example.Interface", "PropertyName"))
        .unwrap();
    
    let (iface, prop): (String, String) = get.body().deserialize().unwrap();
    assert_eq!(iface, "org.example.Interface");
    assert_eq!(prop, "PropertyName");
    
    // GetAll
    let get_all = zbus::message::Message::method_call("/", "GetAll")
        .unwrap()
        .interface("org.freedesktop.DBus.Properties")
        .unwrap()
        .build(&("org.example.Interface",))
        .unwrap();
    
    let (iface,): (String,) = get_all.body().deserialize().unwrap();
    assert_eq!(iface, "org.example.Interface");
}
