//! skills.rs - Infinite Skills. Finite Context.
//!
//! Unified MCP server for discovering tools and loading portable skill packages.
//! Exposes exactly 3 tools to the host LLM: search, exec, manage.

pub mod core;
pub mod execution;
pub mod mcp;
pub mod paths;
pub mod storage;

pub use core::policy::{ConsentLevel, PolicyConfig, PolicyEngine, PolicyMode, PolicyState};
pub use core::registry::{Registry, ServerHealth, ServerInfo};
pub use core::{
    BundledTool, CallableId, CallableKind, CallableRecord, CallableSignature, CostHints,
    ResourceContent, RiskTier, SchemaDigest, ToolDefinition, ToolResult, ToolResultContent,
};

pub use execution::upstream::{
    ConnectionState, Transport, UpstreamConfig, UpstreamError, UpstreamManager, UpstreamStatus,
};
pub use execution::{
    sandbox::{
        DockerConfig, DockerMount, Sandbox, SandboxBackend, SandboxConfig, SandboxConfigOverride,
        SandboxError, SandboxPreset, SandboxResult,
    },
    ExecContext, ExecutionTrace, Runtime, RuntimeError, TraceStep,
};

pub use mcp::SkillsServer;

pub use storage::search::{
    IndexError, SearchEngine, SearchFilters, SearchMatch, SearchQuery, SearchResults,
};
pub use storage::{
    agent_skills, sync, CreateSkillRequest, EntrypointType, Skill, SkillContent, SkillHints,
    SkillManifest, SkillStore, SkillStoreError, ToolPolicy, UpdateSkillRequest, ValidationResult,
};

// Re-export common dependencies that users might need
pub use anyhow;
pub use serde;
pub use serde_json;
pub use tokio;
pub use tracing;
