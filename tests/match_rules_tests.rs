//! Integration tests for AddMatch/RemoveMatch and signal filtering.

use kapsule_dbus_mux::match_rules::{ClientMatchRules, MatchRule};

/// Helper to create a test signal message.
fn create_test_signal(
    interface: &str,
    member: &str,
    path: &str,
    sender: Option<&str>,
) -> zbus::message::Message {
    let mut builder = zbus::message::Message::signal(path, interface, member).unwrap();
    if let Some(s) = sender {
        builder = builder.sender(s).unwrap();
    }
    builder.build(&()).unwrap()
}

/// Helper to create a test signal with body.
fn create_test_signal_with_body<T: serde::Serialize + zbus::zvariant::Type>(
    interface: &str,
    member: &str,
    path: &str,
    sender: Option<&str>,
    body: &T,
) -> zbus::message::Message {
    let mut builder = zbus::message::Message::signal(path, interface, member).unwrap();
    if let Some(s) = sender {
        builder = builder.sender(s).unwrap();
    }
    builder.build(body).unwrap()
}

#[test]
fn test_match_signal_type() {
    let rule = MatchRule::parse("type='signal'").unwrap();
    
    // Signal should match
    let signal = create_test_signal("org.example.Interface", "TestSignal", "/org/example", None);
    assert!(rule.matches(&signal), "Signal should match type='signal'");
    
    // Method call should not match
    let method_call = zbus::message::Message::method_call("/org/example", "TestMethod")
        .unwrap()
        .interface("org.example.Interface")
        .unwrap()
        .build(&())
        .unwrap();
    assert!(!rule.matches(&method_call), "Method call should not match type='signal'");
}

#[test]
fn test_match_interface() {
    let rule = MatchRule::parse("type='signal',interface='org.freedesktop.DBus'").unwrap();
    
    // Matching interface
    let signal = create_test_signal("org.freedesktop.DBus", "NameOwnerChanged", "/org/freedesktop/DBus", None);
    assert!(rule.matches(&signal));
    
    // Different interface
    let signal2 = create_test_signal("org.gnome.Shell", "ExtensionStatusChanged", "/org/gnome/Shell", None);
    assert!(!rule.matches(&signal2));
}

#[test]
fn test_match_member() {
    let rule = MatchRule::parse("type='signal',member='NameOwnerChanged'").unwrap();
    
    // Matching member
    let signal = create_test_signal("org.freedesktop.DBus", "NameOwnerChanged", "/org/freedesktop/DBus", None);
    assert!(rule.matches(&signal));
    
    // Different member
    let signal2 = create_test_signal("org.freedesktop.DBus", "NameAcquired", "/org/freedesktop/DBus", None);
    assert!(!rule.matches(&signal2));
}

#[test]
fn test_match_sender() {
    let rule = MatchRule::parse("type='signal',sender='org.freedesktop.DBus'").unwrap();
    
    // Matching sender
    let signal = create_test_signal(
        "org.freedesktop.DBus", 
        "NameOwnerChanged", 
        "/org/freedesktop/DBus",
        Some("org.freedesktop.DBus"),
    );
    assert!(rule.matches(&signal));
    
    // Different sender
    let signal2 = create_test_signal(
        "org.freedesktop.DBus",
        "NameOwnerChanged",
        "/org/freedesktop/DBus",
        Some(":1.42"),
    );
    assert!(!rule.matches(&signal2));
}

#[test]
fn test_match_path() {
    let rule = MatchRule::parse("type='signal',path='/org/freedesktop/DBus'").unwrap();
    
    // Matching path
    let signal = create_test_signal("org.freedesktop.DBus", "NameOwnerChanged", "/org/freedesktop/DBus", None);
    assert!(rule.matches(&signal));
    
    // Different path
    let signal2 = create_test_signal("org.freedesktop.DBus", "NameOwnerChanged", "/org/example", None);
    assert!(!rule.matches(&signal2));
}

#[test]
fn test_match_path_namespace() {
    let rule = MatchRule::parse("type='signal',path_namespace='/org/freedesktop'").unwrap();
    
    // Exact match
    let signal1 = create_test_signal("org.freedesktop.DBus", "Test", "/org/freedesktop", None);
    assert!(rule.matches(&signal1));
    
    // Child path
    let signal2 = create_test_signal("org.freedesktop.DBus", "Test", "/org/freedesktop/DBus", None);
    assert!(rule.matches(&signal2));
    
    // Deep child
    let signal3 = create_test_signal("org.freedesktop.DBus", "Test", "/org/freedesktop/DBus/Local", None);
    assert!(rule.matches(&signal3));
    
    // Non-matching path
    let signal4 = create_test_signal("org.gnome.Shell", "Test", "/org/gnome/Shell", None);
    assert!(!rule.matches(&signal4));
}

