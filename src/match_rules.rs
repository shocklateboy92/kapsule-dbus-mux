//! D-Bus match rule parsing and matching.
//!
//! This module implements D-Bus match rule parsing according to the D-Bus specification.
//! Match rules are used by clients to subscribe to specific signals.

use std::collections::HashMap;

use tracing::trace;
use zbus::message::Message;

use crate::message::MessageExt;

/// A parsed D-Bus match rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchRule {
    /// The original rule string.
    pub rule_string: String,
    /// Message type filter (signal, method_call, method_return, error).
    pub msg_type: Option<String>,
    /// Sender filter.
    pub sender: Option<String>,
    /// Interface filter.
    pub interface: Option<String>,
    /// Member (method/signal name) filter.
    pub member: Option<String>,
    /// Object path filter.
    pub path: Option<String>,
    /// Object path namespace filter (matches path and all children).
    pub path_namespace: Option<String>,
    /// Destination filter.
    pub destination: Option<String>,
    /// Argument filters (arg0, arg1, etc.).
    pub args: HashMap<u8, String>,
    /// Argument path filters (arg0path, arg1path, etc.).
    pub arg_paths: HashMap<u8, String>,
    /// Argument namespace filters (arg0namespace only, per D-Bus spec).
    /// Matches if arg0 is a bus name in this namespace.
    pub arg0namespace: Option<String>,
    /// Eavesdrop flag (usually ignored for security).
    pub eavesdrop: bool,
}

impl MatchRule {
    /// Parse a match rule string.
    ///
    /// Match rules are comma-separated key=value pairs.
    /// Example: "type='signal',interface='org.freedesktop.DBus',member='NameOwnerChanged'"
    pub fn parse(rule: &str) -> Result<Self, MatchRuleError> {
        let mut result = MatchRule {
            rule_string: rule.to_string(),
            msg_type: None,
            sender: None,
            interface: None,
            member: None,
            path: None,
            path_namespace: None,
            destination: None,
            args: HashMap::new(),
            arg_paths: HashMap::new(),
            arg0namespace: None,
            eavesdrop: false,
        };

        // Handle empty rule (matches everything)
        if rule.trim().is_empty() {
            return Ok(result);
        }

        // Parse comma-separated key='value' pairs
        let mut remaining = rule.trim();
        
        while !remaining.is_empty() {
            // Skip leading whitespace and commas
            remaining = remaining.trim_start();
            if remaining.starts_with(',') {
                remaining = &remaining[1..];
                remaining = remaining.trim_start();
            }
            if remaining.is_empty() {
                break;
            }

            // Find the key
            let eq_pos = remaining.find('=')
                .ok_or_else(|| MatchRuleError::InvalidFormat(format!("Missing '=' in: {}", remaining)))?;
            
            let key = remaining[..eq_pos].trim();
            remaining = &remaining[eq_pos + 1..];

            // Parse the value (must be quoted)
            remaining = remaining.trim_start();
            let value = if remaining.starts_with('\'') {
                // Find closing quote
                let end = remaining[1..].find('\'')
                    .ok_or_else(|| MatchRuleError::InvalidFormat("Unclosed quote".to_string()))?;
                let val = &remaining[1..end + 1];
                remaining = &remaining[end + 2..];
                val.to_string()
            } else {
                // Unquoted value - read until comma or end
                let end = remaining.find(',').unwrap_or(remaining.len());
                let val = remaining[..end].trim();
                remaining = &remaining[end..];
                val.to_string()
            };

            // Process the key-value pair
            match key {
                "type" => result.msg_type = Some(value),
                "sender" => result.sender = Some(value),
                "interface" => result.interface = Some(value),
                "member" => result.member = Some(value),
                "path" => result.path = Some(value),
                "path_namespace" => result.path_namespace = Some(value),
                "destination" => result.destination = Some(value),
                "eavesdrop" => result.eavesdrop = value == "true",
                "arg0namespace" => result.arg0namespace = Some(value),
                key if key.starts_with("arg") && key.ends_with("path") => {
                    // argNpath
                    let num_str = &key[3..key.len() - 4];
                    let num: u8 = num_str.parse()
                        .map_err(|_| MatchRuleError::InvalidArgIndex(key.to_string()))?;
                    if num > 63 {
                        return Err(MatchRuleError::InvalidArgIndex(key.to_string()));
                    }
                    result.arg_paths.insert(num, value);
                }
                key if key.starts_with("arg") => {
                    // argN
                    let num_str = &key[3..];
                    let num: u8 = num_str.parse()
                        .map_err(|_| MatchRuleError::InvalidArgIndex(key.to_string()))?;
                    if num > 63 {
                        return Err(MatchRuleError::InvalidArgIndex(key.to_string()));
                    }
                    result.args.insert(num, value);
                }
                _ => {
                    // Unknown keys are ignored per spec
                    trace!(key = key, "Ignoring unknown match rule key");
                }
            }
        }

        Ok(result)
    }

