//! MCP Server Façade using Official rmcp SDK
//!
//! Exposes exactly four MCP tools to the host LLM:
//! - search: Discovery over unified registry
//! - schema: On-demand schema fetching
//! - exec: Validated execution with policy enforcement
//! - manage: Skill lifecycle management (create, read, update, delete)
//!
//! This implementation uses the official Rust MCP SDK (rmcp) to ensure
//! full protocol compliance and leverage battle-tested infrastructure.
//!
//! Design rationale for 4 tools:
//! - 3 core tools (search/schema/exec) for the 99% use case of discovering and executing callables
//! - 1 management tool (manage) for skill lifecycle, keeping the context minimal while enabling full CRUD
//! - This balance achieves "Infinite Skills. Finite Context." - agents can manage skills without tool bloat

use crate::core::policy::{ConsentLevel, PolicyEngine};
use crate::core::registry::Registry;
use crate::core::{CallableId, ToolResult};
use crate::execution::{ExecContext, Runtime};
use crate::storage::search::{SearchEngine, SearchFilters, SearchQuery};
use crate::storage::{CreateSkillRequest, SkillStore};
use rmcp::{
    handler::server::router::tool::ToolRouter,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, Content, Implementation, ServerCapabilities, ServerInfo},
    service::RequestContext,
    tool, tool_router, Json, RoleServer, ServerHandler,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
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

/// Input schema for search
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
    #[schemars(schema_with = "non_negative_int_schema")]
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

/// Custom schema for JsonValue fields - accepts any JSON value
fn json_value_schema(_gen: &mut schemars::SchemaGenerator) -> schemars::Schema {
    serde_json::from_value(serde_json::json!({})).unwrap()
}

/// Custom schema for Vec<JsonValue> fields - array of any JSON values
fn json_value_vec_schema(_gen: &mut schemars::SchemaGenerator) -> schemars::Schema {
    serde_json::from_value(serde_json::json!({
        "type": "array",
        "items": {}
    }))
    .unwrap()
}

fn non_negative_int_schema(_gen: &mut schemars::SchemaGenerator) -> schemars::Schema {
    serde_json::from_value(serde_json::json!({
        "type": "integer",
        "minimum": 0
    }))
    .unwrap()
}

/// Schema for bundled_files: array of [filename, content] tuples (or null).
fn bundled_files_schema(_gen: &mut schemars::SchemaGenerator) -> schemars::Schema {
    serde_json::from_value(serde_json::json!({
        "anyOf": [
            {
                "type": "array",
                "items": {
                    "type": "array",
                    "items": { "type": "string" },
                    "minItems": 2,
                    "maxItems": 2
                }
            },
            { "type": "null" }
        ]
    }))
    .unwrap()
}