#[test]
fn test_match_arg0() {
    let rule = MatchRule::parse("type='signal',arg0='com.example.Service'").unwrap();
    
    // Matching arg0
    let signal = create_test_signal_with_body(
        "org.freedesktop.DBus",
        "NameOwnerChanged",
        "/org/freedesktop/DBus",
        None,
        &("com.example.Service", "", ":1.100"),
    );
    assert!(rule.matches(&signal));
    
    // Different arg0
    let signal2 = create_test_signal_with_body(
        "org.freedesktop.DBus",
        "NameOwnerChanged",
        "/org/freedesktop/DBus",
        None,
        &("com.other.Service", "", ":1.100"),
    );
    assert!(!rule.matches(&signal2));
}

#[test]
fn test_match_arg0path() {
    // Per D-Bus spec: "there is also a match when either the string given in the 
    // match rule or the appropriate message argument ends with '/' and is a prefix 
    // of the other."
    
    // Test with trailing slash in filter (common case for monitoring directories)
    let rule = MatchRule::parse("type='signal',arg0path='/org/example/'").unwrap();
    
    // Exact match
    let signal1 = create_test_signal_with_body(
        "org.example",
        "PathChanged",
        "/org/example",
        None,
        &("/org/example/",),
    );
    assert!(rule.matches(&signal1), "Exact match should work");
    
    // Child path (filter ends with /, so it matches children)
    let signal2 = create_test_signal_with_body(
        "org.example",
        "PathChanged",
        "/org/example",
        None,
        &("/org/example/child",),
    );
    assert!(rule.matches(&signal2), "Filter with trailing / should match child paths");
    
    // Non-matching path
    let signal3 = create_test_signal_with_body(
        "org.example",
        "PathChanged",
        "/org/example",
        None,
        &("/org/other",),
    );
    assert!(!rule.matches(&signal3), "Non-matching path should not match");
    
    // Test bidirectional matching: arg ends with '/', filter doesn't
    let rule_no_slash = MatchRule::parse("type='signal',arg0path='/org/example'").unwrap();
    
    // Arg with trailing slash that is prefix of filter
    let signal4 = create_test_signal_with_body(
        "org.example",
        "PathChanged",
        "/org/example",
        None,
        &("/org/",),
    );
    assert!(rule_no_slash.matches(&signal4), "Arg ending with / should match if it's prefix of filter");
    
    // Root path with trailing slash
    let signal5 = create_test_signal_with_body(
        "org.example",
        "PathChanged",
        "/",
        None,
        &("/",),
    );
    assert!(rule_no_slash.matches(&signal5), "Root path / should match as prefix");
    
    // Neither ends with slash, not exact match - should NOT match
    let signal6 = create_test_signal_with_body(
        "org.example",
        "PathChanged",
        "/org/example",
        None,
        &("/org/example/child",),
    );
    assert!(!rule_no_slash.matches(&signal6), "Without trailing slash, child should not match");
}

#[test]
fn test_match_complex_rule() {
    let rule = MatchRule::parse(
        "type='signal',sender='org.freedesktop.DBus',interface='org.freedesktop.DBus',member='NameOwnerChanged',path='/org/freedesktop/DBus'"
    ).unwrap();
    
    // All fields match
    let signal = create_test_signal(
        "org.freedesktop.DBus",
        "NameOwnerChanged",
        "/org/freedesktop/DBus",
        Some("org.freedesktop.DBus"),
    );
    assert!(rule.matches(&signal));
    
    // Wrong sender
    let signal2 = create_test_signal(
        "org.freedesktop.DBus",
        "NameOwnerChanged",
        "/org/freedesktop/DBus",
        Some(":1.1"),
    );
    assert!(!rule.matches(&signal2));
    
    // Wrong member
    let signal3 = create_test_signal(
        "org.freedesktop.DBus",
        "NameAcquired",
        "/org/freedesktop/DBus",
        Some("org.freedesktop.DBus"),
    );
    assert!(!rule.matches(&signal3));
}

#[test]
fn test_empty_rule_matches_all_signals() {
    let rule = MatchRule::parse("").unwrap();
    
    let signal1 = create_test_signal("org.example.A", "Signal1", "/a", None);
    let signal2 = create_test_signal("org.example.B", "Signal2", "/b", None);
    
    // Empty rule matches all (but type='signal' is usually needed for signals)
    // Actually, empty rule matches all message types including signals
    assert!(rule.matches(&signal1));
    assert!(rule.matches(&signal2));
}

#[test]
fn test_client_match_rules_filtering() {
    let mut rules = ClientMatchRules::new();
    
    // Add rule for NameOwnerChanged
    rules.add(MatchRule::parse("type='signal',interface='org.freedesktop.DBus',member='NameOwnerChanged'").unwrap());
    
    // Add rule for a specific sender (unique name)
    rules.add(MatchRule::parse("type='signal',sender=':1.100'").unwrap());
    
    // NameOwnerChanged should match
    let noc = create_test_signal("org.freedesktop.DBus", "NameOwnerChanged", "/org/freedesktop/DBus", None);
    assert!(rules.matches(&noc));
    
    // Signal from :1.100 should match
    let my_signal = create_test_signal("com.example.MyService", "TestSignal", "/", Some(":1.100"));
    assert!(rules.matches(&my_signal));
    
    // Random signal should not match
    let random = create_test_signal("org.random.Interface", "RandomSignal", "/random", Some(":1.999"));
    assert!(!rules.matches(&random));
}

