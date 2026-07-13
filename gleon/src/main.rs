//! Gleon CLI wrapper binary.

use tracing::info;

fn main() {
    // Initialize tracing subscriber for logging
    tracing_subscriber::fmt::init();

    info!("Gleon CLI starting up...");
    println!("Hello, world!");
}