/// Output schema for search
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SearchOutput {
    #[schemars(schema_with = "json_value_vec_schema")]
    pub matches: Vec<JsonValue>,
    pub next_cursor: Option<String>,
    pub stats: SearchStats,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SearchStats {
    #[schemars(schema_with = "non_negative_int_schema")]
    pub total_callables: usize,
    #[schemars(schema_with = "non_negative_int_schema")]
    pub total_tools: usize,
    #[schemars(schema_with = "non_negative_int_schema")]
    pub total_skills: usize,
    #[schemars(schema_with = "non_negative_int_schema")]
    pub searched_servers: usize,
    pub stale_servers: Vec<String>,
}

/// Input schema for schema
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
    #[schemars(schema_with = "non_negative_int_schema")]
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

/// Output schema for schema
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SchemaOutput {
    pub callable: CallableInfo,
    pub schema_digest: String,
    #[schemars(schema_with = "json_value_schema")]
    pub input_schema: Option<JsonValue>,
    #[schemars(schema_with = "json_value_schema")]
    pub output_schema: Option<JsonValue>,
    #[schemars(schema_with = "json_value_schema")]
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

/// Input schema for exec
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExecInput {
    /// Callable ID from search results
    pub id: String,

    /// Arguments to pass to the callable
    #[schemars(schema_with = "json_value_schema")]
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

/// Output schema for exec
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExecOutput {
    #[schemars(schema_with = "json_value_schema")]
    pub result: JsonValue,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub route: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "json_value_schema")]
    pub timing: Option<JsonValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "json_value_vec_schema")]
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
                crate::core::ToolResultContent::Text { text } => {
                    contents.push(Content::text(text));
                }
                crate::core::ToolResultContent::Image { data, mime_type } => {
                    contents.push(Content::image(data, mime_type));
                }
                crate::core::ToolResultContent::Resource { resource } => {
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

/// Implement the tool router with exactly 4 tools
#[tool_router]
impl SkillsServer {
    /// Fast discovery over registry (tools + skills) with filters.
    /// Use this to find callables before execution.
    #[tool(
        name = "search",
        description = "Fast discovery over registry (tools + skills) with filters. Use this to find callables before execution."
    )]
    async fn search(
        &self,
        Parameters(input): Parameters<SearchInput>,
    ) -> Result<Json<SearchOutput>, String> {
        debug!("search called with query: {}", input.q);

        let query = SearchQuery {
            q: input.q,
            kind: input.kind,
            mode: input.mode,
            limit: input.limit.clamp(1, 50),
            filters: input.filters,
            cursor: input.cursor,
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

        info!("search returned {} matches", output.matches.len());

        Ok(Json(output))
    }

    /// Fetch full schema and signature for a callable.
    /// Use this after search to get detailed parameter info.
    #[tool(
        name = "schema",
        description = "Fetch full schema and signature for a callable. Use this after search to get detailed parameter info."
    )]
    async fn schema(
        &self,
        Parameters(input): Parameters<SchemaInput>,
    ) -> Result<Json<SchemaOutput>, String> {
        debug!("schema called for: {}", input.id);

        let callable_id: CallableId = input.id.into();
        let record = self
            .registry
            .get(&callable_id)
            .ok_or_else(|| format!("Callable not found: {}", callable_id))?;

        let format = input.format.as_str();
        let max_bytes = input.max_bytes;

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
            if let Some(pointer) = input.json_pointer {
                if let Some(subtree) = schema.pointer(&pointer) {
                    schema = subtree.clone();
                } else {
                    return Err(format!("Invalid JSON pointer: {}", pointer));
                }
            }

            output.input_schema = Some(schema);

            if input.include_output_schema {
                output.output_schema = record.output_schema.clone();
            }
        }

        if format == "signature" || format == "both" {
            let signature = crate::core::CallableSignature::from_schema(&record.input_schema);
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

        info!("schema returned schema for {}", record.fq_name);
        Ok(Json(output))
    }

    /// Execute a callable with validation and policy enforcement.
    /// Always search and get schema first.
    #[tool(
        name = "exec",
        description = "Execute a callable with validation and policy enforcement. Always search and get schema first."
    )]
    async fn exec(&self, Parameters(input): Parameters<ExecInput>) -> Result<String, String> {
        debug!("exec called for: {}", input.id);

        let callable_id: CallableId = input.id.into();
        let dry_run = input.dry_run;

        // Get callable record
        let record = self
            .registry
            .get(&callable_id)
            .ok_or_else(|| format!("Callable not found: {}", callable_id))?;

        // Parse consent level
        let consent_level = input
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
            .authorize(&record, &input.arguments, consent_level)
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
            arguments: input.arguments,
            timeout_ms: input.timeout_ms,
            trace_enabled: input
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

        info!("exec completed for {}", record.fq_name);

        // Convert ToolResult to string representation (content is already JSON from runtime)
        let text: String = result
            .content
            .iter()
            .filter_map(|c| match c {
                crate::core::ToolResultContent::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");

        Ok(text)
    }

    /// Manage skills lifecycle: create, get, update, delete
    ///
    /// This single tool consolidates all skill management operations to maintain
    /// the "Finite Context" principle while enabling full CRUD functionality.
    #[tool(
        name = "manage",
        description = "Manage skill lifecycle: create, get, update, delete skills. Operations: create (requires name, description, skill_md), get (requires skill_id), update (requires skill_id, name, description, skill_md), delete (requires skill_id)."
    )]
    async fn manage(
        &self,
        Parameters(input): Parameters<ManageInput>,
    ) -> Result<Json<ManageOutput>, String> {
        debug!("manage called with operation: {:?}", input.operation);

        match input.operation {
            ManageOperation::Create => {
                let name = input.name.ok_or("name is required for create operation")?;
                let description = input
                    .description
                    .ok_or("description is required for create operation")?;
                let skill_md = input
                    .skill_md
                    .ok_or("skill_md is required for create operation")?;

                let request = CreateSkillRequest {
                    name: name.clone(),
                    version: input.version.unwrap_or_else(|| "1.0.0".to_string()),
                    description,
                    skill_md_content: skill_md,
                    uses_tools: input.uses_tools.unwrap_or_default(),
                    bundled_files: input.bundled_files.unwrap_or_default(),
                    tags: input.tags.unwrap_or_default(),
                };

                let id: crate::core::CallableId = self
                    .skill_store
                    .create_skill(request)
                    .await
                    .map_err(|e| format!("Failed to create skill: {}", e))?;

                info!("Created skill: {} ({})", name, id.as_str());

                Ok(Json(ManageOutput {
                    operation: "create".to_string(),
                    skill_id: Some(id.as_str().to_string()),
                    name: Some(name),
                    message: "Skill created successfully".to_string(),
                    data: None,
                }))
            }

            ManageOperation::Get => {
                let skill_id = input
                    .skill_id
                    .ok_or("skill_id is required for get operation")?;

                // Parse skill_id - it might be a full CallableId like "skill:name@version" or just "name"
                let skill_name = if skill_id.starts_with("skill:") {
                    skill_id
                        .strip_prefix("skill:")
                        .and_then(|s| s.split('@').next())
                        .unwrap_or(&skill_id)
                        .to_string()
                } else {
                    skill_id.clone()
                };

                // If a specific file is requested, return just that file
                if let Some(filename) = input.filename {
                    let file_content = self
                        .skill_store
                        .load_skill_file(&skill_name, &filename)
                        .map_err(|e| format!("Failed to load file: {}", e))?;
                    return Ok(Json(ManageOutput {
                        operation: "get".to_string(),
                        skill_id: Some(skill_id),
                        name: Some(skill_name),
                        message: file_content,
                        data: None,
                    }));
                }

                let content = self
                    .skill_store
                    .load_skill_content(&skill_name)
                    .map_err(|e| format!("Failed to load skill content: {}", e))?;

                let mut response = format!("# Skill: {}\n\n", skill_id);
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

                info!("Returned content for skill: {}", skill_id);

                Ok(Json(ManageOutput {
                    operation: "get".to_string(),
                    skill_id: Some(skill_id.clone()),
                    name: Some(skill_name.clone()),
                    message: response,
                    data: Some(serde_json::json!({
                        "uses_tools": content.uses_tools,
                        "bundled_tools": content.bundled_tools.iter().map(|t| &t.name).collect::<Vec<_>>(),
                        "additional_files": content.additional_files,
                    })),
                }))
            }

            ManageOperation::Update => {
                let skill_id = input
                    .skill_id
                    .ok_or("skill_id is required for update operation")?;
                let name = input.name.ok_or("name is required for update operation")?;
                let description = input
                    .description
                    .ok_or("description is required for update operation")?;
                let skill_md = input
                    .skill_md
                    .ok_or("skill_md is required for update operation")?;

                // Parse skill_id
                let skill_name = if skill_id.starts_with("skill:") {
                    skill_id
                        .strip_prefix("skill:")
                        .and_then(|s| s.split('@').next())
                        .unwrap_or(&skill_id)
                        .to_string()
                } else {
                    skill_id.clone()
                };

                let request = CreateSkillRequest {
                    name: name.clone(),
                    version: input.version.unwrap_or_else(|| "1.0.0".to_string()),
                    description,
                    skill_md_content: skill_md,
                    uses_tools: input.uses_tools.unwrap_or_default(),
                    bundled_files: input.bundled_files.unwrap_or_default(),
                    tags: input.tags.unwrap_or_default(),
                };

                let id: crate::core::CallableId = self
                    .skill_store
                    .update_skill(&skill_name, request)
                    .await
                    .map_err(|e| format!("Failed to update skill: {}", e))?;

                info!("Updated skill: {} ({})", skill_id, id.as_str());

                Ok(Json(ManageOutput {
                    operation: "update".to_string(),
                    skill_id: Some(id.as_str().to_string()),
                    name: Some(name),
                    message: "Skill updated successfully".to_string(),
                    data: None,
                }))
            }

            ManageOperation::Delete => {
                let skill_id = input
                    .skill_id
                    .ok_or("skill_id is required for delete operation")?;

                // Parse skill_id
                let skill_name = if skill_id.starts_with("skill:") {
                    skill_id
                        .strip_prefix("skill:")
                        .and_then(|s| s.split('@').next())
                        .unwrap_or(&skill_id)
                        .to_string()
                } else {
                    skill_id.clone()
                };

                self.skill_store
                    .delete_skill(&skill_name)
                    .map_err(|e| format!("Failed to delete skill: {}", e))?;

                info!("Deleted skill: {}", skill_id);

                Ok(Json(ManageOutput {
                    operation: "delete".to_string(),
                    skill_id: Some(skill_id.clone()),
                    name: Some(skill_name),
                    message: format!("Skill {} deleted successfully", skill_id),
                    data: None,
                }))
            }
        }
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
    #[schemars(schema_with = "bundled_files_schema")]
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
    #[schemars(schema_with = "bundled_files_schema")]
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

/// Management operation type
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ManageOperation {
    /// Create a new skill
    Create,
    /// Get skill content
    Get,
    /// Update an existing skill
    Update,
    /// Delete a skill
    Delete,
}

/// Unified input for skill management operations
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ManageInput {
    /// Operation to perform: create, get, update, delete
    pub operation: ManageOperation,
    /// Skill ID (required for get, update, delete; optional for create)
    #[serde(default)]
    pub skill_id: Option<String>,
    /// Skill name (required for create; optional for update)
    #[serde(default)]
    pub name: Option<String>,
    /// Skill version (optional, defaults to 1.0.0)
    #[serde(default)]
    pub version: Option<String>,
    /// Skill description (required for create/update)
    #[serde(default)]
    pub description: Option<String>,
    /// SKILL.md content (required for create/update)
    #[serde(default)]
    pub skill_md: Option<String>,
    /// MCP tools this skill uses (optional)
    #[serde(default)]
    pub uses_tools: Option<Vec<String>>,
    /// Bundled files as (filename, content) pairs (optional)
    #[schemars(schema_with = "bundled_files_schema")]
    #[serde(default)]
    pub bundled_files: Option<Vec<(String, String)>>,
    /// Tags for categorization (optional)
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    /// Specific filename to load when getting content (optional)
    #[serde(default)]
    pub filename: Option<String>,
}

/// Output from management operations
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ManageOutput {
    /// Operation performed
    pub operation: String,
    /// Skill ID (if applicable)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill_id: Option<String>,
    /// Skill name (if applicable)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Response message or content
    pub message: String,
    /// Additional data (e.g., skill content for get operation)
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "json_value_schema")]
    pub data: Option<JsonValue>,
}

