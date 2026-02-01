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
        if !self.args.is_empty() || !self.arg_paths.is_empty() {
            // Try to deserialize body as tuple of strings
            // This is a simplified implementation - full implementation would
            // need to handle various D-Bus types
            if let Ok(args) = self.extract_string_args(msg) {
                for (idx, expected) in &self.args {
                    match args.get(*idx as usize) {
                        Some(actual) if actual == expected => {}
                        _ => return false,
                    }
                }
                
                for (idx, path_prefix) in &self.arg_paths {
                    match args.get(*idx as usize) {
                        Some(actual) => {
                            if actual != path_prefix && 
                               !actual.starts_with(&format!("{}/", path_prefix)) {
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

    /// Extract string arguments from a message body.
    fn extract_string_args(&self, msg: &Message) -> Result<Vec<String>, ()> {
        // Try to extract up to 10 string arguments
        // This handles common cases like NameOwnerChanged(s, s, s)
        let body = msg.body();
        
        // Try as single string
        if let Ok(s) = body.deserialize::<String>() {
            return Ok(vec![s]);
        }
        
        // Try as tuple of strings
        if let Ok((s1, s2, s3)) = body.deserialize::<(String, String, String)>() {
            return Ok(vec![s1, s2, s3]);
        }
        
        if let Ok((s1, s2)) = body.deserialize::<(String, String)>() {
            return Ok(vec![s1, s2]);
        }
        
        if let Ok((s1,)) = body.deserialize::<(String,)>() {
            return Ok(vec![s1]);
        }

        // Try as Vec<String>
        if let Ok(v) = body.deserialize::<Vec<String>>() {
            return Ok(v);
        }

        Err(())
    }
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

    /// Check if there are any rules.
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Get the number of rules.
    pub fn len(&self) -> usize {
        self.rules.len()
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
}
