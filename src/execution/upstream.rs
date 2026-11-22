//! Upstream MCP Server Manager
//!
//! Uses current, self-contained MCP requests over stdio and Streamable HTTP.

use crate::core::registry::{Registry, ServerHealth, ServerInfo};
use crate::core::{
    CallableId, CallableKind, CallableRecord, CostHints, RiskTier, SchemaDigest, ToolDefinition,
};
use crate::execution::sandbox::{
    PreparedCommand, ProcessGroup, Sandbox, SandboxBackend, SandboxConfig, SandboxConfigOverride,
};
use jiff::Timestamp;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::{mpsc, oneshot, RwLock};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

/// Protocol revision required by the upstream client.
pub const MCP_CURRENT_PROTOCOL_VERSION: &str = "2026-07-28";
const MCP_CLIENT_NAME: &str = "skills.rs";
const MCP_CLIENT_VERSION: &str = env!("CARGO_PKG_VERSION");

fn current_request_meta() -> JsonValue {
    serde_json::json!({
        "io.modelcontextprotocol/protocolVersion": MCP_CURRENT_PROTOCOL_VERSION,
        "io.modelcontextprotocol/clientCapabilities": {},
        "io.modelcontextprotocol/clientInfo": {
            "name": MCP_CLIENT_NAME,
            "version": MCP_CLIENT_VERSION
        }
    })
}

fn current_request_params(mut params: JsonValue) -> JsonValue {
    let Some(object) = params.as_object_mut() else {
        return serde_json::json!({ "_meta": current_request_meta() });
    };
    object.insert("_meta".to_string(), current_request_meta());
    params
}

fn mcp_http_endpoint(url: &str) -> String {
    let url = url.trim().trim_end_matches('/');
    if url.ends_with("/mcp") {
        url.to_string()
    } else {
        format!("{url}/mcp")
    }
}

fn parse_mcp_message(payload: JsonValue) -> Result<Option<McpResponse>> {
    let mut payload = match payload {
        JsonValue::Object(payload)
            if payload.get("jsonrpc").and_then(JsonValue::as_str) == Some("2.0") =>
        {
            payload
        }
        _ => {
            return Err(UpstreamError::ProtocolError(
                "Invalid JSON-RPC version".to_string(),
            ));
        }
    };
    if payload.contains_key("method") {
        if payload.contains_key("id") {
            return Err(UpstreamError::ProtocolError(
                "Current MCP does not allow server-initiated requests".to_string(),
            ));
        }
        return Ok(None);
    }
    let id = payload
        .remove("id")
        .filter(|id| id.is_string() || id.is_number())
        .ok_or_else(|| UpstreamError::ProtocolError("Response has no valid id".to_string()))?;
    let result = payload.remove("result");
    let error = payload.remove("error");
    if result.is_some() == error.is_some() {
        return Err(UpstreamError::ProtocolError(
            "Response must contain exactly one of result or error".to_string(),
        ));
    }
    Ok(Some(McpResponse { id, result, error }))
}

fn response_result(response: McpResponse) -> Result<JsonValue> {
    if let Some(error) = response.error {
        return Err(UpstreamError::RequestFailed(error.to_string()));
    }
    response
        .result
        .ok_or_else(|| UpstreamError::ProtocolError("Response has no result".to_string()))
}

#[derive(Default)]
struct SseDecoder {
    line: Vec<u8>,
    data: Vec<u8>,
    after_cr: bool,
    started: bool,
}

impl SseDecoder {
    fn feed(&mut self, bytes: &[u8], expected_id: &JsonValue) -> Result<Option<McpResponse>> {
        for &byte in bytes {
            if self.after_cr && byte == b'\n' {
                self.after_cr = false;
                continue;
            }
            self.after_cr = byte == b'\r';
            if byte == b'\r' || byte == b'\n' {
                if let Some(response) = self.finish_line(expected_id)? {
                    return Ok(Some(response));
                }
            } else {
                self.line.push(byte);
            }
        }
        Ok(None)
    }

    fn finish_line(&mut self, expected_id: &JsonValue) -> Result<Option<McpResponse>> {
        if self.line.is_empty() {
            if self.data.is_empty() {
                return Ok(None);
            }
            self.data.pop();
            if self.data == b"[DONE]" {
                self.data.clear();
                return Ok(None);
            }
            let payload = serde_json::from_slice(&self.data).map_err(|error| {
                UpstreamError::ProtocolError(format!("Invalid MCP SSE event: {error}"))
            })?;
            self.data.clear();
            return Ok(parse_mcp_message(payload)?.filter(|response| &response.id == expected_id));
        }
        let line = if self.started {
            self.line.as_slice()
        } else {
            self.started = true;
            self.line
                .strip_prefix(b"\xef\xbb\xbf")
                .unwrap_or(&self.line)
        };
        let (field, value) =
            line.iter()
                .position(|&byte| byte == b':')
                .map_or((line, &[][..]), |position| {
                    let (field, value) = line.split_at(position);
                    (field, value.strip_prefix(b":").unwrap_or_default())
                });
        if field == b"data" {
            self.data
                .extend_from_slice(value.strip_prefix(b" ").unwrap_or(value));
            self.data.push(b'\n');
        }
        self.line.clear();
        Ok(None)
    }
}

fn parse_mcp_http_response(body: &[u8], expected_id: &JsonValue) -> Result<McpResponse> {
    let payload = serde_json::from_slice(body).map_err(|error| {
        UpstreamError::ProtocolError(format!("Failed to parse MCP response: {error}"))
    })?;
    parse_mcp_message(payload)?
        .filter(|response| &response.id == expected_id)
        .ok_or_else(|| {
            UpstreamError::ProtocolError("MCP response id does not match request".to_string())
        })
}

async fn with_timeout<T>(
    timeout: Option<Duration>,
    method: &str,
    future: impl Future<Output = Result<T>>,
) -> Result<T> {
    if let Some(timeout) = timeout {
        tokio::time::timeout(timeout, future)
            .await
            .map_err(|_| UpstreamError::Timeout(format!("{method} request timed out")))?
    } else {
        future.await
    }
}

#[derive(Error, Debug)]
pub enum UpstreamError {
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),

    #[error("Request failed: {0}")]
    RequestFailed(String),

    #[error("Protocol error: {0}")]
    ProtocolError(String),

    #[error("Server not found: {0}")]
    ServerNotFound(String),

    #[error("Timeout: {0}")]
    Timeout(String),
}

pub type Result<T> = std::result::Result<T, UpstreamError>;

/// Transport type for upstream connection
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    Stdio,
    Http,
    #[serde(rename = "http+sse")]
    HttpSse,
    /// Agent Skills from a Git repository
    #[serde(rename = "agent_skills_repo")]
    AgentSkillsRepo,
    /// Agent Skills from local filesystem
    #[serde(rename = "agent_skills_fs")]
    AgentSkillsFs,
}

/// Upstream server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpstreamConfig {
    pub alias: String,
    /// Explicitly required for Bench readiness; semantic core aliases are also required in open mode.
    #[serde(default)]
    pub required: bool,
    pub transport: Transport,

    // For stdio
    pub command: Option<Vec<String>>,

    // For HTTP
    pub url: Option<String>,
    pub auth: Option<AuthConfig>,

    // For Agent Skills Repo
    /// Git repository URL (e.g., "<https://github.com/owner/repo>" or "owner/repo")
    pub repo: Option<String>,
    /// Git ref (branch, tag, or commit SHA)
    pub git_ref: Option<String>,
    /// Specific skill names to import (if omitted, imports all)
    pub skills: Option<Vec<String>>,

    // For Agent Skills FS
    /// Local filesystem roots to scan for Agent Skills
    pub roots: Option<Vec<String>>,

    pub tags: Vec<String>,

    /// Sandbox configuration override for this upstream server
    #[serde(default)]
    pub sandbox_config: Option<SandboxConfigOverride>,

    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    #[serde(rename = "type")]
    pub auth_type: String,
    pub env: Option<String>,
    pub token: Option<String>,
}

