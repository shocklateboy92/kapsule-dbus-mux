//! kapsule-dbus-mux - D-Bus multiplexer for container/host bus routing.
//!
//! This crate provides a D-Bus multiplexer that routes messages between a
//! container's internal bus and the host session bus, enabling container
//! applications to seamlessly access host services while allowing
//! container-local services to take priority.

pub mod auth;
pub mod bus_connection;
pub mod client;
pub mod error;
pub mod match_rules;
pub mod message;
pub mod multiplexer;
pub mod name_queue;
pub mod portal_request;
pub mod router;
pub mod routing;
pub mod serial_map;

pub use error::{Error, Result};
pub use multiplexer::Multiplexer;
pub use routing::{Route, RoutingTable};
