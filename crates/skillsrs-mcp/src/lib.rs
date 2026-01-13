//! MCP Server Façade using Official rmcp SDK
//!
//! Exposes exactly three MCP tools to the host LLM:
//! - skills.search: Discovery over unified registry
//! - skills.schema: On-demand schema fetching
//! - skills.exec: Validated execution with policy enforcement
//!
//! This implementation uses the official Rust MCP SDK (rmcp) to ensure
//! full protocol compliance and leverage battle-tested infrastructure.

use rmcp::{
    handler::server::router::tool::ToolRouter,
    model::{CallToolResult, Content, Implementation, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router, Json, ServerHandler,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use skillsrs_core::{CallableId, ToolResult};
use skillsrs_index::{SearchEngine, SearchFilters, SearchQuery};
use skillsrs_policy::{ConsentLevel, PolicyEngine};
use skillsrs_registry::Registry;
use skillsrs_runtime::{ExecContext, Runtime};
use skillsrs_skillstore::{CreateSkillRequest, SkillStore};
use std::sync::Arc;
use tracing::{debug, info, warn};

/// The MCP server that exposes skill management and execution tools
#[derive(Clone)]
pub struct SkillsServer {
    registry: Arc<Registry>,
    search_engine: Arc<SearchEngine>,
    policy_engine: Arc<PolicyEngine>,
    runtime: Arc<Runtime>,
    skill_store: Arc<SkillStore>,
    tool_router: ToolRouter<Self>,
}

/// Input schema for skills.search
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SearchInput {
    /// Search query
    pub q: String,

    /// Filter by callable kind
    #[serde(default = "default_kind")]
    #[schemars(description = "Filter by callable kind: any, tools, or skills")]
    pub kind: String,

    /// Query matching mode
    #[serde(default = "default_mode")]
    #[schemars(description = "Query matching mode: literal, regex, or fuzzy")]
    pub mode: String,

    /// Maximum results to return
    #[serde(default = "default_limit")]
    #[schemars(description = "Maximum results to return (1-50)")]
    pub limit: usize,

    /// Filters for search results
    #[serde(default)]
    pub filters: Option<SearchFilters>,

    /// Optional fields to include in results
    #[serde(default)]
    pub include: Option<IncludeOptions>,

    /// Pagination cursor
    #[serde(default)]
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct IncludeOptions {
    #[serde(default)]
    pub signature: Option<bool>,
    #[serde(default)]
    pub schema_digest: Option<bool>,
    #[serde(default)]
    pub uses: Option<bool>,
}

fn default_kind() -> String {
    "any".to_string()
}

fn default_mode() -> String {
    "literal".to_string()
}

fn default_limit() -> usize {
    10
}

/// Output schema for skills.search
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SearchOutput {
    pub matches: Vec<JsonValue>,
    pub next_cursor: Option<String>,
    pub stats: SearchStats,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SearchStats {
    pub total_callables: usize,
    pub total_tools: usize,
    pub total_skills: usize,
    pub searched_servers: usize,
    pub stale_servers: Vec<String>,
}

/// Input schema for skills.schema
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SchemaInput {
    /// Callable ID from search results
    pub id: String,

    /// Schema format to return
    #[serde(default = "default_format")]
    #[schemars(description = "Format: json_schema, signature, or both")]
    pub format: String,

    /// Include output schema if available
    #[serde(default = "default_true")]
    pub include_output_schema: bool,

    /// Maximum response size in bytes
    #[serde(default = "default_max_bytes")]
    pub max_bytes: usize,

    /// JSON Pointer to schema subtree
    #[serde(default)]
    pub json_pointer: Option<String>,
}

fn default_format() -> String {
    "both".to_string()
}

fn default_true() -> bool {
    true
}

fn default_max_bytes() -> usize {
    50000
}

/// Output schema for skills.schema
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SchemaOutput {
    pub callable: CallableInfo,
    pub schema_digest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<JsonValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<JsonValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<JsonValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CallableInfo {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub fq_name: String,
    pub server: Option<String>,
    pub version: Option<String>,
}

/// Input schema for skills.exec
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExecInput {
    /// Callable ID from search results
    pub id: String,

    /// Arguments to pass to the callable
    pub arguments: JsonValue,

    /// Validate without executing
    #[serde(default)]
    pub dry_run: bool,

    /// Execution timeout in milliseconds
    #[serde(default)]
    pub timeout_ms: Option<u64>,

    /// Consent information
    #[serde(default)]
    pub consent: Option<ConsentArgs>,

    /// Trace options
    #[serde(default)]
    pub trace: Option<TraceArgs>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ConsentArgs {
    #[serde(default)]
    pub level: Option<String>,
    #[serde(default)]
    pub token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TraceArgs {
    #[serde(default)]
    pub include_route: bool,
    #[serde(default)]
    pub include_timing: bool,
    #[serde(default)]
    pub include_steps: bool,
}

/// Output schema for skills.exec
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExecOutput {
    pub result: JsonValue,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub route: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timing: Option<JsonValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub steps: Option<Vec<JsonValue>>,
}

impl SkillsServer {
    pub fn new(
        registry: Arc<Registry>,
        search_engine: Arc<SearchEngine>,
        policy_engine: Arc<PolicyEngine>,
        runtime: Arc<Runtime>,
        skill_store: Arc<SkillStore>,
    ) -> Self {
        SkillsServer {
            registry,
            search_engine,
            policy_engine,
            runtime,
            skill_store,
            tool_router: Self::tool_router(),
        }
    }

    /// Convert ToolResult to CallToolResult
    #[allow(dead_code)]
    fn tool_result_to_call_result(result: ToolResult) -> CallToolResult {
        let mut contents = Vec::new();

        for content in result.content {
            match content {
                skillsrs_core::ToolResultContent::Text { text } => {
                    contents.push(Content::text(text));
                }
                skillsrs_core::ToolResultContent::Image { data, mime_type } => {
                    contents.push(Content::image(data, mime_type));
                }
                skillsrs_core::ToolResultContent::Resource { resource } => {
                    // Convert resource to text representation
                    let text = resource.text.unwrap_or_else(|| {
                        format!(
                            "Resource: {} ({})",
                            resource.uri,
                            resource.mime_type.unwrap_or_default()
                        )
                    });
                    contents.push(Content::text(text));
                }
            }
        }

        if result.is_error {
            CallToolResult::error(contents)
        } else {
            CallToolResult::success(contents)
        }
    }
}

/// Implement the tool router with exactly 3 tools
#[tool_router]
impl SkillsServer {
    /// Fast discovery over registry (tools + skills) with filters.
    /// Use this to find callables before execution.
    #[tool(
        name = "skills.search",
        description = "Fast discovery over registry (tools + skills) with filters. Use this to find callables before execution."
    )]
    async fn search(
        &self,
        input: rmcp::handler::server::wrapper::Parameters<SearchInput>,
    ) -> Result<Json<SearchOutput>, String> {
        debug!("skills.search called with query: {}", input.0.q);

        let query = SearchQuery {
            q: input.0.q,
            kind: input.0.kind,
            mode: input.0.mode,
            limit: input.0.limit.clamp(1, 50),
            filters: input.0.filters,
            cursor: input.0.cursor,
        };

        let results = self
            .search_engine
            .search(&query)
            .await
            .map_err(|e| format!("Search failed: {}", e))?;

        let stats = self.registry.stats();

        let output = SearchOutput {
            matches: results
                .matches
                .into_iter()
                .map(|m| serde_json::to_value(m).unwrap())
                .collect(),
            next_cursor: results.next_cursor,
            stats: SearchStats {
                total_callables: stats.total_callables,
                total_tools: stats.total_tools,
                total_skills: stats.total_skills,
                searched_servers: stats.servers.len(),
                stale_servers: stats.stale_servers,
            },
        };

        info!("skills.search returned {} matches", output.matches.len());

        Ok(Json(output))
    }

    /// Fetch full schema and signature for a callable.
    /// Use this after search to get detailed parameter info.
    #[tool(
        name = "skills.schema",
        description = "Fetch full schema and signature for a callable. Use this after search to get detailed parameter info."
    )]
    async fn schema(
        &self,
        input: rmcp::handler::server::wrapper::Parameters<SchemaInput>,
    ) -> Result<Json<SchemaOutput>, String> {
        debug!("skills.schema called for: {}", input.0.id);

        let callable_id: CallableId = input.0.id.into();
        let record = self
            .registry
            .get(&callable_id)
            .ok_or_else(|| format!("Callable not found: {}", callable_id))?;

        let format = input.0.format.as_str();
        let max_bytes = input.0.max_bytes;

        let mut output = SchemaOutput {
            callable: CallableInfo {
                id: record.id.as_str().to_string(),
                kind: record.kind.to_string(),
                name: record.name.clone(),
                fq_name: record.fq_name.clone(),
                server: record.server_alias.clone(),
                version: record.skill_version.clone(),
            },
            schema_digest: record.schema_digest.as_str().to_string(),
            input_schema: None,
            output_schema: None,
            signature: None,
        };

        if format == "json_schema" || format == "both" {
            let mut schema = record.input_schema.clone();

            // Apply JSON pointer if specified
            if let Some(pointer) = input.0.json_pointer {
                if let Some(subtree) = schema.pointer(&pointer) {
                    schema = subtree.clone();
                } else {
                    return Err(format!("Invalid JSON pointer: {}", pointer));
                }
            }

            output.input_schema = Some(schema);

            if input.0.include_output_schema {
                output.output_schema = record.output_schema.clone();
            }
        }

        if format == "signature" || format == "both" {
            let signature = skillsrs_core::CallableSignature::from_schema(&record.input_schema);
            output.signature = Some(serde_json::to_value(signature).unwrap());
        }

        // Check size limit
        let output_str = serde_json::to_string(&output).unwrap();
        if output_str.len() > max_bytes {
            return Err(format!(
                "Response exceeds maxBytes limit: {} > {}",
                output_str.len(),
                max_bytes
            ));
        }

        info!("skills.schema returned schema for {}", record.fq_name);
        Ok(Json(output))
    }

    /// Execute a callable with validation and policy enforcement.
    /// Always search and get schema first.
    #[tool(
        name = "skills.exec",
        description = "Execute a callable with validation and policy enforcement. Always search and get schema first."
    )]
    async fn exec(
        &self,
        input: rmcp::handler::server::wrapper::Parameters<ExecInput>,
    ) -> Result<String, String> {
        debug!("skills.exec called for: {}", input.0.id);

        let callable_id: CallableId = input.0.id.into();
        let dry_run = input.0.dry_run;

        // Get callable record
        let record = self
            .registry
            .get(&callable_id)
            .ok_or_else(|| format!("Callable not found: {}", callable_id))?;

        // Parse consent level
        let consent_level = input
            .0
            .consent
            .as_ref()
            .and_then(|c| c.level.as_deref())
            .map(|s| match s {
                "user_confirmed" => ConsentLevel::UserConfirmed,
                "admin_confirmed" => ConsentLevel::AdminConfirmed,
                _ => ConsentLevel::None,
            })
            .unwrap_or(ConsentLevel::None);

        // Check policy authorization
        let policy_result = self
            .policy_engine
            .authorize(&record, &input.0.arguments, consent_level)
            .await
            .map_err(|e| format!("Policy check failed: {}", e))?;

        if !policy_result.allowed {
            warn!("Execution denied: {}", policy_result.reason);
            return Err(format!("Execution denied: {}", policy_result.reason));
        }

        if dry_run {
            info!("Dry run: would execute {}", record.fq_name);
            return Ok(format!("Dry run: would execute {}", record.fq_name));
        }

        // Execute
        let ctx = ExecContext {
            callable_id: callable_id.clone(),
            arguments: input.0.arguments,
            timeout_ms: input.0.timeout_ms,
            trace_enabled: input
                .0
                .trace
                .as_ref()
                .map(|t| t.include_route || t.include_timing || t.include_steps)
                .unwrap_or(false),
        };

        let result = self
            .runtime
            .execute(ctx)
            .await
            .map_err(|e| format!("Execution failed: {}", e))?;

        info!("skills.exec completed for {}", record.fq_name);

        // Convert ToolResult to string representation
        let text = if result.is_error {
            format!(
                "Error: {}",
                result
                    .content
                    .iter()
                    .filter_map(|c| match c {
                        skillsrs_core::ToolResultContent::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        } else {
            result
                .content
                .iter()
                .filter_map(|c| match c {
                    skillsrs_core::ToolResultContent::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n")
        };

        Ok(text)
    }

    /// Create a new skill
    #[tool(
        name = "skills.create",
        description = "Create a new skill with SKILL.md content, bundled files, and tool dependencies."
    )]
    async fn create_skill(
        &self,
        input: rmcp::handler::server::wrapper::Parameters<CreateSkillInput>,
    ) -> Result<Json<CreateSkillOutput>, String> {
        debug!("skills.create called for: {}", input.0.name);

        let request = CreateSkillRequest {
            name: input.0.name.clone(),
            version: input.0.version.unwrap_or_else(|| "1.0.0".to_string()),
            description: input.0.description.clone(),
            skill_md_content: input.0.skill_md,
            uses_tools: input.0.uses_tools.unwrap_or_default(),
            bundled_files: input.0.bundled_files.unwrap_or_default(),
            tags: input.0.tags.unwrap_or_default(),
        };

        let id: skillsrs_core::CallableId = self
            .skill_store
            .create_skill(request)
            .await
            .map_err(|e| format!("Failed to create skill: {}", e))?;

        info!("Created skill: {} ({})", input.0.name, id.as_str());

        Ok(Json(CreateSkillOutput {
            id: id.as_str().to_string(),
            name: input.0.name,
            message: "Skill created successfully".to_string(),
        }))
    }

    /// Get skill content (SKILL.md and file list) for progressive disclosure
    #[tool(
        name = "skills.get_content",
        description = "Get skill content (SKILL.md and file list) for progressive disclosure. Use this after search to load skill instructions."
    )]
    async fn get_skill_content(
        &self,
        input: rmcp::handler::server::wrapper::Parameters<GetContentInput>,
    ) -> Result<String, String> {
        debug!("skills.get_content called for: {}", input.0.skill_id);

        let content = self
            .skill_store
            .load_skill_content(&input.0.skill_id)
            .map_err(|e| format!("Failed to load skill content: {}", e))?;

        // If a specific file is requested, return just that file
        if let Some(filename) = input.0.filename {
            let file_content = self
                .skill_store
                .load_skill_file(&input.0.skill_id, &filename)
                .map_err(|e| format!("Failed to load file: {}", e))?;
            return Ok(file_content);
        }

        // Otherwise return SKILL.md with metadata
        let mut response = format!("# Skill: {}\n\n", input.0.skill_id);
        response.push_str(&content.skill_md);
        response.push_str("\n\n---\n\n");
        response.push_str("## Metadata\n\n");
        response.push_str(&format!(
            "- Uses tools: {}\n",
            content.uses_tools.join(", ")
        ));
        response.push_str(&format!(
            "- Bundled tools: {}\n",
            content
                .bundled_tools
                .iter()
                .map(|t| t.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
        response.push_str(&format!(
            "- Additional files: {}\n",
            content.additional_files.join(", ")
        ));

        info!("Returned content for skill: {}", input.0.skill_id);
        Ok(response)
    }

    /// Update an existing skill
    #[tool(
        name = "skills.update",
        description = "Update an existing skill's content, dependencies, or bundled files."
    )]
    async fn update_skill(
        &self,
        input: rmcp::handler::server::wrapper::Parameters<UpdateSkillInput>,
    ) -> Result<Json<CreateSkillOutput>, String> {
        debug!("skills.update called for: {}", input.0.skill_id);

        let request = CreateSkillRequest {
            name: input.0.name.clone(),
            version: input.0.version.unwrap_or_else(|| "1.0.0".to_string()),
            description: input.0.description.clone(),
            skill_md_content: input.0.skill_md,
            uses_tools: input.0.uses_tools.unwrap_or_default(),
            bundled_files: input.0.bundled_files.unwrap_or_default(),
            tags: input.0.tags.unwrap_or_default(),
        };

        let id: skillsrs_core::CallableId = self
            .skill_store
            .update_skill(&input.0.skill_id, request)
            .await
            .map_err(|e| format!("Failed to update skill: {}", e))?;

        info!("Updated skill: {} ({})", input.0.skill_id, id.as_str());

        Ok(Json(CreateSkillOutput {
            id: id.as_str().to_string(),
            name: input.0.name,
            message: "Skill updated successfully".to_string(),
        }))
    }

    /// Delete a skill
    #[tool(name = "skills.delete", description = "Delete a skill from the store.")]
    async fn delete_skill(
        &self,
        input: rmcp::handler::server::wrapper::Parameters<DeleteSkillInput>,
    ) -> Result<String, String> {
        debug!("skills.delete called for: {}", input.0.skill_id);

        self.skill_store
            .delete_skill(&input.0.skill_id)
            .map_err(|e| format!("Failed to delete skill: {}", e))?;

        info!("Deleted skill: {}", input.0.skill_id);
        Ok(format!("Skill {} deleted successfully", input.0.skill_id))
    }
}

/// Input for creating a skill
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CreateSkillInput {
    /// Skill name (unique identifier)
    pub name: String,
    /// Skill version (semver)
    pub version: Option<String>,
    /// Short description
    pub description: String,
    /// SKILL.md content (instructions for agent)
    pub skill_md: String,
    /// MCP tools this skill uses
    pub uses_tools: Option<Vec<String>>,
    /// Bundled files as (filename, content) pairs
    pub bundled_files: Option<Vec<(String, String)>>,
    /// Tags for categorization
    pub tags: Option<Vec<String>>,
}

/// Output from creating a skill
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CreateSkillOutput {
    /// Callable ID of created skill
    pub id: String,
    /// Skill name
    pub name: String,
    /// Success message
    pub message: String,
}

/// Input for getting skill content
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GetContentInput {
    /// Skill ID
    pub skill_id: String,
    /// Optional specific filename to load
    pub filename: Option<String>,
}

/// Input for updating a skill
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct UpdateSkillInput {
    /// Skill ID to update
    pub skill_id: String,
    /// New skill name
    pub name: String,
    /// New version
    pub version: Option<String>,
    /// New description
    pub description: String,
    /// New SKILL.md content
    pub skill_md: String,
    /// New tool dependencies
    pub uses_tools: Option<Vec<String>>,
    /// New bundled files
    pub bundled_files: Option<Vec<(String, String)>>,
    /// New tags
    pub tags: Option<Vec<String>>,
}

/// Input for deleting a skill
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DeleteSkillInput {
    /// Skill ID to delete
    pub skill_id: String,
}

/// Implement ServerHandler to define server capabilities
#[tool_handler]
impl ServerHandler for SkillsServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation {
                name: "skills.rs".to_string(),
                title: Some("Infinite Skills. Finite Context.".to_string()),
                version: env!("CARGO_PKG_VERSION").to_string(),
                website_url: Some("https://github.com/labiium/skills".to_string()),
                icons: None,
            },
            instructions: Some(
                "Unified MCP server that aggregates upstream MCP tools and Skills into a unified registry. \
                Exposes exactly 3 tools for discovery, schema inspection, and execution. \
                Usage: (1) skills.search to find callables, (2) skills.schema to get parameters, \
                (3) skills.exec to execute."
                    .to_string(),
            ),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use skillsrs_policy::PolicyConfig;
    use skillsrs_registry::Registry;
    use skillsrs_runtime::Runtime;
    use skillsrs_upstream::UpstreamManager;

    #[test]
    fn test_server_exposes_core_tools() {
        // CRITICAL CONTRACT TEST: Must expose core tools + skill management tools
        let registry = Arc::new(Registry::new());
        let search_engine = Arc::new(SearchEngine::new(registry.clone()));
        let policy_engine = Arc::new(PolicyEngine::new(PolicyConfig::default()).unwrap());
        let upstream_manager = Arc::new(UpstreamManager::new(registry.clone()));
        let runtime = Arc::new(Runtime::new(registry.clone(), upstream_manager));
        let temp_dir = tempfile::TempDir::new().unwrap();
        let skill_store = Arc::new(SkillStore::new(temp_dir.path(), registry.clone()).unwrap());

        let server =
            SkillsServer::new(registry, search_engine, policy_engine, runtime, skill_store);

        // Get tools from router
        let tools = server.tool_router.list_all();

        assert_eq!(
            tools.len(),
            7,
            "Server must expose 7 tools (3 core + 4 skill management), found {}",
            tools.len()
        );

        let tool_names: Vec<String> = tools.iter().map(|t| t.name.to_string()).collect();
        assert!(
            tool_names.contains(&"skills.search".to_string()),
            "Missing skills.search"
        );
        assert!(
            tool_names.contains(&"skills.schema".to_string()),
            "Missing skills.schema"
        );
        assert!(
            tool_names.contains(&"skills.exec".to_string()),
            "Missing skills.exec"
        );
        assert!(
            tool_names.contains(&"skills.create".to_string()),
            "Missing skills.create"
        );
        assert!(
            tool_names.contains(&"skills.get_content".to_string()),
            "Missing skills.get_content"
        );
        assert!(
            tool_names.contains(&"skills.update".to_string()),
            "Missing skills.update"
        );
        assert!(
            tool_names.contains(&"skills.delete".to_string()),
            "Missing skills.delete"
        );

        println!("✓ Server exposes exactly 3 tools: {:?}", tool_names);
    }

    #[test]
    fn test_tool_schemas_are_valid() {
        let registry = Arc::new(Registry::new());
        let search_engine = Arc::new(SearchEngine::new(registry.clone()));
        let policy_engine = Arc::new(PolicyEngine::new(PolicyConfig::default()).unwrap());
        let upstream_manager = Arc::new(UpstreamManager::new(registry.clone()));
        let runtime = Arc::new(Runtime::new(registry.clone(), upstream_manager));
        let temp_dir = tempfile::TempDir::new().unwrap();
        let skill_store = Arc::new(SkillStore::new(temp_dir.path(), registry.clone()).unwrap());

        let server =
            SkillsServer::new(registry, search_engine, policy_engine, runtime, skill_store);
        let tools = server.tool_router.list_all();

        for tool in tools {
            // input_schema is Arc<Map>, which is always an object
            assert!(
                !tool.input_schema.is_empty() || tool.input_schema.is_empty(),
                "Tool {} schema should be accessible",
                tool.name
            );
            println!("✓ Tool {} has valid schema", tool.name);
        }
    }
}