/// MCP protocol messages
#[derive(Debug, Clone, Serialize, Deserialize)]
struct McpRequest {
    jsonrpc: String,
    id: JsonValue,
    method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<JsonValue>,
}

#[derive(Debug)]
struct McpResponse {
    id: JsonValue,
    result: Option<JsonValue>,
    error: Option<JsonValue>,
}

/// Upstream connection state
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Degraded,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct UpstreamStatus {
    pub alias: String,
    pub state: ConnectionState,
    pub tool_count: usize,
    pub required: bool,
}

impl ConnectionState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Disconnected => "disconnected",
            Self::Connecting => "connecting",
            Self::Connected => "connected",
            Self::Degraded => "degraded",
            Self::Failed => "failed",
        }
    }
}

type ResponseSender = oneshot::Sender<Result<JsonValue>>;

enum StdioCommand {
    Request(McpRequest, ResponseSender),
    Cancel(JsonValue),
    Shutdown,
}

struct PendingRequest {
    id: JsonValue,
    sender: mpsc::UnboundedSender<StdioCommand>,
    completed: bool,
}

impl Drop for PendingRequest {
    fn drop(&mut self) {
        if !self.completed {
            let _ = self.sender.send(StdioCommand::Cancel(self.id.clone()));
        }
    }
}

struct UpstreamSession {
    config: UpstreamConfig,
    state: ConnectionState,
    last_ping: Option<Timestamp>,
    tools: Vec<CallableId>,
    connection_id: Uuid,
    request_tx: Option<mpsc::UnboundedSender<StdioCommand>>,
}

fn validate_discovery(result: &JsonValue) -> Result<()> {
    let supported = result
        .get("supportedVersions")
        .and_then(JsonValue::as_array)
        .is_some_and(|versions| {
            versions
                .iter()
                .any(|version| version.as_str() == Some(MCP_CURRENT_PROTOCOL_VERSION))
        });
    if supported {
        Ok(())
    } else {
        Err(UpstreamError::ProtocolError(format!(
            "Upstream server does not advertise MCP {MCP_CURRENT_PROTOCOL_VERSION}"
        )))
    }
}

fn http_auth_token(auth: Option<&AuthConfig>, endpoint: &reqwest::Url) -> Result<Option<String>> {
    if !matches!(endpoint.scheme(), "http" | "https")
        || !endpoint.username().is_empty()
        || endpoint.password().is_some()
    {
        return Err(UpstreamError::ConnectionFailed(
            "MCP endpoint must be HTTP(S) without embedded credentials".to_string(),
        ));
    }
    let Some(auth) = auth else { return Ok(None) };
    match auth.auth_type.as_str() {
        "none" if auth.token.is_none() && auth.env.is_none() => return Ok(None),
        "bearer" => {}
        _ => {
            return Err(UpstreamError::ConnectionFailed(format!(
                "Unsupported upstream authentication type: {}",
                auth.auth_type
            )))
        }
    }
    let token = if let Some(token) = &auth.token {
        token.clone()
    } else if let Some(name) = &auth.env {
        std::env::var(name).map_err(|_| {
            UpstreamError::ConnectionFailed(format!(
                "Upstream authentication environment variable is unavailable: {name}"
            ))
        })?
    } else {
        return Err(UpstreamError::ConnectionFailed(
            "Bearer authentication requires a token".to_string(),
        ));
    };
    if token.trim().is_empty() {
        return Err(UpstreamError::ConnectionFailed(
            "Bearer authentication token is empty".to_string(),
        ));
    }
    let loopback = endpoint.host_str().is_some_and(|host| {
        host == "localhost"
            || host
                .trim_start_matches('[')
                .trim_end_matches(']')
                .parse::<std::net::IpAddr>()
                .is_ok_and(|address| address.is_loopback())
    });
    if endpoint.scheme() != "https" && !loopback {
        return Err(UpstreamError::ConnectionFailed(
            "Refusing to transmit upstream credentials over non-loopback HTTP".to_string(),
        ));
    }
    Ok(Some(token))
}

struct StdioRuntime {
    child: Child,
    stdin: ChildStdin,
    stdout: ChildStdout,
    _prepared: PreparedCommand,
    group: ProcessGroup,
    timeout: Option<Duration>,
    alias: String,
    connection_id: Uuid,
    sessions: Weak<RwLock<HashMap<String, UpstreamSession>>>,
    registry: Arc<Registry>,
}

fn queue_stdio(messages: &mpsc::UnboundedSender<Vec<u8>>, message: &impl Serialize) -> Result<()> {
    let mut bytes = serde_json::to_vec(message).map_err(|error| {
        UpstreamError::ProtocolError(format!("Cannot serialize MCP message: {error}"))
    })?;
    bytes.push(b'\n');
    messages
        .send(bytes)
        .map_err(|_| UpstreamError::ConnectionFailed("Upstream writer stopped".to_string()))
}

async fn run_stdio_writer(
    mut stdin: ChildStdin,
    mut messages: mpsc::UnboundedReceiver<Vec<u8>>,
    timeout: Option<Duration>,
) -> Result<()> {
    while let Some(bytes) = messages.recv().await {
        with_timeout(timeout, "stdio write", async {
            stdin.write_all(&bytes).await.map_err(|error| {
                UpstreamError::ConnectionFailed(format!("Cannot write MCP message: {error}"))
            })?;
            stdin.flush().await.map_err(|error| {
                UpstreamError::ConnectionFailed(format!("Cannot flush MCP message: {error}"))
            })
        })
        .await?;
    }
    Ok(())
}

impl StdioRuntime {
    #[expect(
        clippy::too_many_lines,
        reason = "one owner routes stdio and closes every pending request on termination"
    )]
    async fn run(mut self, mut requests: mpsc::UnboundedReceiver<StdioCommand>) {
        let mut lines = BufReader::new(self.stdout).lines();
        let (messages, message_rx) = mpsc::unbounded_channel();
        let mut writer = tokio::spawn(run_stdio_writer(self.stdin, message_rx, self.timeout));
        let mut pending: HashMap<JsonValue, ResponseSender> = HashMap::new();
        let mut process_exited = false;
        let failure = loop {
            tokio::select! {
                line = lines.next_line() => {
                    match line {
                        Ok(Some(line)) if line.trim().is_empty() => {}
                        Ok(Some(line)) => {
                            let response = serde_json::from_str(&line)
                                .map_err(|error| UpstreamError::ProtocolError(error.to_string()))
                                .and_then(parse_mcp_message);
                            match response {
                                Ok(Some(response)) => {
                                    if let Some(sender) = pending.remove(&response.id) {
                                        let _ = sender.send(response_result(response));
                                    }
                                }
                                Ok(None) => {}
                                Err(error) => break error.to_string(),
                            }
                        }
                        Ok(None) => break "Upstream closed stdout".to_string(),
                        Err(error) => break format!("Cannot read upstream stdout: {error}"),
                    }
                }
                command = requests.recv() => {
                    match command {
                        Some(StdioCommand::Request(request, sender)) => {
                            if sender.is_closed() {
                                continue;
                            }
                            pending.insert(request.id.clone(), sender);
                            if let Err(error) = queue_stdio(&messages, &request) {
                                break error.to_string();
                            }
                        }
                        Some(StdioCommand::Cancel(id)) => {
                            if pending.remove(&id).is_some() {
                                let notification = serde_json::json!({
                                    "jsonrpc": "2.0",
                                    "method": "notifications/cancelled",
                                    "params": { "requestId": id }
                                });
                                if let Err(error) = queue_stdio(&messages, &notification) {
                                    break error.to_string();
                                }
                            }
                        }
                        Some(StdioCommand::Shutdown) | None => break "Upstream disconnected".to_string(),
                    }
                }
                status = self.child.wait(), if !process_exited => {
                    process_exited = true;
                    let _ = self.group.terminate();
                    debug!("Upstream process {} exited: {status:?}", self.alias);
                }
                result = &mut writer => break format!("Upstream writer ended: {result:?}"),
            }
        };
        requests.close();
        if let Some(sessions) = self.sessions.upgrade() {
            let mut sessions = sessions.write().await;
            if let Some(session) = sessions
                .get_mut(&self.alias)
                .filter(|session| session.connection_id == self.connection_id)
            {
                session.state = ConnectionState::Disconnected;
                session.last_ping = None;
                session.tools.clear();
                session.request_tx = None;
                self.registry.remove_server(&self.alias);
                self.registry.update_server(ServerInfo {
                    alias: self.alias.clone(),
                    health: ServerHealth::Down,
                    tool_count: 0,
                    last_refresh: Timestamp::now(),
                    tags: session.config.tags.clone(),
                });
            }
        }
        for sender in pending.into_values() {
            let _ = sender.send(Err(UpstreamError::ConnectionFailed(failure.clone())));
        }
        while let Ok(command) = requests.try_recv() {
            if let StdioCommand::Request(_, sender) = command {
                let _ = sender.send(Err(UpstreamError::ConnectionFailed(failure.clone())));
            }
        }
        writer.abort();
        drop(messages);
        if tokio::time::timeout(Duration::from_secs(1), self.child.wait())
            .await
            .is_err()
        {
            let _ = self.group.terminate();
            let _ = self.child.kill().await;
        }
        debug!("Stdio connection {} ended: {}", self.alias, failure);
    }
}

