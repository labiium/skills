//! Skills.rs - Infinite Skills. Finite Context.
//!
//! A Rust implementation of a unified MCP server that aggregates upstream MCP servers
//! and local Skills into a unified registry, exposing exactly three MCP tools:
//! - `skills.search`: Search for available callables
//! - `skills.schema`: Retrieve schema information for a callable
//! - `skills.exec`: Execute a callable with validation and policy enforcement

// Re-export all public modules
#[allow(ambiguous_glob_reexports)]
pub use skillsrs_core::*;
#[allow(ambiguous_glob_reexports)]
pub use skillsrs_index::*;
#[allow(ambiguous_glob_reexports)]
pub use skillsrs_mcp::*;
#[allow(ambiguous_glob_reexports)]
pub use skillsrs_policy::*;
#[allow(ambiguous_glob_reexports)]
pub use skillsrs_registry::*;
#[allow(ambiguous_glob_reexports)]
pub use skillsrs_runtime::*;
#[allow(ambiguous_glob_reexports)]
pub use skillsrs_skillstore::*;
#[allow(ambiguous_glob_reexports)]
pub use skillsrs_upstream::*;
