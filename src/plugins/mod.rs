pub mod registry;
pub mod router;
pub mod executor;

pub use registry::{Plugin, PluginRegistry, GetError};
pub use router::route;
pub use executor::execute_plugin;
pub use executor::{invoke_plugin_tool, list_plugins_tool, list_plugin_commands_tool, spawn_agents_tool};
