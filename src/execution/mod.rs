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
use crate::core::{
    BundledTool, CallableId, CallableKind, ExecutionStatus, ToolResult, ToolResultContent,
};
use crate::execution::sandbox::SandboxConfigOverride;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tracing::{debug, info, warn};
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
    pub started_at: Timestamp,
    pub completed_at: Option<Timestamp>,
    pub duration_ms: Option<u64>,
    pub success: bool,
    pub error: Option<String>,
}

/// Execution trace
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionTrace {
    pub execution_id: String,
    pub started_at: Timestamp,
    pub completed_at: Option<Timestamp>,
    pub total_duration_ms: Option<u64>,
    pub steps: Vec<TraceStep>,
}

impl ExecutionTrace {
    fn new() -> Self {
        Self {
            execution_id: Uuid::new_v4().to_string(),
            started_at: Timestamp::now(),
            completed_at: None,
            total_duration_ms: None,
            steps: Vec::new(),
        }
    }

    fn complete(&mut self) {
        let completed_at = Timestamp::now();
        self.completed_at = Some(completed_at);
        self.total_duration_ms = Some(duration_ms(self.started_at, completed_at));
    }
}

fn duration_ms(started_at: Timestamp, completed_at: Timestamp) -> u64 {
    u64::try_from(completed_at.duration_since(started_at).as_millis()).unwrap_or(0)
}

/// Runtime executor
pub struct Runtime {
    registry: Arc<Registry>,
    upstream_manager: Arc<UpstreamManager>,
    sandbox_config: sandbox::SandboxConfig,
    unrestricted: bool,
}

impl Runtime {
    #[must_use]
    pub fn new(registry: Arc<Registry>, upstream_manager: Arc<UpstreamManager>) -> Self {
        Self {
            registry,
            upstream_manager,
            sandbox_config: sandbox::SandboxConfig::default(),
            unrestricted: false,
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
            unrestricted: false,
        }
    }

    pub fn set_unrestricted(&mut self) {
        self.unrestricted = true;
        self.sandbox_config.backend = sandbox::SandboxBackend::None;
        self.sandbox_config.timeout_ms = 0;
        self.sandbox_config.allow_network = true;
        self.sandbox_config.allow_read.clear();
        self.sandbox_config.allow_write.clear();
        self.sandbox_config.max_memory_bytes = 0;
        self.sandbox_config.max_cpu_seconds = 0;
    }

    #[must_use]
    pub const fn sandbox_backend(&self) -> sandbox::SandboxBackend {
        self.sandbox_config.backend
    }

    /// Execute a callable
    /// # Errors
    /// Returns an error if the operation cannot be completed.
    pub async fn execute(&self, ctx: ExecContext) -> Result<ToolResult> {
        self.execute_with_continuation(ctx, None, None).await
    }

    /// Resume an upstream tool without modifying its opaque continuation payload.
    /// # Errors
    /// Returns validation, upstream, or effective-deadline errors.
    pub async fn execute_with_continuation(
        &self,
        ctx: ExecContext,
        input_responses: Option<serde_json::Value>,
        request_state: Option<serde_json::Value>,
    ) -> Result<ToolResult> {
        let record = self.get_callable_record(&ctx.callable_id)?;
        self.validate_callable_arguments(&ctx.callable_id, &ctx.arguments)?;
        if record.kind == CallableKind::Skill
            && (input_responses.is_some() || request_state.is_some())
        {
            return Err(RuntimeError::ValidationFailed(
                "Only upstream tools accept continuation data".to_string(),
            ));
        }
        let mut trace = ctx.trace_enabled.then(ExecutionTrace::new);
        let operation = async {
            match record.kind {
                CallableKind::Tool => {
                    self.execute_tool(&ctx, &record, input_responses, request_state)
                        .await
                }
                CallableKind::Skill => self.execute_skill(&ctx, &record, trace.as_mut()).await,
            }
        };
        let mut result = if let Some(timeout) = self.effective_timeout(&ctx, &record) {
            tokio::time::timeout(Duration::from_millis(timeout), operation)
                .await
                .map_err(|_| RuntimeError::Timeout(timeout))??
        } else {
            operation.await?
        };
        if let Some(mut trace) = trace {
            if result.status == ExecutionStatus::Completed {
                trace.complete();
            }
            result.trace = Some(
                serde_json::to_value(trace).map_err(|e| RuntimeError::Internal(e.to_string()))?,
            );
        }
        Ok(result)
    }