    /// Check if a message matches this rule.
    pub fn matches(&self, msg: &Message) -> bool {
        // Check message type
        if let Some(ref type_filter) = self.msg_type {
            let msg_type_str = match msg.primary_header().msg_type() {
                zbus::message::Type::MethodCall => "method_call",
                zbus::message::Type::MethodReturn => "method_return",
                zbus::message::Type::Error => "error",
                zbus::message::Type::Signal => "signal",
            };
            if type_filter != msg_type_str {
                return false;
            }
        }

        // Check sender
        if let Some(ref sender_filter) = self.sender {
            match msg.sender_str() {
                Some(sender) if sender == *sender_filter => {}
                Some(_) => return false,
                None => return false,
            }
        }

        // Check interface
        if let Some(ref iface_filter) = self.interface {
            match msg.interface_str() {
                Some(iface) if iface == *iface_filter => {}
                Some(_) => return false,
                None => return false,
            }
        }

        // Check member
        if let Some(ref member_filter) = self.member {
            match msg.member_str() {
                Some(member) if member == *member_filter => {}
                Some(_) => return false,
                None => return false,
            }
        }

        // Check path
        if let Some(ref path_filter) = self.path {
            match msg.path_str() {
                Some(path) if path == *path_filter => {}
                Some(_) => return false,
                None => return false,
            }
        }

        // Check path_namespace (matches path and all children)
        if let Some(ref ns_filter) = self.path_namespace {
            match msg.path_str() {
                Some(path) => {
                    if path != *ns_filter && !path.starts_with(&format!("{}/", ns_filter)) {
                        return false;
                    }
                }
                None => return false,
            }
        }

        // Check destination
        if let Some(ref dest_filter) = self.destination {
            match msg.destination_str() {
                Some(dest) if dest == *dest_filter => {}
                Some(_) => return false,
                None if dest_filter.is_empty() => {}
                None => return false,
            }
        }

        // Check argument filters
        if !self.args.is_empty() || !self.arg_paths.is_empty() || self.arg0namespace.is_some() {
            if let Ok(args) = self.extract_string_args(msg) {
                for (idx, expected) in &self.args {
                    match args.get(*idx as usize) {
                        Some(actual) if actual == expected => {}
                        _ => return false,
                    }
                }
                
                for (idx, path_filter) in &self.arg_paths {
                    match args.get(*idx as usize) {
                        Some(actual) => {
                            if !matches_path_filter(actual, path_filter) {
                                return false;
                            }
                        }
                        None => return false,
                    }
                }

                // Check arg0namespace: matches if arg0 equals the namespace or starts with "namespace."
                if let Some(ref ns) = self.arg0namespace {
                    match args.first() {
                        Some(arg0) => {
                            if arg0 != ns && !arg0.starts_with(&format!("{}.", ns)) {
                                return false;
                            }
                        }
                        None => return false,
                    }
                }
            } else {
                // Can't extract args - fail the match if we have arg filters
                return false;
            }
        }

        true
    }

