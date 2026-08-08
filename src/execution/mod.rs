//! Runtime executor
//!
//! Executes callables (tools and skills) with:
//! - Tool execution via proxy to upstream servers
//! - Skill workflow orchestration
//! - Validation and tracing
//! - Timeout enforcement

pub mod sandbox;
pub mod upstream;
pub mod wasm;

use crate::core::registry::Registry;
use crate::core::{BundledTool, CallableId, CallableKind, ToolResult, ToolResultContent};
use crate::execution::sandbox::SandboxConfigOverride;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tracing::{debug, error, info, warn};
use upstream::UpstreamManager;
use uuid::Uuid;

#[derive(Error, Debug)]
pub enum RuntimeError {
    #[error("Callable not found: {0}")]
    CallableNotFound(String),

    #[error("Execution failed: {0}")]
    ExecutionFailed(String),

    #[error("Validation failed: {0}")]
    ValidationFailed(String),

    #[error("Timeout exceeded: {0}ms")]
    Timeout(u64),

    #[error("Upstream error: {0}")]
    UpstreamError(String),

    #[error("Internal error: {0}")]
    Internal(String),

    #[error("Bundled tool execution failed: {0}")]
    BundledToolError(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, RuntimeError>;

/// Execution context
#[derive(Debug, Clone)]
pub struct ExecContext {
    pub callable_id: CallableId,
    pub arguments: serde_json::Value,
    pub timeout_ms: Option<u64>,
    pub trace_enabled: bool,
}

/// Execution trace step
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceStep {
    pub step_index: usize,
    pub callable_id: String,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub duration_ms: Option<u64>,
    pub success: bool,
    pub error: Option<String>,
}

/// Execution trace
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionTrace {
    pub execution_id: String,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub total_duration_ms: Option<u64>,
    pub steps: Vec<TraceStep>,
}

impl ExecutionTrace {
    fn new() -> Self {
        Self {
            execution_id: Uuid::new_v4().to_string(),
            started_at: chrono::Utc::now(),
            completed_at: None,
            total_duration_ms: None,
            steps: Vec::new(),
        }
    }

    fn complete(&mut self) {
        self.completed_at = Some(chrono::Utc::now());
        if let Some(completed) = self.completed_at {
            self.total_duration_ms = Some(duration_ms(self.started_at, completed));
        }
    }
}

fn duration_ms(
    started_at: chrono::DateTime<chrono::Utc>,
    completed_at: chrono::DateTime<chrono::Utc>,
) -> u64 {
    u64::try_from(
        completed_at
            .signed_duration_since(started_at)
            .num_milliseconds(),
    )
    .unwrap_or(0)
}

/// Runtime executor
pub struct Runtime {
    registry: Arc<Registry>,
    upstream_manager: Arc<UpstreamManager>,
    sandbox_config: sandbox::SandboxConfig,
}

impl Runtime {
    #[must_use]
    pub fn new(registry: Arc<Registry>, upstream_manager: Arc<UpstreamManager>) -> Self {
        Self {
            registry,
            upstream_manager,
            sandbox_config: sandbox::SandboxConfig::default(),
        }
    }

    /// Create runtime with custom sandbox configuration
    #[must_use]
    pub const fn with_sandbox_config(
        registry: Arc<Registry>,
        upstream_manager: Arc<UpstreamManager>,
        sandbox_config: sandbox::SandboxConfig,
    ) -> Self {
        Self {
            registry,
            upstream_manager,
            sandbox_config,
        }
    }

    /// Execute a callable
    /// # Errors
    /// Returns an error if the operation cannot be completed.
    pub async fn execute(&self, ctx: ExecContext) -> Result<ToolResult> {
        let mut trace = if ctx.trace_enabled {
            Some(ExecutionTrace::new())
        } else {
            None
        };

        info!("Executing callable: {}", ctx.callable_id.as_str());

        let record = self.get_callable_record(&ctx.callable_id)?;

        // Validate arguments against schema
        self.validate_callable_arguments(&ctx.callable_id, &ctx.arguments)?;

        // Apply timeout
        // Execute based on callable kind
        let result = match record.kind {
            CallableKind::Tool => {
                if let Some(timeout_ms) = ctx.timeout_ms {
                    tokio::time::timeout(
                        Duration::from_millis(timeout_ms),
                        self.execute_tool(&ctx, &record),
                    )
                    .await
                    .map_err(|_| RuntimeError::Timeout(timeout_ms))?
                } else {
                    self.execute_tool(&ctx, &record).await
                }
            }
            CallableKind::Skill => {
                if let Some(timeout_ms) = ctx.timeout_ms {
                    tokio::time::timeout(
                        Duration::from_millis(timeout_ms),
                        self.execute_skill(&ctx, &record, trace.as_mut()),
                    )
                    .await
                    .map_err(|_| RuntimeError::Timeout(timeout_ms))?
                } else {
                    self.execute_skill(&ctx, &record, trace.as_mut()).await
                }
            }
        }?;

        // Complete trace
        if let Some(ref mut trace) = trace {
            trace.complete();
        }

        // Add trace to result if enabled
        if let Some(trace) = trace {
            let mut result_with_trace = result;
            if let Some(ref mut structured) = result_with_trace.structured_content {
                if let Some(obj) = structured.as_object_mut() {
                    let trace = serde_json::to_value(trace)
                        .map_err(|error| RuntimeError::Internal(error.to_string()))?;
                    obj.insert("trace".to_string(), trace);
                }
            } else {
                result_with_trace.structured_content = Some(serde_json::json!({ "trace": trace }));
            }
            return Ok(result_with_trace);
        }

        Ok(result)
    }

    /// Validate arguments for a registered callable without executing it.
    /// # Errors
    /// Returns an error if the operation cannot be completed.
    pub fn validate_callable_arguments(
        &self,
        callable_id: &CallableId,
        arguments: &serde_json::Value,
    ) -> Result<()> {
        let record = self.get_callable_record(callable_id)?;
        Self::validate_arguments(&record.input_schema, arguments)
    }

    fn get_callable_record(&self, callable_id: &CallableId) -> Result<crate::core::CallableRecord> {
        self.registry
            .get(callable_id)
            .ok_or_else(|| RuntimeError::CallableNotFound(callable_id.as_str().to_string()))
    }

    /// Validate arguments against JSON schema
    fn validate_arguments(schema: &serde_json::Value, arguments: &serde_json::Value) -> Result<()> {
        // Basic validation: check required fields
        if let Some(required) = schema.get("required").and_then(|r| r.as_array()) {
            if let Some(args_obj) = arguments.as_object() {
                for req in required {
                    if let Some(req_str) = req.as_str() {
                        if !args_obj.contains_key(req_str) {
                            return Err(RuntimeError::ValidationFailed(format!(
                                "Missing required argument: {req_str}"
                            )));
                        }
                    }
                }
            } else {
                return Err(RuntimeError::ValidationFailed(
                    "Arguments must be an object".to_string(),
                ));
            }
        }

        // In a full implementation, use jsonschema crate for complete validation
        debug!("Arguments validated successfully");
        Ok(())
    }

    /// Execute a tool (proxy to upstream)
    async fn execute_tool(
        &self,
        ctx: &ExecContext,
        record: &crate::core::CallableRecord,
    ) -> Result<ToolResult> {
        debug!("Executing tool: {}", record.fq_name);

        let server = record
            .server_alias
            .as_ref()
            .ok_or_else(|| RuntimeError::Internal("Tool missing server alias".to_string()))?;

        let tool_name = record
            .upstream_tool_name
            .as_ref()
            .ok_or_else(|| RuntimeError::Internal("Tool missing upstream name".to_string()))?;

        info!(
            "Proxying to upstream: server={}, tool={}",
            server, tool_name
        );

        // Call upstream server
        let result = self
            .upstream_manager
            .call_tool(server, tool_name, ctx.arguments.clone())
            .await
            .map_err(|e| RuntimeError::UpstreamError(e.to_string()))?;

        // Convert upstream JSON response to ToolResult
        // MCP tools/call returns { content: [...], isError?: bool }
        let is_error = result
            .get("isError")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);

        let content = result
            .get("content")
            .and_then(serde_json::Value::as_array)
            .map_or_else(
                || {
                    vec![ToolResultContent::Text {
                        text: result.to_string(),
                    }]
                },
                |content_array| {
                    content_array
                        .iter()
                        .filter_map(|item| {
                            item.get("type")
                                .and_then(serde_json::Value::as_str)
                                .and_then(|item_type| match item_type {
                                    "text" => {
                                        let text =
                                            item.get("text").and_then(|v| v.as_str()).unwrap_or("");
                                        Some(ToolResultContent::Text {
                                            text: text.to_string(),
                                        })
                                    }
                                    "image" => {
                                        let data =
                                            item.get("data").and_then(|v| v.as_str()).unwrap_or("");
                                        let mime_type = item
                                            .get("mimeType")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("image/png")
                                            .to_string();
                                        Some(ToolResultContent::Image {
                                            data: data.to_string(),
                                            mime_type,
                                        })
                                    }
                                    "resource" => {
                                        let uri =
                                            item.get("uri").and_then(|v| v.as_str()).unwrap_or("");
                                        let mime_type: Option<&str> =
                                            item.get("mimeType").and_then(|v| v.as_str());
                                        let text: Option<&str> =
                                            item.get("text").and_then(|v| v.as_str());
                                        let blob: Option<&str> =
                                            item.get("blob").and_then(|v| v.as_str());

                                        Some(ToolResultContent::Resource {
                                            resource: crate::core::ResourceContent {
                                                uri: uri.to_string(),
                                                mime_type: mime_type.map(|s: &str| s.to_string()),
                                                text: text.map(|s: &str| s.to_string()),
                                                blob: blob.map(|s: &str| s.to_string()),
                                            },
                                        })
                                    }
                                    _ => None,
                                })
                        })
                        .collect()
                },
            );

        if content.is_empty() && !is_error {
            error!("Tool returned empty content, treating as error");
            return Ok(ToolResult {
                content: vec![ToolResultContent::Text {
                    text: "Tool returned no content".to_string(),
                }],
                structured_content: Some(result),
                is_error: true,
            });
        }

        Ok(ToolResult {
            content,
            structured_content: Some(result),
            is_error,
        })
    }

    /// Execute a skill (workflow orchestration)
    async fn execute_skill(
        &self,
        ctx: &ExecContext,
        record: &crate::core::CallableRecord,
        trace: Option<&mut ExecutionTrace>,
    ) -> Result<ToolResult> {
        debug!("Executing skill: {}", record.fq_name);

        let step_start = chrono::Utc::now();

        // Skills are instructions for agents (progressive disclosure)
        // This implementation handles skills with bundled tools
        // For skills that only reference MCP tools, the agent loads SKILL.md and calls them

        info!("Executing skill workflow: {}", record.fq_name);

        // Check if this skill has bundled tools
        if !record.bundled_tools.is_empty() {
            // For now, execute the first bundled tool as the entrypoint
            // In a full implementation, SKILL.md would specify which tool to run
            if let Some(bundled_tool) = record.bundled_tools.first() {
                info!("Executing bundled tool: {}", bundled_tool.name);

                // Extract tool-level sandbox config from skill record
                let tool_override = record.sandbox_config.as_ref();
                let result = self
                    .execute_bundled_tool(
                        bundled_tool,
                        &ctx.arguments,
                        ctx.timeout_ms,
                        tool_override,
                    )
                    .await?;

                // Record trace step
                if let Some(trace) = trace {
                    let step_end = chrono::Utc::now();
                    let duration = duration_ms(step_start, step_end);

                    trace.steps.push(TraceStep {
                        step_index: trace.steps.len(),
                        callable_id: ctx.callable_id.as_str().to_string(),
                        started_at: step_start,
                        completed_at: Some(step_end),
                        duration_ms: Some(duration),
                        success: !result.is_error,
                        error: None,
                    });
                }

                return Ok(result);
            }
        }

        // Return skill info for agent to load SKILL.md
        let result = ToolResult {
            content: vec![ToolResultContent::Text {
                text: format!(
                    "Skill {} provides instructions for agent. Load SKILL.md for details.",
                    record.fq_name
                ),
            }],
            structured_content: Some(serde_json::json!({
                "skill": record.name,
                "version": record.skill_version,
                "uses_tools": record.uses.iter().map(super::core::CallableId::as_str).collect::<Vec<_>>(),
                "bundled_tools": record.bundled_tools.iter().map(|t| &t.name).collect::<Vec<_>>(),
                "additional_files": &record.additional_files,
                "message": "Load SKILL.md to see instructions"
            })),
            is_error: false,
        };

        // Record trace step
        if let Some(trace) = trace {
            let step_end = chrono::Utc::now();
            let duration = duration_ms(step_start, step_end);

            trace.steps.push(TraceStep {
                step_index: trace.steps.len(),
                callable_id: ctx.callable_id.as_str().to_string(),
                started_at: step_start,
                completed_at: Some(step_end),
                duration_ms: Some(duration),
                success: true,
                error: None,
            });
        }

        Ok(result)
    }

    /// Execute a bundled tool (script)
    #[expect(
        clippy::too_many_lines,
        reason = "bundled tool execution keeps setup, cleanup, and result mapping together"
    )]
    async fn execute_bundled_tool(
        &self,
        tool: &BundledTool,
        arguments: &serde_json::Value,
        timeout_ms: Option<u64>,
        tool_sandbox_override: Option<&SandboxConfigOverride>,
    ) -> Result<ToolResult> {
        if tool.command.is_empty() {
            return Err(RuntimeError::BundledToolError("Empty command".to_string()));
        }

        let program = tool
            .command
            .first()
            .ok_or_else(|| RuntimeError::BundledToolError("Empty command".to_string()))?;
        let args = tool.command.get(1..).unwrap_or_default().to_vec();

        debug!(
            "Executing bundled tool: {} with command: {:?}",
            tool.name, tool.command
        );

        // Prepare arguments as JSON for the script
        let args_json = serde_json::to_string_pretty(arguments).map_err(|e| {
            RuntimeError::BundledToolError(format!("Failed to serialize arguments: {e}"))
        })?;

        // Determine working directory from script path
        let working_dir = if args.is_empty() {
            std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
        } else {
            Path::new(args.first().map_or("", String::as_str))
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .to_path_buf()
        };

        // Prepare environment variables
        let temp_dir = std::env::temp_dir();
        let args_file = temp_dir.join(format!("skill_args_{}.json", Uuid::new_v4()));
        std::fs::write(&args_file, &args_json)?;

        let env_vars = vec![
            (
                "SKILL_ARGS_FILE".to_string(),
                args_file.to_string_lossy().to_string(),
            ),
            ("SKILL_ARGS_JSON".to_string(), args_json),
        ];

        // Create sandbox with configuration merging
        let sandbox_config = tool_sandbox_override.map_or_else(
            || self.sandbox_config.clone(),
            |override_config| self.sandbox_config.with_override(override_config),
        );

        // Apply timeout override if provided
        let mut sandbox_config = sandbox_config;
        if let Some(timeout) = timeout_ms {
            sandbox_config.timeout_ms = timeout;
        }

        // Add working directory to allowed paths
        sandbox_config.allow_read.push(working_dir.clone());
        sandbox_config.allow_write.push(temp_dir);

        let sandbox = sandbox::Sandbox::new(sandbox_config);

        // Execute in sandbox
        let result = sandbox
            .execute(program, &args, &working_dir, &env_vars)
            .await;

        // Clean up temp file
        let _ = std::fs::remove_file(&args_file);

        let sandbox_result = result.map_err(|e| match e {
            sandbox::SandboxError::Timeout(ms) => RuntimeError::Timeout(ms),
            sandbox::SandboxError::Io(e) => RuntimeError::Io(e),
            e => RuntimeError::BundledToolError(e.to_string()),
        })?;

        info!(
            "Bundled tool {} completed in {}ms, success: {}",
            tool.name,
            sandbox_result.duration_ms,
            sandbox_result.exit_code.unwrap_or(-1) == 0
        );

        let success = sandbox_result.exit_code.unwrap_or(-1) == 0 && !sandbox_result.timed_out;

        if !success {
            warn!(
                "Bundled tool {} failed with stderr: {}",
                tool.name, sandbox_result.stderr
            );
        }

        // Try to parse stdout as JSON for structured content
        let structured_content =
            serde_json::from_str::<serde_json::Value>(&sandbox_result.stdout).ok();

        let mut content = vec![];

        if !sandbox_result.stdout.is_empty() {
            content.push(ToolResultContent::Text {
                text: sandbox_result.stdout,
            });
        }

        if !sandbox_result.stderr.is_empty() && !success {
            content.push(ToolResultContent::Text {
                text: format!("Error: {}", sandbox_result.stderr),
            });
        }

        if sandbox_result.timed_out {
            content.push(ToolResultContent::Text {
                text: "Execution timed out".to_string(),
            });
        }

        if content.is_empty() {
            content.push(ToolResultContent::Text {
                text: if success {
                    "Command completed successfully with no output".to_string()
                } else {
                    format!(
                        "Command failed with exit code: {:?}",
                        sandbox_result.exit_code
                    )
                },
            });
        }

        Ok(ToolResult {
            content,
            structured_content,
            is_error: !success,
        })
    }
}

/// Workflow DSL interpreter (placeholder)
pub struct WorkflowEngine {
    #[allow(dead_code)]
    registry: Arc<Registry>,
}

impl WorkflowEngine {
    #[must_use]
    pub const fn new(registry: Arc<Registry>) -> Self {
        Self { registry }
    }

    /// Execute a workflow definition
    /// # Errors
    /// Returns an error if the operation cannot be completed.
    #[expect(
        clippy::unused_async,
        reason = "workflow execution is async by contract before the implementation gains awaits"
    )]
    pub async fn execute(
        &self,
        _workflow: &serde_json::Value,
        _arguments: &serde_json::Value,
    ) -> Result<ToolResult> {
        // Full implementation would:
        // - Parse workflow YAML/JSON
        // - Execute steps in order
        // - Handle call, assign, map, branch, retry, validate, emit constructs
        // - Maintain variable context
        // - Enforce tool allowlists

        Ok(ToolResult {
            content: vec![ToolResultContent::Text {
                text: "Workflow executed (placeholder)".to_string(),
            }],
            structured_content: None,
            is_error: false,
        })
    }
}