    fn effective_timeout(
        &self,
        ctx: &ExecContext,
        record: &crate::core::CallableRecord,
    ) -> Option<u64> {
        if self.unrestricted {
            return None;
        }
        let config = record.sandbox_config.as_ref().map_or_else(
            || self.sandbox_config.clone(),
            |overrides| self.sandbox_config.with_override(overrides),
        );
        match (config.timeout_ms, ctx.timeout_ms.filter(|value| *value > 0)) {
            (0, timeout) => timeout,
            (configured, Some(requested)) => Some(configured.min(requested)),
            (configured, None) => Some(configured),
        }
    }

    /// Run only an explicitly selected bundled helper, by name or skill-relative path.
    /// # Errors
    /// Rejects unknown/ambiguous helpers and returns validation or process failures.
    pub async fn execute_script(&self, ctx: ExecContext, script: &str) -> Result<ToolResult> {
        let record = self.get_callable_record(&ctx.callable_id)?;
        let (tool, root) = Self::selected_script(&record, script)?;
        Self::validate_arguments(&tool.schema, &ctx.arguments)?;
        let started_at = Timestamp::now();
        let mut result = self
            .execute_bundled_tool(
                tool,
                &ctx.arguments,
                &root,
                self.effective_timeout(&ctx, &record),
                if self.unrestricted {
                    None
                } else {
                    record.sandbox_config.as_ref()
                },
            )
            .await?;
        if ctx.trace_enabled {
            let mut trace = ExecutionTrace::new();
            trace.started_at = started_at;
            trace.complete();
            result.trace = Some(
                serde_json::to_value(trace)
                    .map_err(|error| RuntimeError::Internal(error.to_string()))?,
            );
        }
        Ok(result)
    }

    /// Validate an explicit helper selection without starting a process.
    /// # Errors
    /// Rejects missing/ambiguous helpers and arguments missing required fields.
    pub fn validate_script_arguments(
        &self,
        callable_id: &CallableId,
        script: &str,
        arguments: &serde_json::Value,
    ) -> Result<()> {
        let record = self.get_callable_record(callable_id)?;
        let (tool, _) = Self::selected_script(&record, script)?;
        Self::validate_arguments(&tool.schema, arguments)
    }