    /// Check if a message matches this rule, ignoring the destination filter.
    /// 
    /// This is used for unicast signals sent to the mux's unique name on upstream buses.
    /// The mux forwards calls on behalf of clients, so signals addressed to the mux
    /// should be delivered to clients based on other criteria (path, interface, etc.),
    /// not the destination filter which would contain the mux's unique name.
    pub fn matches_ignoring_destination(&self, msg: &Message) -> bool {
        // Check message type
        if let Some(ref type_filter) = self.msg_type {
            let msg_type_str = match msg.primary_header().msg_type() {
                zbus::message::Type::MethodCall => "method_call",
                zbus::message::Type::MethodReturn => "method_return",
                zbus::message::Type::Error => "error",
                zbus::message::Type::Signal => "signal",
            };
            if type_filter != msg_type_str {
                return false;
            }
        }

        // Check sender
        if let Some(ref sender_filter) = self.sender {
            match msg.sender_str() {
                Some(sender) if sender == *sender_filter => {}
                Some(_) => return false,
                None => return false,
            }
        }

        // Check interface
        if let Some(ref iface_filter) = self.interface {
            match msg.interface_str() {
                Some(iface) if iface == *iface_filter => {}
                Some(_) => return false,
                None => return false,
            }
        }

        // Check member
        if let Some(ref member_filter) = self.member {
            match msg.member_str() {
                Some(member) if member == *member_filter => {}
                Some(_) => return false,
                None => return false,
            }
        }

        // Check path
        if let Some(ref path_filter) = self.path {
            match msg.path_str() {
                Some(path) if path == *path_filter => {}
                Some(_) => return false,
                None => return false,
            }
        }

        // Check path_namespace (matches path and all children)
        if let Some(ref ns_filter) = self.path_namespace {
            match msg.path_str() {
                Some(path) => {
                    if path != *ns_filter && !path.starts_with(&format!("{}/", ns_filter)) {
                        return false;
                    }
                }
                None => return false,
            }
        }

        // NOTE: We intentionally skip the destination filter here.

        // Check argument filters
        if !self.args.is_empty() || !self.arg_paths.is_empty() || self.arg0namespace.is_some() {
            if let Ok(args) = self.extract_string_args(msg) {
                for (idx, expected) in &self.args {
                    match args.get(*idx as usize) {
                        Some(actual) if actual == expected => {}
                        _ => return false,
                    }
                }
                
                for (idx, path_filter) in &self.arg_paths {
                    match args.get(*idx as usize) {
                        Some(actual) => {
                            if !matches_path_filter(actual, path_filter) {
                                return false;
                            }
                        }
                        None => return false,
                    }
                }

                if let Some(ref ns) = self.arg0namespace {
                    match args.first() {
                        Some(arg0) => {
                            if arg0 != ns && !arg0.starts_with(&format!("{}.", ns)) {
                                return false;
                            }
                        }
                        None => return false,
                    }
                }
            } else {
                return false;
            }
        }

        true
    }

    /// Extract string arguments from a message body.
    /// 
    /// This extracts arguments by position, returning the string value for each
    /// position that contains a STRING or OBJECT_PATH type. Non-string arguments
    /// are represented as empty strings in the returned vector.
    fn extract_string_args(&self, msg: &Message) -> Result<Vec<String>, ()> {
        use zbus::zvariant::Value;
        
        let body = msg.body();
        
        // Get the max arg index we need to extract
        let max_idx = self.args.keys()
            .chain(self.arg_paths.keys())
            .max()
            .copied()
            .unwrap_or(0);
        
        // Also check arg0namespace
        let max_idx = if self.arg0namespace.is_some() {
            max_idx.max(0)
        } else {
            max_idx
        };
        
        let mut result = Vec::new();
        
        // Try to deserialize as a structure containing the arguments
        // We use zvariant's Structure type to access arguments by position
        if let Ok(structure) = body.deserialize::<zbus::zvariant::Structure>() {
            for (idx, field) in structure.fields().iter().enumerate() {
                if idx as u8 > max_idx {
                    break;
                }
                // Extract string or object path values
                let s = match field {
                    Value::Str(s) => s.to_string(),
                    Value::ObjectPath(p) => p.to_string(),
                    _ => String::new(), // Non-string type, use empty placeholder
                };
                result.push(s);
            }
            return Ok(result);
        }
        
        // Try common tuple patterns as fallback
        // Single string
        if let Ok(s) = body.deserialize::<String>() {
            return Ok(vec![s]);
        }
        
        // Try as tuple of strings (common for NameOwnerChanged)
        if let Ok((s1, s2, s3)) = body.deserialize::<(String, String, String)>() {
            return Ok(vec![s1, s2, s3]);
        }
        
        if let Ok((s1, s2)) = body.deserialize::<(String, String)>() {
            return Ok(vec![s1, s2]);
        }
        
        if let Ok((s1,)) = body.deserialize::<(String,)>() {
            return Ok(vec![s1]);
        }

        Err(())
    }
}

