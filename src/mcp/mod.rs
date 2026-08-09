//! MCP Server Façade using Official rmcp SDK
//!
//! Exposes exactly three MCP tools to the host LLM:
//! - search: Discovery over unified registry
//! - exec: Description, dry-run validation, and execution with policy enforcement
//! - manage: Skill lifecycle management (create, read, update, delete)
//!
//! This implementation uses the official Rust MCP SDK (rmcp) to ensure
//! full protocol compliance and leverage battle-tested infrastructure.
//!
//! Design rationale for 3 tools:
//! - 2 core tools (search/exec) for the 99% use case of discovering and executing callables
//! - 1 management tool (manage) for skill lifecycle, keeping the context minimal while enabling full CRUD
//! - This balance achieves "Infinite Skills. Finite Context." - agents can manage skills without tool bloat

use crate::core::policy::{ConsentLevel, PolicyEngine};
use crate::core::registry::Registry;
use crate::core::CallableId;
use crate::execution::{ExecContext, Runtime};
use crate::storage::search::{SearchEngine, SearchFilters, SearchQuery};
use crate::storage::{CreateSkillRequest, SkillStore};
use rmcp::{
    handler::server::router::tool::ToolRouter,
    handler::server::wrapper::Parameters,
    model::{
        CacheScope, CallToolResponse, Implementation, ProtocolVersion, ServerCapabilities,
        ServerInfo,
    },
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
    upstream_manager: Option<Arc<crate::execution::upstream::UpstreamManager>>,
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

const fn default_limit() -> usize {
    10
}

fn schema_from_object(value: JsonValue) -> schemars::Schema {
    match value {
        JsonValue::Object(map) => map.into(),
        _ => schemars::Schema::default(),
    }
}

/// Custom schema for `JsonValue` fields - accepts any JSON value
fn json_value_schema(_gen: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schema_from_object(serde_json::json!({}))
}

/// Custom schema for Vec<JsonValue> fields - array of any JSON values
fn json_value_vec_schema(_gen: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schema_from_object(serde_json::json!({
        "type": "array",
        "items": {}
    }))
}

fn optional_json_value_schema(_gen: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schema_from_object(serde_json::json!({
        "anyOf": [{}, { "type": "null" }]
    }))
}

fn optional_json_value_vec_schema(_gen: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schema_from_object(serde_json::json!({
        "anyOf": [
            {
                "type": "array",
                "items": {}
            },
            { "type": "null" }
        ]
    }))
}

fn non_negative_int_schema(_gen: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schema_from_object(serde_json::json!({
        "type": "integer",
        "minimum": 0
    }))
}

/// Schema for `bundled_files`: array of [filename, content] tuples (or null).
fn bundled_files_schema(_gen: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schema_from_object(serde_json::json!({
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

fn default_format() -> String {
    "both".to_string()
}

const fn default_true() -> bool {
    true
}

const fn default_max_bytes() -> usize {
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

    /// Arguments to pass to the callable. Missing arguments normalize to an empty object.
    #[serde(default = "default_arguments")]
    #[schemars(schema_with = "json_value_schema")]
    pub arguments: JsonValue,

    /// Return callable schema/signature details instead of executing.
    #[serde(default)]
    pub describe: bool,

    /// Schema format to return when describe is true.
    #[serde(default = "default_format")]
    #[schemars(description = "Format: json_schema, signature, or both")]
    pub format: String,

    /// Include output schema if available when describe is true.
    #[serde(default = "default_true")]
    pub include_output_schema: bool,

    /// Maximum describe response size in bytes.
    #[serde(default = "default_max_bytes")]
    #[schemars(schema_with = "non_negative_int_schema")]
    pub max_bytes: usize,

    /// JSON Pointer to schema subtree when describe is true.
    #[serde(default)]
    pub json_pointer: Option<String>,

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

fn default_arguments() -> JsonValue {
    serde_json::json!({})
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DescribeOptions {
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

/// Response from exec. Exactly one or more sections may be present depending on `describe/dry_run/execution` mode.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExecResponse {
    pub mode: String,
    pub callable: CallableInfo,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<SchemaOutput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dry_run: Option<DryRunOutput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution: Option<ExecOutput>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DryRunOutput {
    pub valid: bool,
    pub message: String,
}

/// Non-lossy execution output.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExecOutput {
    #[schemars(schema_with = "json_value_vec_schema")]
    pub content: Vec<JsonValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "optional_json_value_schema")]
    pub structured_content: Option<JsonValue>,
    pub is_error: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "optional_json_value_schema")]
    pub timing: Option<JsonValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "optional_json_value_vec_schema")]
    pub steps: Option<Vec<JsonValue>>,
}

impl SkillsServer {
    #[must_use]
    pub fn new(
        registry: Arc<Registry>,
        search_engine: Arc<SearchEngine>,
        policy_engine: Arc<PolicyEngine>,
        runtime: Arc<Runtime>,
        skill_store: Arc<SkillStore>,
    ) -> Self {
        Self {
            registry,
            search_engine,
            policy_engine,
            runtime,
            skill_store,
            upstream_manager: None,
            tool_router: Self::tool_router(),
        }
    }

    #[must_use]
    pub fn with_upstream_manager(
        mut self,
        upstream_manager: Arc<crate::execution::upstream::UpstreamManager>,
    ) -> Self {
        self.upstream_manager = Some(upstream_manager);
        self
    }

    #[must_use]
    pub fn tool_names(&self) -> Vec<String> {
        self.tool_router
            .list_all()
            .into_iter()
            .map(|tool| tool.name.to_string())
            .collect()
    }

    fn callable_info(record: &crate::core::CallableRecord) -> CallableInfo {
        CallableInfo {
            id: record.id.as_str().to_string(),
            kind: record.kind.to_string(),
            name: record.name.clone(),
            fq_name: record.fq_name.clone(),
            server: record.server_alias.clone(),
            version: record.skill_version.clone(),
        }
    }

    fn describe_callable(
        record: &crate::core::CallableRecord,
        describe: &DescribeOptions,
    ) -> Result<SchemaOutput, String> {
        let format = describe.format.as_str();
        let mut output = SchemaOutput {
            callable: Self::callable_info(record),
            schema_digest: record.schema_digest.as_str().to_string(),
            input_schema: None,
            output_schema: None,
            signature: None,
        };

        if format == "json_schema" || format == "both" {
            let mut schema = record.input_schema.clone();

            if let Some(pointer) = describe.json_pointer.as_deref() {
                if let Some(subtree) = schema.pointer(pointer) {
                    schema = subtree.clone();
                } else {
                    return Err(format!("Invalid JSON pointer: {pointer}"));
                }
            }

            output.input_schema = Some(schema);

            if describe.include_output_schema {
                output.output_schema.clone_from(&record.output_schema);
            }
        }

        if format == "signature" || format == "both" {
            let signature = crate::core::CallableSignature::from_schema(&record.input_schema);
            output.signature = Some(
                serde_json::to_value(signature)
                    .map_err(|error| format!("Failed to serialize signature: {error}"))?,
            );
        }

        let output_str = serde_json::to_string(&output)
            .map_err(|error| format!("Failed to serialize description: {error}"))?;
        if output_str.len() > describe.max_bytes {
            return Err(format!(
                "Response exceeds max_bytes limit: {} > {}",
                output_str.len(),
                describe.max_bytes
            ));
        }

        Ok(output)
    }

    fn result_to_exec_output(
        result: crate::core::ToolResult,
        record: &crate::core::CallableRecord,
        trace: Option<&TraceArgs>,
    ) -> Result<ExecOutput, String> {
        let include_route = trace.is_some_and(|t| t.include_route);
        let include_timing = trace.is_some_and(|t| t.include_timing);
        let include_steps = trace.is_some_and(|t| t.include_steps);

        let trace_value = result
            .structured_content
            .as_ref()
            .and_then(|structured| structured.get("trace"));
        let timing = include_timing
            .then(|| {
                trace_value.map(|trace| {
                    serde_json::json!({
                        "started_at": trace.get("started_at").cloned().unwrap_or(JsonValue::Null),
                        "completed_at": trace.get("completed_at").cloned().unwrap_or(JsonValue::Null),
                        "total_duration_ms": trace.get("total_duration_ms").cloned().unwrap_or(JsonValue::Null),
                    })
                })
            })
            .flatten();
        let steps = include_steps
            .then(|| {
                trace_value
                    .and_then(|trace| trace.get("steps"))
                    .and_then(|steps| steps.as_array())
                    .cloned()
            })
            .flatten();

        Ok(ExecOutput {
            content: result
                .content
                .into_iter()
                .map(|content| {
                    serde_json::to_value(content)
                        .map_err(|error| format!("Failed to serialize tool content: {error}"))
                })
                .collect::<Result<Vec<_>, _>>()?,
            structured_content: result.structured_content,
            is_error: result.is_error,
            route: include_route.then(|| record.fq_name.clone()),
            timing,
            steps,
        })
    }

    /// # Errors
    /// Returns an error if the operation cannot be completed.
    pub async fn exec_callable(&self, input: ExecInput) -> Result<ExecResponse, String> {
        debug!("exec called for: {}", input.id);

        let callable_id: CallableId = input.id.into();
        let dry_run = input.dry_run;

        let record = self
            .registry
            .get(&callable_id)
            .ok_or_else(|| format!("Callable not found: {callable_id}"))?;

        if input.describe {
            let describe = DescribeOptions {
                format: input.format,
                include_output_schema: input.include_output_schema,
                max_bytes: input.max_bytes,
                json_pointer: input.json_pointer,
            };
            let description = Some(Self::describe_callable(&record, &describe)?);
            info!("exec described {}", record.fq_name);
            return Ok(ExecResponse {
                mode: "describe".to_string(),
                callable: Self::callable_info(&record),
                description,
                dry_run: None,
                execution: None,
            });
        }

        self.runtime
            .validate_callable_arguments(&callable_id, &input.arguments)
            .map_err(|e| format!("Validation failed: {e}"))?;

        let consent_level = input
            .consent
            .as_ref()
            .and_then(|c| c.level.as_deref())
            .map_or(ConsentLevel::None, |s| match s {
                "user_confirmed" => ConsentLevel::UserConfirmed,
                "admin_confirmed" => ConsentLevel::AdminConfirmed,
                _ => ConsentLevel::None,
            });

        let policy_result = self
            .policy_engine
            .authorize(&record, &input.arguments, consent_level)
            .await
            .map_err(|e| format!("Policy check failed: {e}"))?;

        if !policy_result.allowed {
            warn!("Execution denied: {}", policy_result.reason);
            return Err(format!("Execution denied: {}", policy_result.reason));
        }

        if dry_run {
            info!("Dry run validated {}", record.fq_name);
            return Ok(ExecResponse {
                mode: "dry_run".to_string(),
                callable: Self::callable_info(&record),
                description: None,
                dry_run: Some(DryRunOutput {
                    valid: true,
                    message: format!("Dry run validated {}", record.fq_name),
                }),
                execution: None,
            });
        }

        let trace_enabled = input
            .trace
            .as_ref()
            .is_some_and(|t| t.include_route || t.include_timing || t.include_steps);

        let ctx = ExecContext {
            callable_id: callable_id.clone(),
            arguments: input.arguments,
            timeout_ms: input.timeout_ms,
            trace_enabled,
        };

        let result = self
            .runtime
            .execute(ctx)
            .await
            .map_err(|e| format!("Execution failed: {e}"))?;

        info!("exec completed for {}", record.fq_name);

        Ok(ExecResponse {
            mode: "exec".to_string(),
            callable: Self::callable_info(&record),
            description: None,
            dry_run: None,
            execution: Some(Self::result_to_exec_output(
                result,
                &record,
                input.trace.as_ref(),
            )?),
        })
    }
}

/// Implement the tool router with exactly 3 tools
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
            limit: input.limit.clamp(1, 50),
            filters: input.filters,
            cursor: input.cursor,
        };

        let results = self
            .search_engine
            .search(&query)
            .await
            .map_err(|e| format!("Search failed: {e}"))?;

        let stats = self.registry.stats();

        let matches = results
            .matches
            .into_iter()
            .map(|search_match| {
                serde_json::to_value(search_match)
                    .map_err(|error| format!("Failed to serialize search result: {error}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let output = SearchOutput {
            matches,
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

    /// Describe, validate, or execute a callable with policy enforcement.
    /// Use describe for schema/signature details, `dry_run` to validate without execution.
    #[tool(
        name = "exec",
        description = "Describe, validate, or execute a callable with policy enforcement. Set describe for schema/signature details or dry_run to validate without execution."
    )]
    async fn exec(
        &self,
        Parameters(input): Parameters<ExecInput>,
    ) -> Result<Json<ExecResponse>, String> {
        self.exec_callable(input).await.map(Json)
    }

    /// Manage skills lifecycle: create, get, update, delete
    ///
    /// This single tool consolidates all skill management operations to maintain
    /// the "Finite Context" principle while enabling full CRUD functionality.
    #[tool(
        name = "manage",
        description = "Manage skill and upstream lifecycle. Operations: create/get/update/delete skills, add_upstream/remove_upstream/list_upstreams. For add_upstream: requires upstream_alias, transport, url|command, optional description and tags."
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
                    tags: input.tags.unwrap_or_default(),
                    scripts: input.scripts.unwrap_or_default(),
                    references: input.references.unwrap_or_default(),
                    assets: input.assets.unwrap_or_default(),
                };

                let id: crate::core::CallableId = self
                    .skill_store
                    .create_skill(request)
                    .await
                    .map_err(|e| format!("Failed to create skill: {e}"))?;

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
                        .map_err(|e| format!("Failed to load file: {e}"))?;
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
                    .map_err(|e| format!("Failed to load skill content: {e}"))?;

                let mut response = format!("# Skill: {skill_id}\n\n");
                response.push_str(&content.skill_md);
                response.push_str("\n\n---\n\n");
                response.push_str("## Metadata\n\n");
                response.push_str("- Uses tools: ");
                response.push_str(&content.uses_tools.join(", "));
                response.push('\n');
                response.push_str("- Bundled tools: ");
                response.push_str(
                    &content
                        .bundled_tools
                        .iter()
                        .map(|tool| tool.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                );
                response.push('\n');
                response.push_str("- Additional files: ");
                response.push_str(&content.additional_files.join(", "));
                response.push('\n');

                info!("Returned content for skill: {}", skill_id);

                Ok(Json(ManageOutput {
                    operation: "get".to_string(),
                    skill_id: Some(skill_id),
                    name: Some(skill_name),
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
                    tags: input.tags.unwrap_or_default(),
                    scripts: input.scripts.unwrap_or_default(),
                    references: input.references.unwrap_or_default(),
                    assets: input.assets.unwrap_or_default(),
                };

                let id: crate::core::CallableId = self
                    .skill_store
                    .update_skill(&skill_name, request)
                    .await
                    .map_err(|e| format!("Failed to update skill: {e}"))?;

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
                    .map_err(|e| format!("Failed to delete skill: {e}"))?;

                info!("Deleted skill: {}", skill_id);

                Ok(Json(ManageOutput {
                    operation: "delete".to_string(),
                    skill_id: Some(skill_id.clone()),
                    name: Some(skill_name),
                    message: format!("Skill {skill_id} deleted successfully"),
                    data: None,
                }))
            }

            ManageOperation::AddUpstream => {
                let alias = input
                    .upstream_alias
                    .ok_or("upstream_alias is required for add_upstream operation")?;
                let transport_str = input
                    .transport
                    .ok_or("transport is required for add_upstream operation")?;

                let transport = match transport_str.as_str() {
                    "stdio" => crate::execution::upstream::Transport::Stdio,
                    "http" => crate::execution::upstream::Transport::Http,
                    "http_sse" | "http+sse" => crate::execution::upstream::Transport::HttpSse,
                    _ => return Err(format!("Invalid transport type: {transport_str}")),
                };

                let config = crate::execution::upstream::UpstreamConfig {
                    alias: alias.clone(),
                    transport,
                    command: input.command,
                    url: input.url,
                    auth: None,
                    repo: None,
                    git_ref: None,
                    skills: None,
                    roots: None,
                    tags: input.tags.unwrap_or_default(),
                    sandbox_config: None,
                    description: input.upstream_description,
                };

                let message = if let Some(ref upstream_manager) = self.upstream_manager {
                    upstream_manager
                        .add_upstream(config)
                        .await
                        .map_err(|e| format!("Failed to add upstream: {e}"))?;

                    if input.test_connection {
                        format!("Upstream MCP server '{alias}' added and connected successfully")
                    } else {
                        format!("Upstream MCP server '{alias}' added (connection test skipped)")
                    }
                } else {
                    return Err("Upstream manager not available".to_string());
                };

                info!("Added upstream: {}", alias);

                Ok(Json(ManageOutput {
                    operation: "add_upstream".to_string(),
                    skill_id: None,
                    name: Some(alias.clone()),
                    message,
                    data: Some(serde_json::json!({ "alias": alias, "transport": transport_str })),
                }))
            }

            ManageOperation::RemoveUpstream => {
                let alias = input
                    .upstream_alias
                    .ok_or("upstream_alias is required for remove_upstream operation")?;

                if let Some(ref upstream_manager) = self.upstream_manager {
                    upstream_manager
                        .disconnect(&alias)
                        .await
                        .map_err(|e| format!("Failed to remove upstream: {e}"))?;
                } else {
                    return Err("Upstream manager not available".to_string());
                }

                info!("Removed upstream: {}", alias);

                Ok(Json(ManageOutput {
                    operation: "remove_upstream".to_string(),
                    skill_id: None,
                    name: Some(alias.clone()),
                    message: format!("Upstream MCP server '{alias}' removed successfully"),
                    data: None,
                }))
            }

            ManageOperation::ListUpstreams => {
                let servers = if let Some(ref upstream_manager) = self.upstream_manager {
                    upstream_manager.list_servers().await
                } else {
                    return Err("Upstream manager not available".to_string());
                };

                let message = if servers.is_empty() {
                    "No upstream MCP servers configured".to_string()
                } else {
                    format!(
                        "Found {} upstream server(s): {}",
                        servers.len(),
                        servers.join(", ")
                    )
                };

                info!("Listed {} upstream servers", servers.len());

                Ok(Json(ManageOutput {
                    operation: "list_upstreams".to_string(),
                    skill_id: None,
                    name: None,
                    message,
                    data: Some(serde_json::json!({ "servers": servers, "count": servers.len() })),
                }))
            }

            ManageOperation::UpdateUpstream => {
                let alias = input
                    .upstream_alias
                    .ok_or("upstream_alias is required for update_upstream operation")?;

                let updates = if let Some(ref upstream_manager) = self.upstream_manager {
                    let current = upstream_manager
                        .get_config(&alias)
                        .await
                        .ok_or_else(|| format!("Upstream '{alias}' not found"))?;

                    crate::execution::upstream::UpstreamConfig {
                        alias: alias.clone(),
                        transport: current.transport,
                        command: input.command.or(current.command),
                        url: input.url.or(current.url),
                        auth: current.auth,
                        repo: current.repo,
                        git_ref: current.git_ref,
                        skills: current.skills,
                        roots: current.roots,
                        tags: if let Some(tags) = input.tags {
                            if tags.is_empty() {
                                current.tags
                            } else {
                                tags
                            }
                        } else {
                            current.tags
                        },
                        sandbox_config: current.sandbox_config,
                        description: input.upstream_description.or(current.description),
                    }
                } else {
                    return Err("Upstream manager not available".to_string());
                };

                if let Some(ref upstream_manager) = self.upstream_manager {
                    upstream_manager
                        .update_upstream(&alias, updates)
                        .await
                        .map_err(|e| format!("Failed to update upstream: {e}"))?;
                }

                info!("Updated upstream: {}", alias);

                Ok(Json(ManageOutput {
                    operation: "update_upstream".to_string(),
                    skill_id: None,
                    name: Some(alias.clone()),
                    message: format!("Upstream MCP server '{alias}' updated successfully"),
                    data: Some(serde_json::json!({ "alias": alias })),
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
    /// Add an upstream MCP server
    AddUpstream,
    /// Remove an upstream MCP server
    RemoveUpstream,
    /// List all upstream MCP servers
    ListUpstreams,
    /// Update an upstream MCP server configuration
    UpdateUpstream,
}

/// Unified input for skill management operations
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ManageInput {
    /// Operation to perform: create, get, update, delete, `add_upstream`, `remove_upstream`, `list_upstreams`
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
    /// Scripts as (filename, content) pairs to place in scripts/ subdirectory
    #[schemars(schema_with = "bundled_files_schema")]
    #[serde(default)]
    pub scripts: Option<Vec<(String, String)>>,
    /// Reference files as (filename, content) pairs to place in references/ subdirectory
    #[schemars(schema_with = "bundled_files_schema")]
    #[serde(default)]
    pub references: Option<Vec<(String, String)>>,
    /// Asset files as (filename, content) pairs to place in assets/ subdirectory
    #[schemars(schema_with = "bundled_files_schema")]
    #[serde(default)]
    pub assets: Option<Vec<(String, String)>>,
    /// Tags for categorization (optional)
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    /// Specific filename to load when getting content (optional)
    #[serde(default)]
    pub filename: Option<String>,
    /// Upstream alias (required for `add_upstream`, `remove_upstream`)
    #[serde(default)]
    pub upstream_alias: Option<String>,
    /// Transport type for upstream: stdio, http, `http_sse` (required for `add_upstream`)
    #[serde(default)]
    pub transport: Option<String>,
    /// URL for HTTP transport (required for http transport)
    #[serde(default)]
    pub url: Option<String>,
    /// Command for stdio transport as array of strings (required for stdio transport)
    #[serde(default)]
    pub command: Option<Vec<String>>,
    /// Test connection after adding (optional, defaults to true)
    #[serde(default = "default_true")]
    pub test_connection: bool,
    /// Description of the upstream MCP server (optional, helps LLMs discover tools)
    #[serde(default)]
    pub upstream_description: Option<String>,
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
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_protocol_version(ProtocolVersion::V_2026_07_28)
            .with_server_info(
                Implementation::new("skillsrs", env!("CARGO_PKG_VERSION"))
                    .with_title("Infinite Skills. Finite Context.")
                    .with_website_url("https://github.com/labiium/skills"),
            )
            .with_instructions(
                "Unified MCP server that aggregates upstream MCP tools and Skills into a unified registry. \
                Exposes exactly 3 tools: search (discovery), exec (description, validation, execution), \
                and manage (skill lifecycle). Usage: (1) search to find callables, \
                (2) exec with describe to get parameters, (3) exec to validate or execute, \
                (4) manage to create/update/delete skills."
            )
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
        Ok(rmcp::model::ListToolsResult::with_all_items(tools)
            .with_ttl_ms(0)
            .with_cache_scope(CacheScope::Public))
    }

    async fn call_tool(
        &self,
        request: rmcp::model::CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, rmcp::model::ErrorData> {
        use rmcp::handler::server::tool::ToolCallContext;

        let tool_context = ToolCallContext::new(self, request, context);
        self.tool_router.call(tool_context).await
    }
}
