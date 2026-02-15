//! skills.rs - Infinite Skills. Finite Context.
//!
//! Unified MCP server aggregating tools and skills with 99% token reduction.
//! Exposes exactly 4 tools to the host LLM: search, schema, exec, manage.

pub mod core;
pub mod execution;
pub mod mcp;
pub mod paths;
pub mod storage;

pub use core::policy::{ConsentLevel, PolicyConfig, PolicyEngine};
pub use core::registry::{Registry, ServerHealth, ServerInfo};
pub use core::{
    BundledTool, CallableId, CallableKind, CallableRecord, CallableSignature, CostHints,
    ResourceContent, RiskTier, SchemaDigest, ToolDefinition, ToolResult, ToolResultContent,
};

pub use execution::upstream::{
    ConnectionState, Transport, UpstreamConfig, UpstreamError, UpstreamManager,
};
pub use execution::{
    sandbox::{Sandbox, SandboxBackend, SandboxConfig, SandboxError, SandboxResult},
    ExecContext, ExecutionTrace, Runtime, RuntimeError, TraceStep, WorkflowEngine,
};

pub use mcp::SkillsServer;

pub use storage::search::{
    IndexError, SearchEngine, SearchFilters, SearchMatch, SearchQuery, SearchResults,
};
pub use storage::{
    agent_skills, sync, CreateSkillRequest, EntrypointType, Skill, SkillContent, SkillHints,
    SkillManifest, SkillStore, SkillStoreError, ToolPolicy, ValidationResult,
};

// Re-export common dependencies that users might need
pub use anyhow;
pub use serde;
pub use serde_json;
pub use tokio;
pub use tracing;