/// Upstream manager
pub struct UpstreamManager {
    sessions: Arc<RwLock<HashMap<String, UpstreamSession>>>,
    registry: Arc<Registry>,
    http_client: std::result::Result<reqwest::Client, reqwest::Error>,
    sandbox_config: RwLock<SandboxConfig>,
    unrestricted: AtomicBool,
}

impl UpstreamManager {
    #[must_use]
    pub fn new(registry: Arc<Registry>) -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            registry,
            http_client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build(),
            sandbox_config: RwLock::new(SandboxConfig::default()),
            unrestricted: AtomicBool::new(false),
        }
    }

    /// Remove the fixed stdio tool-call timeout for the open workspace profile.
    pub fn set_unrestricted(&self) {
        self.unrestricted.store(true, Ordering::Release);
    }

    /// Set the base process policy before connecting upstreams.
    pub async fn set_sandbox_config(&self, config: SandboxConfig) {
        *self.sandbox_config.write().await = config;
    }

    async fn resolved_sandbox_config(&self, config: &UpstreamConfig) -> SandboxConfig {
        let base = if self.unrestricted.load(Ordering::Acquire) {
            SandboxConfig {
                backend: SandboxBackend::None,
                timeout_ms: 0,
                allow_network: true,
                max_memory_bytes: 0,
                max_cpu_seconds: 0,
                ..SandboxConfig::default()
            }
        } else {
            self.sandbox_config.read().await.clone()
        };
        if let Some(overrides) = &config.sandbox_config {
            overrides.resolve(&base)
        } else {
            base
        }
    }

    async fn request_timeout(&self, config: &UpstreamConfig) -> Option<Duration> {
        let timeout_ms = if let Some(timeout) = config
            .sandbox_config
            .as_ref()
            .and_then(|config| config.timeout_ms)
        {
            timeout
        } else if let Some(preset) = config
            .sandbox_config
            .as_ref()
            .and_then(|config| config.preset)
        {
            preset.to_config().timeout_ms
        } else if self.unrestricted.load(Ordering::Acquire) {
            0
        } else {
            self.sandbox_config.read().await.timeout_ms
        };
        if timeout_ms == 0 && self.unrestricted.load(Ordering::Acquire) {
            None
        } else {
            Some(Duration::from_millis(if timeout_ms == 0 {
                30_000
            } else {
                timeout_ms
            }))
        }
    }

    /// Add an upstream server
    /// # Errors
    /// Returns an error if the operation cannot be completed.
    pub async fn add_upstream(&self, config: UpstreamConfig) -> Result<()> {
        let alias = config.alias.clone();
        info!("Adding upstream server: {}", alias);

        let session = UpstreamSession {
            config: config.clone(),
            state: ConnectionState::Disconnected,
            last_ping: None,
            tools: Vec::new(),
            connection_id: Uuid::new_v4(),
            request_tx: None,
        };

        self.sessions.write().await.insert(alias.clone(), session);

        // Connect and fetch tools
        self.connect(&alias).await?;

        Ok(())
    }

    /// Connect to an upstream server
    /// # Errors
    /// Returns an error if the operation cannot be completed.
    pub async fn connect(&self, alias: &str) -> Result<()> {
        info!("Connecting to upstream: {}", alias);
        let (config, connection_id) = {
            let mut sessions = self.sessions.write().await;
            let session = sessions
                .get_mut(alias)
                .ok_or_else(|| UpstreamError::ServerNotFound(alias.to_string()))?;
            if let Some(sender) = session.request_tx.take() {
                let _ = sender.send(StdioCommand::Shutdown);
            }
            session.connection_id = Uuid::new_v4();
            session.state = ConnectionState::Connecting;
            session.last_ping = None;
            session.tools.clear();
            let connection = (session.config.clone(), session.connection_id);
            drop(sessions);
            connection
        };
        self.registry.remove_server(alias);
        let result = async {
            match config.transport {
                Transport::Stdio => self.connect_stdio(&config, connection_id).await?,
                Transport::Http | Transport::HttpSse => self.connect_http(&config).await?,
                Transport::AgentSkillsRepo | Transport::AgentSkillsFs => {}
            }
            self.refresh_tools(alias).await?;
            let mut sessions = self.sessions.write().await;
            let session = sessions
                .get_mut(alias)
                .filter(|session| session.connection_id == connection_id)
                .ok_or_else(|| {
                    UpstreamError::ConnectionFailed(
                        "Connection changed during discovery".to_string(),
                    )
                })?;
            if matches!(config.transport, Transport::Stdio)
                && session
                    .request_tx
                    .as_ref()
                    .is_none_or(mpsc::UnboundedSender::is_closed)
            {
                return Err(UpstreamError::ConnectionFailed(
                    "Upstream exited during discovery".to_string(),
                ));
            }
            session.state = ConnectionState::Connected;
            session.last_ping = Some(Timestamp::now());
            self.registry.update_server(ServerInfo {
                alias: alias.to_string(),
                health: ServerHealth::Connected,
                tool_count: session.tools.len(),
                last_refresh: Timestamp::now(),
                tags: config.tags.clone(),
            });
            drop(sessions);
            Ok(())
        }
        .await;
        if result.is_err() {
            self.mark_disconnected(alias, Some(connection_id)).await;
        }
        result
    }

    async fn mark_disconnected(&self, alias: &str, expected_connection: Option<Uuid>) {
        let mut sessions = self.sessions.write().await;
        if let Some(session) = sessions.get_mut(alias).filter(|session| {
            expected_connection.is_none_or(|expected| expected == session.connection_id)
        }) {
            if let Some(sender) = session.request_tx.take() {
                let _ = sender.send(StdioCommand::Shutdown);
            }
            session.state = ConnectionState::Disconnected;
            session.tools.clear();
            session.last_ping = None;
            self.registry.remove_server(alias);
            self.registry.update_server(ServerInfo {
                alias: alias.to_string(),
                health: ServerHealth::Down,
                tool_count: 0,
                last_refresh: Timestamp::now(),
                tags: session.config.tags.clone(),
            });
        }
    }

    /// Start a contained stdio process and probe the current protocol without initialization.
    async fn connect_stdio(&self, config: &UpstreamConfig, connection_id: Uuid) -> Result<()> {
        let command = config
            .command
            .as_ref()
            .ok_or_else(|| UpstreamError::ConnectionFailed("No command specified".to_string()))?;
        let (program, arguments) = command
            .split_first()
            .ok_or_else(|| UpstreamError::ConnectionFailed("Command list is empty".to_string()))?;
        let sandbox = Sandbox::new(self.resolved_sandbox_config(config).await);
        let directory = std::env::current_dir().map_err(|error| {
            UpstreamError::ConnectionFailed(format!("Cannot resolve working directory: {error}"))
        })?;
        let mut prepared = sandbox
            .prepare_command(program, arguments, &directory, &[])
            .map_err(|error| UpstreamError::ConnectionFailed(error.to_string()))?;
        prepared
            .command_mut()
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        let (mut child, group) = prepared
            .spawn()
            .map_err(|error| UpstreamError::ConnectionFailed(error.to_string()))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| UpstreamError::ConnectionFailed("Failed to get stdin".to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| UpstreamError::ConnectionFailed("Failed to get stdout".to_string()))?;
        let (request_tx, request_rx) = mpsc::unbounded_channel();
        {
            let mut sessions = self.sessions.write().await;
            let session = sessions
                .get_mut(&config.alias)
                .filter(|session| session.connection_id == connection_id)
                .ok_or_else(|| {
                    UpstreamError::ConnectionFailed(
                        "Connection changed before stdio startup".to_string(),
                    )
                })?;
            session.request_tx = Some(request_tx);
            drop(sessions);
        }
        let runtime = StdioRuntime {
            child,
            stdin,
            stdout,
            _prepared: prepared,
            group,
            timeout: self.request_timeout(config).await,
            alias: config.alias.clone(),
            connection_id,
            sessions: Arc::downgrade(&self.sessions),
            registry: self.registry.clone(),
        };
        tokio::spawn(runtime.run(request_rx));
        let result = self
            .send_stdio_request(config, "server/discover", serde_json::json!({}))
            .await?;
        validate_discovery(&result)
    }

    async fn connect_http(&self, config: &UpstreamConfig) -> Result<()> {
        let result = self
            .send_current_http_request(config, "server/discover", serde_json::json!({}))
            .await?;
        validate_discovery(&result)
    }

    /// Refresh tools from an upstream server
    /// # Errors
    /// Returns an error if the operation cannot be completed.
    pub async fn refresh_tools(&self, alias: &str) -> Result<()> {
        info!("Refreshing tools from: {}", alias);

        let (config, connection_id) = {
            let sessions = self.sessions.read().await;
            let session = sessions
                .get(alias)
                .ok_or_else(|| UpstreamError::ServerNotFound(alias.to_string()))?;
            let connection = (session.config.clone(), session.connection_id);
            drop(sessions);
            connection
        };

        // Fetch tools based on transport
        let tools = match config.transport {
            Transport::Stdio | Transport::Http | Transport::HttpSse => {
                self.list_tools(&config).await?
            }
            Transport::AgentSkillsRepo | Transport::AgentSkillsFs => {
                // Agent Skills don't provide MCP tools, they provide skills
                // Skills are discovered by SkillStore directly
                debug!("Agent Skills transport - no MCP tools to refresh");
                Vec::new()
            }
        };

        let mut sessions = self.sessions.write().await;
        let session = sessions
            .get_mut(alias)
            .filter(|session| session.connection_id == connection_id)
            .ok_or_else(|| {
                UpstreamError::ConnectionFailed(
                    "Connection changed during tool discovery".to_string(),
                )
            })?;
        if matches!(config.transport, Transport::Stdio)
            && session
                .request_tx
                .as_ref()
                .is_none_or(mpsc::UnboundedSender::is_closed)
        {
            return Err(UpstreamError::ConnectionFailed(
                "Upstream exited during tool discovery".to_string(),
            ));
        }

        // Remove old tools for this server
        self.registry.remove_server(alias);

        // Register new tools
        let mut tool_ids = Vec::new();
        for tool_def in tools {
            let digest = SchemaDigest::from_schema(&tool_def.input_schema)
                .map_err(|e| UpstreamError::ProtocolError(e.to_string()))?;

            let id = CallableId::tool(alias, &tool_def.name, digest.as_str());

            let record = CallableRecord {
                id: id.clone(),
                kind: CallableKind::Tool,
                fq_name: format!("{}.{}", alias, tool_def.name),
                name: tool_def.name.clone(),
                title: Some(tool_def.name.clone()),
                description: tool_def.description.clone(),
                tags: vec![alias.to_string()],
                input_schema: tool_def.input_schema.clone(),
                output_schema: None,
                schema_digest: digest,
                server_alias: Some(alias.to_string()),
                upstream_tool_name: Some(tool_def.name.clone()),
                skill_version: None,
                uses: vec![],
                skill_directory: None,
                bundled_tools: vec![],
                additional_files: vec![],
                cost_hints: CostHints::default(),
                risk_tier: RiskTier::Unknown,
                last_seen: Timestamp::now(),
                sandbox_config: None,
            };

            self.registry
                .register(record)
                .map_err(|e| UpstreamError::ProtocolError(e.to_string()))?;

            tool_ids.push(id);
        }

        session.tools.clone_from(&tool_ids);

        // Update server info
        self.registry.update_server(ServerInfo {
            alias: alias.to_string(),
            health: ServerHealth::Connected,
            tool_count: tool_ids.len(),
            last_refresh: Timestamp::now(),
            tags: config.tags,
        });
        drop(sessions);

        info!("Refreshed {} tools from {}", tool_ids.len(), alias);
        Ok(())
    }

    async fn list_tools(&self, config: &UpstreamConfig) -> Result<Vec<ToolDefinition>> {
        let mut tools = Vec::new();
        let mut cursor = None;
        let mut seen_cursors = HashSet::new();
        loop {
            let mut params = serde_json::Map::new();
            if let Some(cursor) = cursor.take() {
                params.insert("cursor".to_string(), JsonValue::String(cursor));
            }
            let response = self
                .send_request(config, "tools/list", JsonValue::Object(params))
                .await?;
            let tools_array = response
                .get("tools")
                .and_then(JsonValue::as_array)
                .ok_or_else(|| {
                    UpstreamError::ProtocolError("Invalid tools/list response".to_string())
                })?;
            for tool in tools_array {
                let name = tool
                    .get("name")
                    .and_then(JsonValue::as_str)
                    .ok_or_else(|| UpstreamError::ProtocolError("Tool missing name".to_string()))?;
                tools.push(ToolDefinition {
                    name: name.to_string(),
                    description: tool
                        .get("description")
                        .and_then(JsonValue::as_str)
                        .map(str::to_string),
                    input_schema: tool
                        .get("inputSchema")
                        .cloned()
                        .unwrap_or_else(|| serde_json::json!({ "type": "object" })),
                });
            }
            match response.get("nextCursor") {
                None | Some(JsonValue::Null) => return Ok(tools),
                Some(JsonValue::String(next)) => {
                    if !seen_cursors.insert(next.clone()) {
                        return Err(UpstreamError::ProtocolError(
                            "tools/list repeated a pagination cursor".to_string(),
                        ));
                    }
                    cursor = Some(next.clone());
                }
                Some(_) => {
                    return Err(UpstreamError::ProtocolError(
                        "tools/list returned an invalid pagination cursor".to_string(),
                    ))
                }
            }
        }
    }

    /// Call a tool and retain its complete, unmodified MCP result.
    /// # Errors
    /// Returns transport and JSON-RPC errors without interpreting tool output.
    pub async fn call_tool(
        &self,
        server_alias: &str,
        tool_name: &str,
        arguments: JsonValue,
    ) -> Result<JsonValue> {
        self.call_tool_with_continuation(server_alias, tool_name, arguments, None, None)
            .await
    }

    /// Retry a tool call with request-scoped MRTR input and opaque server state.
    /// # Errors
    /// Returns transport and JSON-RPC errors without interpreting tool output.
    pub async fn call_tool_with_continuation(
        &self,
        server_alias: &str,
        tool_name: &str,
        arguments: JsonValue,
        input_responses: Option<JsonValue>,
        request_state: Option<JsonValue>,
    ) -> Result<JsonValue> {
        let config = self
            .get_server_config(server_alias)
            .await
            .ok_or_else(|| UpstreamError::ServerNotFound(server_alias.to_string()))?;
        let mut params = serde_json::Map::new();
        params.insert("name".to_string(), JsonValue::String(tool_name.to_string()));
        params.insert("arguments".to_string(), arguments);
        if let Some(input_responses) = input_responses {
            params.insert("inputResponses".to_string(), input_responses);
        }
        if let Some(request_state) = request_state {
            params.insert("requestState".to_string(), request_state);
        }
        self.send_request(&config, "tools/call", JsonValue::Object(params))
            .await
    }

    async fn send_request(
        &self,
        config: &UpstreamConfig,
        method: &str,
        params: JsonValue,
    ) -> Result<JsonValue> {
        match config.transport {
            Transport::Stdio => self.send_stdio_request(config, method, params).await,
            Transport::Http | Transport::HttpSse => {
                self.send_current_http_request(config, method, params).await
            }
            Transport::AgentSkillsRepo | Transport::AgentSkillsFs => {
                Err(UpstreamError::RequestFailed(
                    "Agent Skills transport does not support MCP requests".to_string(),
                ))
            }
        }
    }

    async fn send_stdio_request(
        &self,
        config: &UpstreamConfig,
        method: &str,
        params: JsonValue,
    ) -> Result<JsonValue> {
        let request_tx = self
            .sessions
            .read()
            .await
            .get(&config.alias)
            .and_then(|session| session.request_tx.clone())
            .ok_or_else(|| {
                UpstreamError::ConnectionFailed(format!(
                    "Upstream {} is disconnected",
                    config.alias
                ))
            })?;
        let id = JsonValue::String(Uuid::new_v4().to_string());
        let request = McpRequest {
            jsonrpc: "2.0".to_string(),
            id: id.clone(),
            method: method.to_string(),
            params: Some(current_request_params(params)),
        };
        let (sender, receiver) = oneshot::channel();
        let mut pending = PendingRequest {
            id,
            sender: request_tx.clone(),
            completed: false,
        };
        request_tx
            .send(StdioCommand::Request(request, sender))
            .map_err(|_| {
                UpstreamError::ConnectionFailed("Upstream request channel closed".to_string())
            })?;
        let result = with_timeout(self.request_timeout(config).await, method, async {
            receiver.await.map_err(|_| {
                UpstreamError::ConnectionFailed("Upstream response channel closed".to_string())
            })?
        })
        .await;
        pending.completed = !matches!(result, Err(UpstreamError::Timeout(_)));
        result
    }

    async fn send_current_http_request(
        &self,
        config: &UpstreamConfig,
        method: &str,
        params: JsonValue,
    ) -> Result<JsonValue> {
        let connection_id = self
            .sessions
            .read()
            .await
            .get(&config.alias)
            .map(|session| session.connection_id);
        let result = with_timeout(
            self.request_timeout(config).await,
            method,
            self.send_http_request(config, method, params),
        )
        .await;
        if matches!(
            result,
            Err(UpstreamError::ConnectionFailed(_)
                | UpstreamError::Timeout(_)
                | UpstreamError::ProtocolError(_))
        ) {
            self.mark_disconnected(&config.alias, connection_id).await;
        }
        result
    }

    async fn send_http_request(
        &self,
        config: &UpstreamConfig,
        method: &str,
        params: JsonValue,
    ) -> Result<JsonValue> {
        let url = config
            .url
            .as_deref()
            .ok_or_else(|| UpstreamError::ConnectionFailed("No URL configured".to_string()))?;
        let endpoint = mcp_http_endpoint(url);
        let endpoint = reqwest::Url::parse(&endpoint).map_err(|error| {
            UpstreamError::ConnectionFailed(format!("Invalid MCP endpoint: {error}"))
        })?;
        let token = http_auth_token(config.auth.as_ref(), &endpoint)?;
        let client = self.http_client.as_ref().map_err(|error| {
            UpstreamError::ConnectionFailed(format!("Cannot construct HTTP client: {error}"))
        })?;
        let mut request = client
            .post(endpoint)
            .header("Accept", "application/json, text/event-stream")
            .header("MCP-Protocol-Version", MCP_CURRENT_PROTOCOL_VERSION)
            .header("Mcp-Method", method);
        if method == "tools/call" {
            if let Some(name) = params.get("name").and_then(JsonValue::as_str) {
                request = request.header("Mcp-Name", name);
            }
        }
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        let payload = McpRequest {
            jsonrpc: "2.0".to_string(),
            id: JsonValue::String(Uuid::new_v4().to_string()),
            method: method.to_string(),
            params: Some(current_request_params(params)),
        };
        let mut response = request.json(&payload).send().await.map_err(|error| {
            UpstreamError::ConnectionFailed(format!("MCP {method} request failed: {error}"))
        })?;
        if !response.status().is_success() {
            return Err(UpstreamError::ConnectionFailed(format!(
                "MCP {method} returned {}",
                response.status()
            )));
        }
        let is_sse = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| {
                value
                    .split(';')
                    .next()
                    .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("text/event-stream"))
            });
        if is_sse {
            let mut decoder = SseDecoder::default();
            while let Some(chunk) = response.chunk().await.map_err(|error| {
                UpstreamError::ConnectionFailed(format!("Cannot read MCP event stream: {error}"))
            })? {
                if let Some(response) = decoder.feed(&chunk, &payload.id)? {
                    return response_result(response);
                }
            }
            return Err(UpstreamError::ConnectionFailed(
                "MCP event stream ended before the matching response".to_string(),
            ));
        }
        let body = response.bytes().await.map_err(|error| {
            UpstreamError::ConnectionFailed(format!("Cannot read MCP response: {error}"))
        })?;
        response_result(parse_mcp_http_response(&body, &payload.id)?)
    }

    /// Get upstream config for a server
    pub async fn get_server_config(&self, alias: &str) -> Option<UpstreamConfig> {
        let sessions = self.sessions.read().await;
        sessions.get(alias).map(|s| s.config.clone())
    }

    /// Get all connected servers
    pub async fn list_servers(&self) -> Vec<String> {
        let sessions = self.sessions.read().await;
        sessions.keys().cloned().collect()
    }

    /// Get connection state for a server
    pub async fn get_state(&self, alias: &str) -> Option<ConnectionState> {
        let sessions = self.sessions.read().await;
        sessions.get(alias).map(|s| s.state)
    }

    /// Return the live state and discovered tool count for every configured upstream.
    pub async fn status(&self) -> Vec<UpstreamStatus> {
        let sessions = self.sessions.read().await;
        sessions
            .values()
            .map(|session| UpstreamStatus {
                alias: session.config.alias.clone(),
                state: session.state,
                tool_count: session.tools.len(),
                required: session.config.required
                    || (self.unrestricted.load(Ordering::Acquire)
                        && matches!(session.config.alias.as_str(), "codiium" | "dociium")),
            })
            .collect()
    }

    pub async fn get_config(&self, alias: &str) -> Option<UpstreamConfig> {
        let sessions = self.sessions.read().await;
        sessions.get(alias).map(|s| s.config.clone())
    }

    /// # Errors
    /// Returns an error if the operation cannot be completed.
    pub async fn update_upstream(&self, alias: &str, updates: UpstreamConfig) -> Result<()> {
        info!("Updating upstream server: {}", alias);

        let (health, tool_count, tags, reconnect) = {
            let mut sessions = self.sessions.write().await;
            let session = sessions
                .get_mut(alias)
                .ok_or_else(|| UpstreamError::ServerNotFound(alias.to_string()))?;
            let mut reconnect = false;

            if let Some(desc) = updates.description {
                session.config.description = Some(desc);
            }
            if !updates.tags.is_empty() {
                session.config.tags = updates.tags;
            }
            if let Some(url) = updates.url {
                reconnect |= session.config.url.as_ref() != Some(&url);
                session.config.url = Some(url);
            }
            if let Some(command) = updates.command {
                reconnect |= session.config.command.as_ref() != Some(&command);
                session.config.command = Some(command);
            }
            if let Some(auth) = updates.auth {
                reconnect |= session.config.auth.as_ref().is_none_or(|current| {
                    current.auth_type != auth.auth_type
                        || current.env != auth.env
                        || current.token != auth.token
                });
                session.config.auth = Some(auth);
            }
            if let Some(sandbox_config) = updates.sandbox_config {
                reconnect |= session.config.sandbox_config.as_ref() != Some(&sandbox_config);
                session.config.sandbox_config = Some(sandbox_config);
            }

            let health = match session.state {
                ConnectionState::Connected | ConnectionState::Connecting => ServerHealth::Connected,
                ConnectionState::Degraded => ServerHealth::Degraded,
                ConnectionState::Disconnected | ConnectionState::Failed => ServerHealth::Down,
            };
            let values = (
                health,
                session.tools.len(),
                session.config.tags.clone(),
                reconnect,
            );
            drop(sessions);
            values
        };

        if reconnect {
            return self.connect(alias).await;
        }

        self.registry.update_server(ServerInfo {
            alias: alias.to_string(),
            health,
            tool_count,
            last_refresh: Timestamp::now(),
            tags,
        });

        info!("Updated upstream server: {}", alias);
        Ok(())
    }

    /// Disconnect from a server
    /// # Errors
    /// Returns an error if the operation cannot be completed.
    pub async fn disconnect(&self, alias: &str) -> Result<()> {
        self.mark_disconnected(alias, None).await;
        self.sessions.write().await.remove(alias);
        Ok(())
    }

    /// Reconnect to a server (for failure recovery)
    /// # Errors
    /// Returns an error if the operation cannot be completed.
    pub async fn reconnect(&self, alias: &str) -> Result<()> {
        warn!("Reconnecting to: {}", alias);

        // Mark as degraded
        self.registry.mark_server_degraded(alias);

        // Attempt reconnection
        match self.connect(alias).await {
            Ok(()) => {
                info!("Reconnected successfully: {}", alias);
                Ok(())
            }
            Err(e) => {
                error!("Reconnection failed for {}: {}", alias, e);
                self.registry.mark_server_down(alias);
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::indexing_slicing,
        clippy::unwrap_used,
        reason = "protocol fixture assertions should fail loudly"
    )]

    use super::*;
    use axum::{
        extract::Json,
        http::{HeaderMap, StatusCode},
        routing::post,
        Router,
    };

    async fn current_mcp_handler(
        headers: HeaderMap,
        Json(payload): Json<JsonValue>,
    ) -> (StatusCode, Json<JsonValue>) {
        let method = payload
            .get("method")
            .and_then(JsonValue::as_str)
            .unwrap_or_default();
        let valid = headers
            .get("mcp-protocol-version")
            .and_then(|value| value.to_str().ok())
            == Some(MCP_CURRENT_PROTOCOL_VERSION)
            && headers
                .get("mcp-method")
                .and_then(|value| value.to_str().ok())
                == Some(method)
            && payload
                .pointer("/params/_meta/io.modelcontextprotocol~1protocolVersion")
                .and_then(JsonValue::as_str)
                == Some(MCP_CURRENT_PROTOCOL_VERSION)
            && payload
                .pointer("/params/_meta/io.modelcontextprotocol~1clientCapabilities")
                .is_some()
            && payload
                .pointer("/params/_meta/io.modelcontextprotocol~1clientInfo/name")
                .and_then(JsonValue::as_str)
                == Some(MCP_CLIENT_NAME)
            && (method != "tools/call"
                || headers
                    .get("mcp-name")
                    .and_then(|value| value.to_str().ok())
                    == payload.pointer("/params/name").and_then(JsonValue::as_str));
        if !valid {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": payload.get("id").cloned().unwrap_or(JsonValue::Null),
                    "error": { "code": -32600, "message": "current MCP request required" }
                })),
            );
        }

        let result = match method {
            "server/discover" => serde_json::json!({
                "resultType": "complete",
                "supportedVersions": [MCP_CURRENT_PROTOCOL_VERSION],
                "capabilities": { "tools": {} },
                "ttlMs": 0,
                "cacheScope": "private"
            }),
            "tools/list" if payload.pointer("/params/cursor").is_some() => serde_json::json!({
                "resultType": "complete",
                "tools": [{ "name": "second", "inputSchema": { "type": "object" } }]
            }),
            "tools/list" => serde_json::json!({
                "resultType": "complete",
                "tools": [{ "name": "first", "inputSchema": { "type": "object" } }],
                "nextCursor": "second-page"
            }),
            "tools/call" => serde_json::json!({
                "resultType": "complete",
                "structuredContent": { "ok": true, "params": payload.get("params") }
            }),
            _ => serde_json::json!({ "resultType": "complete" }),
        };
        (
            StatusCode::OK,
            Json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": payload.get("id").cloned().unwrap_or(JsonValue::Null),
                "result": result
            })),
        )
    }

    #[tokio::test]
    async fn current_http_requests_include_protocol_metadata_and_standard_headers() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new().route("/mcp", post(current_mcp_handler)),
            )
            .await
            .unwrap();
        });

        let manager = UpstreamManager::new(Arc::new(Registry::new()));
        let config = UpstreamConfig {
            alias: "current".to_string(),
            required: false,
            transport: Transport::Http,
            command: None,
            url: Some(format!("http://{addr}")),
            auth: None,
            repo: None,
            git_ref: None,
            skills: None,
            roots: None,
            tags: vec![],
            sandbox_config: None,
            description: None,
        };

        manager.connect_http(&config).await.unwrap();
        let result = manager
            .send_current_http_request(
                &config,
                "tools/call",
                serde_json::json!({ "name": "test", "arguments": {} }),
            )
            .await
            .unwrap();
        assert_eq!(result["structuredContent"]["ok"], true);
    }

    #[test]
    fn current_http_helpers_normalize_endpoint_and_parse_sse() {
        assert_eq!(
            mcp_http_endpoint("https://example.test"),
            "https://example.test/mcp"
        );
        assert_eq!(
            mcp_http_endpoint("https://example.test/mcp/"),
            "https://example.test/mcp"
        );
        let response = SseDecoder::default()
            .feed(
                b"data: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"ok\":true}}\n\n",
                &JsonValue::from(1),
            )
            .unwrap()
            .unwrap();
        assert_eq!(response.result.unwrap()["ok"], true);
    }

    fn http_config(url: String) -> UpstreamConfig {
        UpstreamConfig {
            alias: "fixture".to_string(),
            required: true,
            transport: Transport::Http,
            command: None,
            url: Some(url),
            auth: None,
            repo: None,
            git_ref: None,
            skills: None,
            roots: None,
            tags: vec![],
            sandbox_config: None,
            description: None,
        }
    }

    async fn serve(router: Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        format!("http://{address}")
    }

    #[tokio::test]
    async fn http_pagination_and_continuation_preserve_request_scoped_input() {
        let url = serve(Router::new().route("/mcp", post(current_mcp_handler))).await;
        let registry = Arc::new(Registry::new());
        let manager = UpstreamManager::new(registry.clone());
        manager.add_upstream(http_config(url)).await.unwrap();
        assert_eq!(registry.get_by_server("fixture").len(), 2);
        assert_eq!(manager.status().await[0].tool_count, 2);
        let inputs = serde_json::json!({ "prompt": { "action": "accept", "content": {} } });
        let state = serde_json::json!("opaque+/=é");
        let response = manager
            .call_tool_with_continuation(
                "fixture",
                "test",
                serde_json::json!({}),
                Some(inputs.clone()),
                Some(state.clone()),
            )
            .await
            .unwrap();
        let params = &response["structuredContent"]["params"];
        assert_eq!(params["inputResponses"], inputs);
        assert_eq!(params["requestState"], state);
    }

    async fn streaming_mcp_handler(Json(payload): Json<JsonValue>) -> axum::response::Response {
        use futures::StreamExt;

        let result =
            serde_json::json!({ "resultType": "complete", "structuredContent": { "text": "été" } });
        let response =
            serde_json::json!({ "jsonrpc": "2.0", "id": payload["id"], "result": result })
                .to_string();
        let (first, rest) = response.split_once(',').unwrap();
        let events = format!(
            "\u{feff}: keepalive\r\n\r\n\
             data: {{\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\",\"params\":{{}}}}\r\n\r\n\
             data: {{\"jsonrpc\":\"2.0\",\"id\":\"unrelated\",\"result\":{{\"wrong\":true}}}}\r\n\r\n\
             data: {first},\r\ndata: {rest}\r\n\r\n"
        );
        let mut remaining = bytes::Bytes::from(events);
        let chunks = std::iter::from_fn(move || {
            if remaining.is_empty() {
                None
            } else {
                Some(remaining.split_to(remaining.len().min(7)))
            }
        });
        let stream = futures::stream::iter(chunks.map(Ok::<_, std::io::Error>))
            .chain(futures::stream::pending());
        axum::response::Response::builder()
            .header("content-type", "text/event-stream; charset=utf-8")
            .body(axum::body::Body::from_stream(stream))
            .unwrap()
    }

    #[tokio::test]
    async fn http_sse_assembles_multiline_events_and_returns_matching_response_before_eof() {
        let url = serve(Router::new().route("/mcp", post(streaming_mcp_handler))).await;
        let manager = UpstreamManager::new(Arc::new(Registry::new()));
        let response = tokio::time::timeout(
            Duration::from_secs(2),
            manager.send_current_http_request(
                &http_config(url),
                "tools/call",
                serde_json::json!({ "name": "test", "arguments": {} }),
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(response["structuredContent"]["text"], "été");
    }

    #[tokio::test]
    async fn configured_authentication_never_falls_back_to_anonymous_or_redirected_requests() {
        let url = serve(Router::new().route("/mcp", post(current_mcp_handler))).await;
        let manager = UpstreamManager::new(Arc::new(Registry::new()));
        for auth in [
            AuthConfig {
                auth_type: "bearer".to_string(),
                env: None,
                token: None,
            },
            AuthConfig {
                auth_type: "bearer".to_string(),
                env: None,
                token: Some(String::new()),
            },
            AuthConfig {
                auth_type: "header".to_string(),
                env: None,
                token: Some("secret".to_string()),
            },
            AuthConfig {
                auth_type: "bearer".to_string(),
                env: Some(format!("SKILLS_MISSING_{}", Uuid::new_v4().simple())),
                token: None,
            },
        ] {
            let mut config = http_config(url.clone());
            config.auth = Some(auth);
            assert!(manager.connect_http(&config).await.is_err());
        }
        let mut config = http_config("http://example.invalid".to_string());
        config.auth = Some(AuthConfig {
            auth_type: "bearer".to_string(),
            env: None,
            token: Some("secret".to_string()),
        });
        let error = manager.connect_http(&config).await.unwrap_err();
        assert!(error.to_string().contains("Refusing"));

        let url = serve(
            Router::new()
                .route(
                    "/mcp",
                    post(|| async {
                        (
                            StatusCode::TEMPORARY_REDIRECT,
                            [(axum::http::header::LOCATION, "/target")],
                        )
                    }),
                )
                .route("/target", post(current_mcp_handler)),
        )
        .await;
        assert!(manager.connect_http(&http_config(url)).await.is_err());
    }

    #[tokio::test]
    async fn standard_http_deadline_marks_a_hung_dependency_disconnected() {
        let url = serve(Router::new().route(
            "/mcp",
            post(|| async { futures::future::pending::<StatusCode>().await }),
        ))
        .await;
        let manager = UpstreamManager::new(Arc::new(Registry::new()));
        let sandbox = SandboxConfig {
            timeout_ms: 40,
            ..SandboxConfig::default()
        };
        manager.set_sandbox_config(sandbox).await;
        assert!(matches!(
            manager.add_upstream(http_config(url)).await,
            Err(UpstreamError::Timeout(_))
        ));
        let status = manager.status().await;
        assert_eq!(status[0].state, ConnectionState::Disconnected);
        assert_eq!(status[0].tool_count, 0);
    }

    #[cfg(unix)]
    const STDIO_FIXTURE: &str = r#"
import json, os, sys, threading, time
lock = threading.Lock()
seen = set()
active = set()
cancelled = []
loop_pages = sys.argv[-1] == "loop"
def send(value):
    with lock:
        print(json.dumps(value, ensure_ascii=False), flush=True)
def respond(request, result):
    send({"jsonrpc":"2.0", "id":request["id"], "result":result})
def echo(request):
    time.sleep(request["params"].get("arguments", {}).get("delay", 0))
    respond(request, {"resultType":"complete", "structuredContent":{
        "arguments":request["params"].get("arguments"), "id":request["id"]}})
for line in sys.stdin:
    request = json.loads(line)
    method = request["method"]
    if method == "notifications/cancelled":
        assert "id" not in request
        identifier = request["params"]["requestId"]
        active.discard(identifier)
        cancelled.append(identifier)
        continue
    assert method not in ("initialize", "notifications/initialized")
    assert request["id"] not in seen
    seen.add(request["id"])
    meta = request["params"]["_meta"]
    assert meta["io.modelcontextprotocol/protocolVersion"] == "2026-07-28"
    assert meta["io.modelcontextprotocol/clientInfo"]["name"] == "skills.rs"
    assert "io.modelcontextprotocol/clientCapabilities" in meta
    send({"jsonrpc":"2.0", "method":"notifications/progress", "params":{}})
    if method == "server/discover":
        respond(request, {"resultType":"complete", "supportedVersions":["2026-07-28"], "capabilities":{"tools":{}}})
    elif method == "tools/list":
        cursor = request["params"].get("cursor")
        result = {"resultType":"complete", "tools":[{"name":"second" if cursor else "first", "inputSchema":{"type":"object"}}]}
        if not cursor or loop_pages:
            result["nextCursor"] = "next"
        respond(request, result)
    elif method == "tools/call":
        name = request["params"]["name"]
        if name == "exit":
            os._exit(0)
        elif name == "hold":
            active.add(request["id"])
        elif name == "inspect":
            respond(request, {"resultType":"complete", "structuredContent":{"active":len(active), "cancelled":len(cancelled)}})
        elif name == "home":
            respond(request, {"resultType":"complete", "structuredContent":{"home":os.environ.get("HOME")}})
        elif name == "mrtr":
            params = request["params"]
            if "requestState" not in params:
                respond(request, {"resultType":"input_required", "requestState":"opaque+/=é", "inputRequests":{}})
            else:
                respond(request, {"resultType":"complete", "content":[{"type":"image", "mimeType":"image/png", "data":"AQID"}], "structuredContent":params})
        else:
            threading.Thread(target=echo, args=(request,), daemon=True).start()
"#;

    #[cfg(unix)]
    fn stdio_config(mode: &str) -> (tempfile::TempDir, UpstreamConfig) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("server.py");
        std::fs::write(&path, STDIO_FIXTURE).unwrap();
        let mut config = http_config(String::new());
        config.transport = Transport::Stdio;
        config.url = None;
        config.command = Some(vec![
            "python3".to_string(),
            "-u".to_string(),
            path.to_string_lossy().into_owned(),
            mode.to_string(),
        ]);
        (directory, config)
    }

    #[cfg(unix)]
    fn native_config(timeout_ms: u64) -> SandboxConfig {
        SandboxConfig {
            backend: SandboxBackend::Timeout,
            timeout_ms,
            allow_network: true,
            max_memory_bytes: 0,
            max_cpu_seconds: 0,
            ..SandboxConfig::default()
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn current_stdio_paginates_and_isolates_concurrent_calls_and_mrtr_retries() {
        let (_directory, config) = stdio_config("normal");
        let registry = Arc::new(Registry::new());
        let manager = Arc::new(UpstreamManager::new(registry.clone()));
        manager.set_sandbox_config(native_config(3_000)).await;
        manager.add_upstream(config).await.unwrap();
        assert_eq!(registry.get_by_server("fixture").len(), 2);
        let calls = (0..24).map(|index| {
            let manager = manager.clone();
            async move {
                let arguments = serde_json::json!({ "index": index, "delay": if index % 2 == 0 { 0.04 } else { 0.0 } });
                let response = manager.call_tool("fixture", "echo", arguments.clone()).await.unwrap();
                assert_eq!(response["structuredContent"]["arguments"], arguments);
                response["structuredContent"]["id"].clone()
            }
        });
        let identifiers: HashSet<_> = futures::future::join_all(calls).await.into_iter().collect();
        assert_eq!(identifiers.len(), 24);
        let initial = manager
            .call_tool("fixture", "mrtr", serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(initial["resultType"], "input_required");
        let inputs = serde_json::json!({});
        let completed = manager
            .call_tool_with_continuation(
                "fixture",
                "mrtr",
                serde_json::json!({}),
                Some(inputs.clone()),
                Some(initial["requestState"].clone()),
            )
            .await
            .unwrap();
        assert_eq!(
            completed["structuredContent"]["requestState"],
            initial["requestState"]
        );
        assert_eq!(completed["structuredContent"]["inputResponses"], inputs);
        assert_eq!(completed["content"][0]["data"], "AQID");
        if let Ok(home) = std::env::var("HOME") {
            let response = manager
                .call_tool("fixture", "home", serde_json::json!({}))
                .await
                .unwrap();
            assert_ne!(response["structuredContent"]["home"], home);
        }
        manager.disconnect("fixture").await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stdio_timeout_and_task_cancellation_send_idless_notifications_and_release_pending_calls(
    ) {
        let (_directory, config) = stdio_config("normal");
        let manager = Arc::new(UpstreamManager::new(Arc::new(Registry::new())));
        manager.set_sandbox_config(native_config(3_000)).await;
        manager.add_upstream(config).await.unwrap();
        manager.set_sandbox_config(native_config(100)).await;
        assert!(matches!(
            manager
                .call_tool("fixture", "hold", serde_json::json!({}))
                .await,
            Err(UpstreamError::Timeout(_))
        ));
        let held_manager = manager.clone();
        let held = tokio::spawn(async move {
            held_manager
                .call_tool("fixture", "hold", serde_json::json!({}))
                .await
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let status = manager
                    .call_tool("fixture", "inspect", serde_json::json!({}))
                    .await
                    .unwrap();
                if status["structuredContent"]["active"] == 1 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        held.abort();
        let _ = held.await;
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let status = manager
                    .call_tool("fixture", "inspect", serde_json::json!({}))
                    .await
                    .unwrap();
                if status["structuredContent"]["active"] == 0
                    && status["structuredContent"]["cancelled"] == 2
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            manager.get_state("fixture").await,
            Some(ConnectionState::Connected)
        );
        manager.disconnect("fixture").await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stdio_eof_clears_dependency_readiness_and_registered_tools() {
        let (_directory, config) = stdio_config("normal");
        let registry = Arc::new(Registry::new());
        let manager = UpstreamManager::new(registry.clone());
        manager.set_sandbox_config(native_config(3_000)).await;
        manager.add_upstream(config).await.unwrap();
        assert!(matches!(
            manager
                .call_tool("fixture", "exit", serde_json::json!({}))
                .await,
            Err(UpstreamError::ConnectionFailed(_))
        ));
        assert_eq!(
            manager.get_state("fixture").await,
            Some(ConnectionState::Disconnected)
        );
        assert_eq!(manager.status().await[0].tool_count, 0);
        assert!(registry.get_by_server("fixture").is_empty());
        assert_eq!(
            registry.get_server("fixture").unwrap().health,
            ServerHealth::Down
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stdio_repeated_pagination_cursor_fails_instead_of_looping() {
        let (_directory, config) = stdio_config("loop");
        let manager = UpstreamManager::new(Arc::new(Registry::new()));
        manager.set_sandbox_config(native_config(3_000)).await;
        assert!(matches!(
            manager.add_upstream(config).await,
            Err(UpstreamError::ProtocolError(_))
        ));
        assert_eq!(
            manager.get_state("fixture").await,
            Some(ConnectionState::Disconnected)
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unsupported_upstream_containment_fails_before_spawning_the_command() {
        let directory = tempfile::tempdir().unwrap();
        let marker = directory.path().join("spawned");
        let mut config = http_config(String::new());
        config.transport = Transport::Stdio;
        config.command = Some(vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            "printf spawned > \"$1\"".to_string(),
            "fixture".to_string(),
            marker.to_string_lossy().into_owned(),
        ]);
        let manager = UpstreamManager::new(Arc::new(Registry::new()));
        for backend in [
            SandboxBackend::Wasm,
            SandboxBackend::Docker,
            SandboxBackend::None,
            SandboxBackend::Timeout,
        ] {
            config.sandbox_config = Some(SandboxConfigOverride {
                backend: Some(backend),
                allow_network: Some(false),
                ..SandboxConfigOverride::default()
            });
            assert!(manager.add_upstream(config.clone()).await.is_err());
            assert!(!marker.exists());
            assert_eq!(
                manager.get_state("fixture").await,
                Some(ConnectionState::Disconnected)
            );
        }
    }

    #[tokio::test]
    async fn http_connection_failure_clears_discovered_tools_and_readiness() {
        let url = serve(Router::new().route(
            "/mcp",
            post(
                |headers: HeaderMap, Json(payload): Json<JsonValue>| async move {
                    if payload["method"] == "tools/call" {
                        (StatusCode::SERVICE_UNAVAILABLE, Json(serde_json::json!({})))
                    } else {
                        current_mcp_handler(headers, Json(payload)).await
                    }
                },
            ),
        ))
        .await;
        let registry = Arc::new(Registry::new());
        let manager = UpstreamManager::new(registry.clone());
        manager.add_upstream(http_config(url)).await.unwrap();
        assert!(manager
            .call_tool("fixture", "test", serde_json::json!({}))
            .await
            .is_err());
        assert_eq!(
            manager.get_state("fixture").await,
            Some(ConnectionState::Disconnected)
        );
        assert!(registry.get_by_server("fixture").is_empty());
    }

    #[tokio::test]
    async fn http_authentication_updates_reconnect_and_fail_closed() {
        let url = serve(Router::new().route("/mcp", post(current_mcp_handler))).await;
        let manager = UpstreamManager::new(Arc::new(Registry::new()));
        manager.add_upstream(http_config(url)).await.unwrap();
        let mut updates = manager.get_config("fixture").await.unwrap();
        updates.auth = Some(AuthConfig {
            auth_type: "bearer".to_string(),
            env: None,
            token: Some(String::new()),
        });
        assert!(manager.update_upstream("fixture", updates).await.is_err());
        assert_eq!(
            manager.get_state("fixture").await,
            Some(ConnectionState::Disconnected)
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn open_stdio_uses_explicit_trusted_environment_and_unbounded_policy() {
        let (_directory, config) = stdio_config("normal");
        let manager = UpstreamManager::new(Arc::new(Registry::new()));
        manager.set_sandbox_config(native_config(25)).await;
        manager.set_unrestricted();
        manager.add_upstream(config).await.unwrap();
        let response = tokio::time::timeout(
            Duration::from_secs(2),
            manager.call_tool("fixture", "echo", serde_json::json!({ "delay": 0.15 })),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(response["structuredContent"]["arguments"]["delay"], 0.15);
        let response = manager
            .call_tool("fixture", "home", serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(
            response["structuredContent"]["home"],
            serde_json::to_value(std::env::var("HOME").ok()).unwrap()
        );
        manager.disconnect("fixture").await.unwrap();
    }
}