#[test]
fn test_client_match_rules_empty() {
    let rules = ClientMatchRules::new();
    
    // With no rules, nothing should match
    let signal = create_test_signal("org.example", "Test", "/", None);
    assert!(!rules.matches(&signal));
}

#[test]
fn test_match_rule_parse_errors() {
    // Missing equals
    assert!(MatchRule::parse("type").is_err());
    
    // Unclosed quote
    assert!(MatchRule::parse("type='signal").is_err());
    
    // Invalid arg index
    assert!(MatchRule::parse("arg99='test'").is_err());
}

#[test]
fn test_match_rule_destination() {
    let rule = MatchRule::parse("destination=':1.42'").unwrap();
    
    // For signals, destination is usually not set, so this typically wouldn't match
    // But if it were set...
    let signal = zbus::message::Message::signal("/test", "org.example", "Test")
        .unwrap()
        .destination(":1.42")
        .unwrap()
        .build(&())
        .unwrap();
    assert!(rule.matches(&signal));
}

#[test]
fn test_match_rule_with_whitespace() {
    // Various whitespace formats should parse
    let rule1 = MatchRule::parse("  type='signal'  ").unwrap();
    let rule2 = MatchRule::parse("type='signal', interface='org.example'").unwrap();
    let rule3 = MatchRule::parse("type='signal' ,interface='org.example'").unwrap();
    
    assert_eq!(rule1.msg_type, Some("signal".to_string()));
    assert_eq!(rule2.msg_type, Some("signal".to_string()));
    assert_eq!(rule2.interface, Some("org.example".to_string()));
    assert_eq!(rule3.msg_type, Some("signal".to_string()));
    assert_eq!(rule3.interface, Some("org.example".to_string()));
}

#[test]
fn test_match_rules_name_owner_changed_filter() {
    // Common use case: subscribe to NameOwnerChanged for a specific service
    let rule = MatchRule::parse(
        "type='signal',sender='org.freedesktop.DBus',interface='org.freedesktop.DBus',member='NameOwnerChanged',arg0='com.example.MyApp'"
    ).unwrap();
    
    // Matching: NameOwnerChanged for com.example.MyApp
    let signal = create_test_signal_with_body(
        "org.freedesktop.DBus",
        "NameOwnerChanged",
        "/org/freedesktop/DBus",
        Some("org.freedesktop.DBus"),
        &("com.example.MyApp", "", ":1.50"),
    );
    assert!(rule.matches(&signal), "Should match NameOwnerChanged for MyApp");
    
    // Non-matching: NameOwnerChanged for different service
    let signal2 = create_test_signal_with_body(
        "org.freedesktop.DBus",
        "NameOwnerChanged",
        "/org/freedesktop/DBus",
        Some("org.freedesktop.DBus"),
        &("com.other.Service", "", ":1.51"),
    );
    assert!(!rule.matches(&signal2), "Should not match NameOwnerChanged for other service");
}

#[test]
fn test_multiple_arg_filters() {
    // Rule that filters on both arg0 and arg1 (though unusual)
    let rule = MatchRule::parse("type='signal',arg0='hello',arg1='world'").unwrap();
    
    let signal_match = create_test_signal_with_body(
        "org.example",
        "Test",
        "/",
        None,
        &("hello", "world"),
    );
    assert!(rule.matches(&signal_match));
    
    let signal_no_match = create_test_signal_with_body(
        "org.example",
        "Test",
        "/",
        None,
        &("hello", "other"),
    );
    assert!(!rule.matches(&signal_no_match));
}

#[test]
fn test_add_remove_match_rules() {
    let mut rules = ClientMatchRules::new();
    
    let rule_str = "type='signal',interface='org.example'";
    let rule = MatchRule::parse(rule_str).unwrap();
    
    // Add rule
    rules.add(rule);
    assert_eq!(rules.len(), 1);
    
    // Try to add duplicate - should not increase count
    let rule_dup = MatchRule::parse(rule_str).unwrap();
    rules.add(rule_dup);
    assert_eq!(rules.len(), 1);
    
    // Add different rule
    let rule2 = MatchRule::parse("type='signal',member='OtherSignal'").unwrap();
    rules.add(rule2);
    assert_eq!(rules.len(), 2);
    
    // Remove first rule
    assert!(rules.remove(rule_str));
    assert_eq!(rules.len(), 1);
    
    // Try to remove again - should return false
    assert!(!rules.remove(rule_str));
    assert_eq!(rules.len(), 1);
}
