pub mod registry;
pub mod router;
pub mod executor;

pub use registry::{Plugin, PluginRegistry, GetError};
pub use router::route;
pub use executor::{execute_plugin, execute_plugin_via_cli, execute_plugin_via_cli_streaming, CliResult};
