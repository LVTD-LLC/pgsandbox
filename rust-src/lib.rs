pub mod cli;
pub mod config;
pub mod doctor;
pub mod mcp;
pub mod names;
pub mod postgres;
pub mod setup;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