impl ServerHandler for SkillsServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: Default::default(),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation {
                name: "skillsrs".to_string(),
                title: Some("Infinite Skills. Finite Context.".to_string()),
                version: env!("CARGO_PKG_VERSION").to_string(),
                website_url: Some("https://github.com/labiium/skills".to_string()),
                icons: None,
            },
            instructions: Some(
                "Unified MCP server that aggregates upstream MCP tools and Skills into a unified registry. \
                Exposes exactly 4 tools: search (discovery), schema (parameters), exec (execution), \
                and manage (skill lifecycle). Usage: (1) search to find callables, \
                (2) schema to get parameters, (3) exec to execute, (4) manage to create/update/delete skills."
                    .to_string(),
            ),
        }
    }

    async fn initialize(
        &self,
        _request: rmcp::model::InitializeRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<rmcp::model::InitializeResult, rmcp::model::ErrorData> {
        Ok(self.get_info())
    }

    async fn list_tools(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<rmcp::model::ListToolsResult, rmcp::model::ErrorData> {
        let tools = self.tool_router.list_all();
        Ok(rmcp::model::ListToolsResult {
            tools,
            next_cursor: None,
            meta: None,
        })
    }

    async fn call_tool(
        &self,
        request: rmcp::model::CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<rmcp::model::CallToolResult, rmcp::model::ErrorData> {
        use rmcp::handler::server::tool::ToolCallContext;

        let tool_context = ToolCallContext::new(self, request, context);
        self.tool_router.call(tool_context).await
    }
}