/// Check if an argument value matches a path filter according to D-Bus argNpath rules.
/// 
/// Per the D-Bus specification:
/// "As with normal argument matches, if the argument is exactly equal to the string
/// given in the match rule then the rule is satisfied. Additionally, there is also
/// a match when either the string given in the match rule or the appropriate message
/// argument ends with '/' and is a prefix of the other."
/// 
/// Examples with arg0path='/aa/bb/':
/// - '/' matches (arg ends with '/' and is prefix of filter)
/// - '/aa/' matches (arg ends with '/' and is prefix of filter)  
/// - '/aa/bb/' matches (exact)
/// - '/aa/bb/cc/' matches (filter ends with '/' and is prefix of arg)
/// - '/aa/bb/cc' matches (filter ends with '/' and is prefix of arg)
fn matches_path_filter(arg_value: &str, filter: &str) -> bool {
    // Exact match
    if arg_value == filter {
        return true;
    }
    
    // If filter ends with '/', check if arg starts with filter
    if filter.ends_with('/') && arg_value.starts_with(filter) {
        return true;
    }
    
    // If arg ends with '/', check if filter starts with arg
    if arg_value.ends_with('/') && filter.starts_with(arg_value) {
        return true;
    }
    
    false
}

/// Errors that can occur when parsing match rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatchRuleError {
    /// Invalid match rule format.
    InvalidFormat(String),
    /// Invalid argument index.
    InvalidArgIndex(String),
}

impl std::fmt::Display for MatchRuleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MatchRuleError::InvalidFormat(msg) => write!(f, "Invalid match rule format: {}", msg),
            MatchRuleError::InvalidArgIndex(key) => write!(f, "Invalid argument index: {}", key),
        }
    }
}

impl std::error::Error for MatchRuleError {}

/// Collection of match rules for a client.
#[derive(Debug, Default)]
pub struct ClientMatchRules {
    /// All match rules for this client.
    rules: Vec<MatchRule>,
}

impl ClientMatchRules {
    /// Create a new empty match rule collection.
    pub fn new() -> Self {
        Self { rules: Vec::new() }
    }

    /// Add a match rule.
    pub fn add(&mut self, rule: MatchRule) {
        // Avoid duplicates
        if !self.rules.iter().any(|r| r.rule_string == rule.rule_string) {
            self.rules.push(rule);
        }
    }

    /// Remove a match rule by its original string.
    pub fn remove(&mut self, rule_string: &str) -> bool {
        let len_before = self.rules.len();
        self.rules.retain(|r| r.rule_string != rule_string);
        self.rules.len() < len_before
    }

    /// Check if any rule matches the message.
    pub fn matches(&self, msg: &Message) -> bool {
        self.rules.iter().any(|r| r.matches(msg))
    }

    /// Check if any rule matches the message, ignoring destination filters.
    /// Used for unicast signals addressed to the mux.
    pub fn matches_ignoring_destination(&self, msg: &Message) -> bool {
        self.rules.iter().any(|r| r.matches_ignoring_destination(msg))
    }

    /// Check if there are any rules.
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Get the number of rules.
    pub fn len(&self) -> usize {
        self.rules.len()
    }

    /// Get all rule strings for this client.
    pub fn rule_strings(&self) -> Vec<String> {
        self.rules.iter().map(|r| r.rule_string.clone()).collect()
    }

    /// Get debug info about rules (path filters).
    pub fn debug_rules(&self) -> Vec<String> {
        self.rules.iter()
            .map(|r| {
                format!(
                    "type={:?} iface={:?} member={:?} path={:?} path_ns={:?} sender={:?} dest={:?}",
                    r.msg_type, r.interface, r.member, r.path, r.path_namespace, r.sender, r.destination
                )
            })
            .collect()
    }
}

/// Tracks reference counts for match rules on upstream buses.
///
/// When multiple clients add the same match rule, we only forward it to
/// the upstream bus once. When all clients remove the rule, we remove it
/// from the upstream bus.
#[derive(Debug, Default)]
pub struct MatchRuleRefCount {
    /// Reference counts per rule string for each bus.
    /// Key: (bus_name, rule_string), Value: count
    counts: std::collections::HashMap<(String, String), usize>,
}

impl MatchRuleRefCount {
    /// Create a new empty reference counter.
    pub fn new() -> Self {
        Self {
            counts: std::collections::HashMap::new(),
        }
    }

    /// Increment the reference count for a rule on a bus.
    /// Returns true if this is the first reference (rule should be added to bus).
    pub fn increment(&mut self, bus: &str, rule: &str) -> bool {
        let key = (bus.to_string(), rule.to_string());
        let count = self.counts.entry(key).or_insert(0);
        *count += 1;
        *count == 1
    }