    fn selected_script<'a>(
        record: &'a crate::core::CallableRecord,
        script: &str,
    ) -> Result<(&'a BundledTool, std::path::PathBuf)> {
        if record.kind != CallableKind::Skill {
            return Err(RuntimeError::ValidationFailed(
                "Script selection requires a skill".to_string(),
            ));
        }
        let root = record
            .skill_directory
            .as_ref()
            .ok_or_else(|| {
                RuntimeError::ExecutionFailed("Skill directory is unavailable".to_string())
            })?
            .canonicalize()?;
        let relative = Path::new(script);
        let selected_path = if !script.is_empty()
            && relative
                .components()
                .all(|part| matches!(part, std::path::Component::Normal(_)))
        {
            root.join(relative)
                .canonicalize()
                .ok()
                .filter(|path| path.starts_with(&root))
        } else {
            None
        };
        let mut matches = record.bundled_tools.iter().filter(|tool| {
            tool.name == script
                || selected_path.as_ref().is_some_and(|selected| {
                    tool.command.iter().any(|part| {
                        let path = Path::new(part);
                        let path = if path.is_absolute() {
                            path.to_path_buf()
                        } else {
                            root.join(path)
                        };
                        path.canonicalize().is_ok_and(|path| path == *selected)
                    })
                })
        });
        let tool = matches.next().ok_or_else(|| {
            RuntimeError::ValidationFailed(format!("Unknown bundled helper: {script}"))
        })?;
        if matches.next().is_some() {
            return Err(RuntimeError::ValidationFailed(format!(
                "Ambiguous bundled helper: {script}"
            )));
        }
        Ok((tool, root))
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
        if record.kind == CallableKind::Skill {
            return Ok(());
        }
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
        input_responses: Option<serde_json::Value>,
        request_state: Option<serde_json::Value>,
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

        let raw = self
            .upstream_manager
            .call_tool_with_continuation(
                server,
                tool_name,
                ctx.arguments.clone(),
                input_responses,
                request_state,
            )
            .await
            .map_err(|error| RuntimeError::UpstreamError(error.to_string()))?;
        Ok(ToolResult::from_upstream(raw))
    }

    /// Execute a skill (workflow orchestration)
    async fn execute_skill(
        &self,
        ctx: &ExecContext,
        record: &crate::core::CallableRecord,
        trace: Option<&mut ExecutionTrace>,
    ) -> Result<ToolResult> {
        let step_start = Timestamp::now();
        let root = record
            .skill_directory
            .as_ref()
            .ok_or_else(|| {
                RuntimeError::ExecutionFailed("Skill directory is unavailable".to_string())
            })?
            .canonicalize()?;
        let document_path = root.join("SKILL.md").canonicalize()?;
        if !document_path.starts_with(&root) {
            return Err(RuntimeError::ExecutionFailed(
                "SKILL.md escapes the skill directory".to_string(),
            ));
        }
        let document = tokio::fs::read_to_string(document_path).await?;
        let body = if document.starts_with("---\n") || document.starts_with("---\r\n") {
            crate::storage::agent_skills::parse_frontmatter_public(&document)
                .map_err(|error| RuntimeError::ExecutionFailed(error.to_string()))?
                .1
        } else {
            document
        };
        let result = ToolResult::success(body.clone()).with_structured(serde_json::json!({
            "skill": record.name,
            "version": record.skill_version,
            "body": body,
            "resources_root": root,
            "uses_tools": record.uses.iter().map(CallableId::as_str).collect::<Vec<_>>(),
            "bundled_tools": record.bundled_tools.iter().map(|tool| &tool.name).collect::<Vec<_>>(),
            "additional_files": record.additional_files,
        }));

        // Record trace step
        if let Some(trace) = trace {
            let step_end = Timestamp::now();
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
        working_dir: &Path,
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
        let args = tool.command.get(1..).unwrap_or_default();

        debug!(
            "Executing bundled tool: {} with command: {:?}",
            tool.name, tool.command
        );

        // Prepare arguments as JSON for the script
        let args_json = serde_json::to_string_pretty(arguments).map_err(|e| {
            RuntimeError::BundledToolError(format!("Failed to serialize arguments: {e}"))
        })?;

        let invocation = tempfile::tempdir()?;
        let args_file = invocation.path().join("arguments.json");
        tokio::fs::write(&args_file, &args_json).await?;

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
        if self.unrestricted {
            sandbox_config.backend = sandbox::SandboxBackend::None;
            sandbox_config.timeout_ms = 0;
        }
        if !self.unrestricted {
            if let Some(timeout) = timeout_ms {
                sandbox_config.timeout_ms = timeout;
            }
        }

        if matches!(
            sandbox_config.backend,
            sandbox::SandboxBackend::Bubblewrap
                | sandbox::SandboxBackend::Docker
                | sandbox::SandboxBackend::Wasm
        ) {
            sandbox_config.allow_read.push(working_dir.to_path_buf());
            sandbox_config
                .allow_write
                .push(invocation.path().to_path_buf());
        }

        let sandbox = sandbox::Sandbox::new(sandbox_config);

        // Execute in sandbox
        let result = sandbox.execute(program, args, working_dir, &env_vars).await;

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

        if content.is_empty() && !success {
            content.push(ToolResultContent::Text {
                text: format!(
                    "Command failed with exit code: {:?}",
                    sandbox_result.exit_code
                ),
            });
        }

        Ok(ToolResult {
            content,
            structured_content,
            is_error: !success,
            status: ExecutionStatus::Completed,
            raw_output: None,
            trace: None,
        })
    }
}
