//! kapsule-dbus-mux - D-Bus multiplexer for container/host bus routing.
//!
//! This binary provides a D-Bus multiplexer that routes messages between a
//! container's internal bus and the host session bus.

use std::path::PathBuf;

use clap::Parser;
use tracing::{error, info};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

use kapsule_dbus_mux::{Multiplexer, Result};

/// D-Bus multiplexer for container/host bus routing.
#[derive(Parser, Debug)]
#[command(name = "kapsule-dbus-mux")]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Socket path for clients to connect to.
    #[arg(long, short = 'l')]
    listen: PathBuf,

    /// Container's internal D-Bus socket address.
    ///
    /// Should be in D-Bus address format, e.g., "unix:path=/path/to/socket".
    #[arg(long, short = 'c')]
    container_bus: String,

    /// Host session bus address.
    ///
    /// If not specified, uses the DBUS_SESSION_BUS_ADDRESS environment variable.
    #[arg(long, short = 'H')]
    host_bus: Option<String>,

    /// Log level filter (e.g., "debug", "info", "warn", "error").
    #[arg(long, short = 'v', default_value = "info")]
    log_level: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Set up tracing/logging
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(&args.log_level));

    tracing_subscriber::registry()
        .with(fmt::layer().with_target(true))
        .with(filter)
        .init();

    info!(
        listen = %args.listen.display(),
        container_bus = %args.container_bus,
        host_bus = ?args.host_bus,
        "Starting kapsule-dbus-mux"
    );

    // Create and run the multiplexer
    let mux = Multiplexer::new(
        args.listen,
        &args.container_bus,
        args.host_bus.as_deref(),
    ).await?;

    // Handle shutdown signals
    let shutdown = async {
        tokio::signal::ctrl_c().await.ok();
        info!("Received shutdown signal");
    };

    tokio::select! {
        result = mux.run() => {
            if let Err(e) = result {
                error!(error = %e, "Multiplexer error");
                return Err(e);
            }
        }
        _ = shutdown => {
            info!("Shutting down");
        }
    }

    Ok(())
}