    /// Decrement the reference count for a rule on a bus.
    /// Returns true if this was the last reference (rule should be removed from bus).
    pub fn decrement(&mut self, bus: &str, rule: &str) -> bool {
        let key = (bus.to_string(), rule.to_string());
        if let Some(count) = self.counts.get_mut(&key) {
            if *count > 0 {
                *count -= 1;
                if *count == 0 {
                    self.counts.remove(&key);
                    return true;
                }
            }
        }
        false
    }

    /// Get all rules for a specific bus that have non-zero refcount.
    pub fn rules_for_bus(&self, bus: &str) -> Vec<String> {
        self.counts
            .iter()
            .filter(|((b, _), count)| b == bus && **count > 0)
            .map(|((_, rule), _)| rule.clone())
            .collect()
    }

    /// Remove all references for rules associated with a specific client's rules.
    /// Returns a list of (bus, rule) pairs that should be removed from buses.
    pub fn remove_client_rules(&mut self, bus: &str, rules: &[String]) -> Vec<String> {
        let mut to_remove = Vec::new();
        for rule in rules {
            if self.decrement(bus, rule) {
                to_remove.push(rule.clone());
            }
        }
        to_remove
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_empty_rule() {
        let rule = MatchRule::parse("").unwrap();
        assert!(rule.msg_type.is_none());
        assert!(rule.sender.is_none());
    }

    #[test]
    fn test_parse_type_signal() {
        let rule = MatchRule::parse("type='signal'").unwrap();
        assert_eq!(rule.msg_type, Some("signal".to_string()));
    }

    #[test]
    fn test_parse_multiple_fields() {
        let rule = MatchRule::parse(
            "type='signal',interface='org.freedesktop.DBus',member='NameOwnerChanged'"
        ).unwrap();
        assert_eq!(rule.msg_type, Some("signal".to_string()));
        assert_eq!(rule.interface, Some("org.freedesktop.DBus".to_string()));
        assert_eq!(rule.member, Some("NameOwnerChanged".to_string()));
    }

    #[test]
    fn test_parse_sender_interface() {
        let rule = MatchRule::parse(
            "sender='org.gnome.Shell',interface='org.gnome.Shell.Extensions'"
        ).unwrap();
        assert_eq!(rule.sender, Some("org.gnome.Shell".to_string()));
        assert_eq!(rule.interface, Some("org.gnome.Shell.Extensions".to_string()));
    }

    #[test]
    fn test_parse_path() {
        let rule = MatchRule::parse("path='/org/freedesktop/DBus'").unwrap();
        assert_eq!(rule.path, Some("/org/freedesktop/DBus".to_string()));
    }

    #[test]
    fn test_parse_path_namespace() {
        let rule = MatchRule::parse("path_namespace='/org/freedesktop'").unwrap();
        assert_eq!(rule.path_namespace, Some("/org/freedesktop".to_string()));
    }

    #[test]
    fn test_parse_arg0() {
        let rule = MatchRule::parse("arg0='test.Name'").unwrap();
        assert_eq!(rule.args.get(&0), Some(&"test.Name".to_string()));
    }

    #[test]
    fn test_parse_arg0path() {
        let rule = MatchRule::parse("arg0path='/org/example'").unwrap();
        assert_eq!(rule.arg_paths.get(&0), Some(&"/org/example".to_string()));
    }

    #[test]
    fn test_parse_complex_rule() {
        let rule = MatchRule::parse(
            "type='signal',sender='org.freedesktop.DBus',interface='org.freedesktop.DBus',member='NameOwnerChanged',path='/org/freedesktop/DBus',arg0='com.example.Service'"
        ).unwrap();
        assert_eq!(rule.msg_type, Some("signal".to_string()));
        assert_eq!(rule.sender, Some("org.freedesktop.DBus".to_string()));
        assert_eq!(rule.interface, Some("org.freedesktop.DBus".to_string()));
        assert_eq!(rule.member, Some("NameOwnerChanged".to_string()));
        assert_eq!(rule.path, Some("/org/freedesktop/DBus".to_string()));
        assert_eq!(rule.args.get(&0), Some(&"com.example.Service".to_string()));
    }

    #[test]
    fn test_parse_eavesdrop() {
        let rule = MatchRule::parse("eavesdrop='true'").unwrap();
        assert!(rule.eavesdrop);
        
        let rule = MatchRule::parse("eavesdrop='false'").unwrap();
        assert!(!rule.eavesdrop);
    }

    #[test]
    fn test_parse_invalid_format() {
        // Missing equals
        assert!(MatchRule::parse("type").is_err());
    }

    #[test]
    fn test_parse_unclosed_quote() {
        assert!(MatchRule::parse("type='signal").is_err());
    }

    #[test]
    fn test_parse_high_arg_index() {
        // arg64 is invalid (max is 63)
        assert!(MatchRule::parse("arg64='test'").is_err());
        
        // arg63 is valid
        assert!(MatchRule::parse("arg63='test'").is_ok());
    }

    #[test]
    fn test_client_match_rules_add_remove() {
        let mut rules = ClientMatchRules::new();
        assert!(rules.is_empty());
        
        let rule = MatchRule::parse("type='signal'").unwrap();
        rules.add(rule);
        assert_eq!(rules.len(), 1);
        
        // Adding duplicate should not increase count
        let rule2 = MatchRule::parse("type='signal'").unwrap();
        rules.add(rule2);
        assert_eq!(rules.len(), 1);
        
        // Add different rule
        let rule3 = MatchRule::parse("type='method_call'").unwrap();
        rules.add(rule3);
        assert_eq!(rules.len(), 2);
        
        // Remove
        assert!(rules.remove("type='signal'"));
        assert_eq!(rules.len(), 1);
        
        // Remove non-existent
        assert!(!rules.remove("type='signal'"));
        assert_eq!(rules.len(), 1);
    }

    #[test]
    fn test_parse_with_whitespace() {
        let rule = MatchRule::parse("  type='signal' , interface='org.example'  ").unwrap();
        assert_eq!(rule.msg_type, Some("signal".to_string()));
        assert_eq!(rule.interface, Some("org.example".to_string()));
    }

    #[test]
    fn test_parse_destination() {
        let rule = MatchRule::parse("destination=':1.42'").unwrap();
        assert_eq!(rule.destination, Some(":1.42".to_string()));
    }

    #[test]
    fn test_match_rule_refcount_basic() {
        let mut refcount = MatchRuleRefCount::new();
        
        // First add returns true (should add to bus)
        assert!(refcount.increment("container", "type='signal'"));
        
        // Second add returns false (already on bus)
        assert!(!refcount.increment("container", "type='signal'"));
        
        // First remove returns false (still has references)
        assert!(!refcount.decrement("container", "type='signal'"));
        
        // Second remove returns true (last reference, should remove from bus)
        assert!(refcount.decrement("container", "type='signal'"));
        
        // Decrement on non-existent rule returns false
        assert!(!refcount.decrement("container", "type='signal'"));
    }

    #[test]
    fn test_match_rule_refcount_multiple_buses() {
        let mut refcount = MatchRuleRefCount::new();
        
        // Add same rule to different buses
        assert!(refcount.increment("container", "type='signal'"));
        assert!(refcount.increment("host", "type='signal'"));
        
        // Remove from one bus doesn't affect the other
        assert!(refcount.decrement("container", "type='signal'"));
        assert!(refcount.decrement("host", "type='signal'"));
    }

    #[test]
    fn test_match_rule_refcount_rules_for_bus() {
        let mut refcount = MatchRuleRefCount::new();
        
        refcount.increment("container", "type='signal'");
        refcount.increment("container", "interface='org.example'");
        refcount.increment("host", "type='signal'");
        
        let container_rules = refcount.rules_for_bus("container");
        assert_eq!(container_rules.len(), 2);
        assert!(container_rules.contains(&"type='signal'".to_string()));
        assert!(container_rules.contains(&"interface='org.example'".to_string()));
        
        let host_rules = refcount.rules_for_bus("host");
        assert_eq!(host_rules.len(), 1);
        assert!(host_rules.contains(&"type='signal'".to_string()));
    }

    #[test]
    fn test_client_match_rules_rule_strings() {
        let mut rules = ClientMatchRules::new();
        rules.add(MatchRule::parse("type='signal'").unwrap());
        rules.add(MatchRule::parse("interface='org.example'").unwrap());
        
        let strings = rules.rule_strings();
        assert_eq!(strings.len(), 2);
        assert!(strings.contains(&"type='signal'".to_string()));
        assert!(strings.contains(&"interface='org.example'".to_string()));
    }
}
