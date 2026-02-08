//! Upstream MCP Server Manager
//!
//! Manages connections to upstream MCP servers in multiple transports:
//! - stdio: subprocess communication
//! - HTTP: Streamable HTTP and legacy HTTP+SSE
//!
//! Handles lifecycle, health monitoring, and automatic reconnection.

use crate::core::{
    CallableId, CallableKind, CallableRecord, CostHints, RiskTier, SchemaDigest, ToolDefinition,
};
use crate::core::registry::{Registry, ServerHealth, ServerInfo};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::sync::Arc;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot, RwLock};
use tracing::{debug, error, info, warn};

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
    pub transport: Transport,

    // For stdio
    pub command: Option<Vec<String>>,

    // For HTTP
    pub url: Option<String>,
    pub auth: Option<AuthConfig>,

    // For Agent Skills Repo
    /// Git repository URL (e.g., "https://github.com/owner/repo" or "owner/repo")
    pub repo: Option<String>,
    /// Git ref (branch, tag, or commit SHA)
    pub git_ref: Option<String>,
    /// Specific skill names to import (if omitted, imports all)
    pub skills: Option<Vec<String>>,

    // For Agent Skills FS
    /// Local filesystem roots to scan for Agent Skills
    pub roots: Option<Vec<String>>,

    pub tags: Vec<String>,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
struct McpResponse {
    jsonrpc: String,
    id: JsonValue,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<JsonValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonValue>,
}

/// Upstream connection state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Degraded,
    Failed,
}

/// Request tracker for correlating responses
type ResponseSender = oneshot::Sender<Result<JsonValue>>;

/// Upstream session
struct UpstreamSession {
    config: UpstreamConfig,
    state: ConnectionState,
    last_ping: Option<chrono::DateTime<chrono::Utc>>,
    tools: Vec<CallableId>,
    // For stdio: child process and request sender
    process: Option<Child>,
    request_tx: Option<mpsc::UnboundedSender<(JsonValue, McpRequest, ResponseSender)>>,
}

/// Upstream manager
pub struct UpstreamManager {
    sessions: Arc<RwLock<HashMap<String, UpstreamSession>>>,
    registry: Arc<Registry>,
    http_client: reqwest::Client,
}

impl UpstreamManager {
    pub fn new(registry: Arc<Registry>) -> Self {
        UpstreamManager {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            registry,
            http_client: reqwest::Client::new(),
        }
    }

    /// Add an upstream server
    pub async fn add_upstream(&self, config: UpstreamConfig) -> Result<()> {
        let alias = config.alias.clone();
        info!("Adding upstream server: {}", alias);

        let session = UpstreamSession {
            config: config.clone(),
            state: ConnectionState::Disconnected,
            last_ping: None,
            tools: Vec::new(),
            process: None,
            request_tx: None,
        };

        self.sessions.write().await.insert(alias.clone(), session);

        // Connect and fetch tools
        self.connect(&alias).await?;

        Ok(())
    }

    /// Connect to an upstream server
    pub async fn connect(&self, alias: &str) -> Result<()> {
        info!("Connecting to upstream: {}", alias);

        let config = {
            let sessions = self.sessions.read().await;
            sessions
                .get(alias)
                .ok_or_else(|| UpstreamError::ServerNotFound(alias.to_string()))?
                .config
                .clone()
        };

        // Update state to connecting
        {
            let mut sessions = self.sessions.write().await;
            if let Some(session) = sessions.get_mut(alias) {
                session.state = ConnectionState::Connecting;
            }
        }

        // Initialize connection based on transport
        match config.transport {
            Transport::Stdio => self.connect_stdio(&config).await?,
            Transport::Http => self.connect_http(&config).await?,
            Transport::HttpSse => self.connect_http_sse(&config).await?,
            Transport::AgentSkillsRepo => {
                // Agent Skills Repo: skills are added via CLI 'skills add' command
                // This transport type is for configuration only, no connection needed
                info!("Agent Skills Repo transport - use 'skills add' to import skills");
            }
            Transport::AgentSkillsFs => {
                // Agent Skills FS: skills are indexed from filesystem
                // No connection needed, skills are loaded directly by SkillStore
                info!("Agent Skills FS transport - skills loaded from filesystem");
            }
        }

        // Fetch tools
        self.refresh_tools(alias).await?;

        // Update state to connected
        {
            let mut sessions = self.sessions.write().await;
            if let Some(session) = sessions.get_mut(alias) {
                session.state = ConnectionState::Connected;
                session.last_ping = Some(chrono::Utc::now());
            }
        }

        // Update registry
        self.registry.update_server(ServerInfo {
            alias: alias.to_string(),
            health: ServerHealth::Connected,
            tool_count: 0, // Will be updated by refresh_tools
            last_refresh: chrono::Utc::now(),
            tags: config.tags.clone(),
        });

        info!("Connected to upstream: {}", alias);
        Ok(())
    }

    /// Connect via stdio (subprocess)
    async fn connect_stdio(&self, config: &UpstreamConfig) -> Result<()> {
        let command = config
            .command
            .as_ref()
            .ok_or_else(|| UpstreamError::ConnectionFailed("No command specified".to_string()))?;

        if command.is_empty() {
            return Err(UpstreamError::ConnectionFailed(
                "Command list is empty".to_string(),
            ));
        }

        debug!("Spawning stdio process: {:?}", command);

        // Spawn subprocess with stdio piped
        let mut child = Command::new(&command[0])
            .args(&command[1..])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| UpstreamError::ConnectionFailed(format!("Failed to spawn: {}", e)))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| UpstreamError::ConnectionFailed("Failed to get stdin".to_string()))?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| UpstreamError::ConnectionFailed("Failed to get stdout".to_string()))?;

        // Channel for sending requests
        let (request_tx, mut request_rx) =
            mpsc::unbounded_channel::<(JsonValue, McpRequest, ResponseSender)>();

        // Channel for responses from stdout reader
        let (response_tx, mut response_rx) = mpsc::unbounded_channel::<McpResponse>();

        // Spawn stdout reader task
        let alias = config.alias.clone();
        tokio::spawn(async move {
            let reader = BufReader::new(stdout);
            let mut lines = reader.lines();

            while let Ok(Some(line)) = lines.next_line().await {
                if line.trim().is_empty() {
                    continue;
                }

                match serde_json::from_str::<McpResponse>(&line) {
                    Ok(response) => {
                        if response_tx.send(response).is_err() {
                            debug!("Response channel closed for {}", alias);
                            break;
                        }
                    }
                    Err(e) => {
                        warn!("Failed to parse response from {}: {}", alias, e);
                    }
                }
            }
            debug!("Stdout reader finished for {}", alias);
        });

        // Spawn request writer and response router task
        let alias = config.alias.clone();
        tokio::spawn(async move {
            let mut stdin = stdin;
            let mut pending_requests: HashMap<JsonValue, ResponseSender> = HashMap::new();

            loop {
                tokio::select! {
                    Some((id, request, response_tx)) = request_rx.recv() => {
                        // Send request to stdin
                        let json = match serde_json::to_string(&request) {
                            Ok(j) => j,
                            Err(e) => {
                                let _ = response_tx.send(Err(UpstreamError::ProtocolError(
                                    format!("Failed to serialize request: {}", e)
                                )));
                                continue;
                            }
                        };

                        if let Err(e) = stdin.write_all(json.as_bytes()).await {
                            let _ = response_tx.send(Err(UpstreamError::ConnectionFailed(
                                format!("Failed to write to stdin: {}", e)
                            )));
                            break;
                        }

                        if let Err(e) = stdin.write_all(b"\n").await {
                            let _ = response_tx.send(Err(UpstreamError::ConnectionFailed(
                                format!("Failed to write newline: {}", e)
                            )));
                            break;
                        }

                        if let Err(e) = stdin.flush().await {
                            let _ = response_tx.send(Err(UpstreamError::ConnectionFailed(
                                format!("Failed to flush: {}", e)
                            )));
                            break;
                        }

                        // Track pending request
                        pending_requests.insert(id, response_tx);
                    }
                    Some(response) = response_rx.recv() => {
                        // Route response to waiting request
                        if let Some(sender) = pending_requests.remove(&response.id) {
                            if let Some(error) = response.error {
                                let _ = sender.send(Err(UpstreamError::RequestFailed(
                                    error.to_string()
                                )));
                            } else if let Some(result) = response.result {
                                let _ = sender.send(Ok(result));
                            } else {
                                let _ = sender.send(Err(UpstreamError::ProtocolError(
                                    "Response has no result or error".to_string()
                                )));
                            }
                        }
                    }
                    else => break,
                }
            }
            debug!("Request handler finished for {}", alias);
        });

        // Send initialize request
        let init_request = McpRequest {
            jsonrpc: "2.0".to_string(),
            id: JsonValue::from(1),
            method: "initialize".to_string(),
            params: Some(serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "tools": {}
                },
                "clientInfo": {
                    "name": "skills.rs",
                    "version": "0.1.0"
                }
            })),
        };

        let (response_tx, response_rx) = oneshot::channel();
        request_tx
            .send((JsonValue::from(1), init_request, response_tx))
            .map_err(|_| {
                UpstreamError::ConnectionFailed("Failed to send initialize request".to_string())
            })?;

        // Wait for initialize response
        let _init_response = tokio::time::timeout(std::time::Duration::from_secs(10), response_rx)
            .await
            .map_err(|_| UpstreamError::Timeout("Initialize request timed out".to_string()))?
            .map_err(|_| {
                UpstreamError::ConnectionFailed("Initialize response channel closed".to_string())
            })??;

        debug!("MCP initialize succeeded for {}", config.alias);

        // Send initialized notification
        let initialized_notif = McpRequest {
            jsonrpc: "2.0".to_string(),
            id: JsonValue::Null,
            method: "notifications/initialized".to_string(),
            params: None,
        };

        let (notif_tx, _notif_rx) = oneshot::channel();
        request_tx
            .send((JsonValue::Null, initialized_notif, notif_tx))
            .map_err(|_| {
                UpstreamError::ConnectionFailed(
                    "Failed to send initialized notification".to_string(),
                )
            })?;

        // Store session data
        let alias = config.alias.clone();
        let mut sessions = self.sessions.write().await;
        if let Some(session) = sessions.get_mut(&alias) {
            session.process = Some(child);
            session.request_tx = Some(request_tx);
        }

        Ok(())
    }

    /// Connect via HTTP
    async fn connect_http(&self, config: &UpstreamConfig) -> Result<()> {
        let url = config
            .url
            .as_ref()
            .ok_or_else(|| UpstreamError::ConnectionFailed("No URL specified".to_string()))?;

        debug!("Connecting to HTTP endpoint: {}", url);

        // In full implementation:
        // - POST to /mcp/initialize
        // - Handle session tokens
        // - Store connection state

        // Mock for now
        Ok(())
    }

    /// Connect via HTTP+SSE (legacy)
    async fn connect_http_sse(&self, config: &UpstreamConfig) -> Result<()> {
        let url = config
            .url
            .as_ref()
            .ok_or_else(|| UpstreamError::ConnectionFailed("No URL specified".to_string()))?;

        debug!("Connecting to HTTP+SSE endpoint: {}", url);

        // In full implementation:
        // - POST to initialize endpoint
        // - Open SSE connection for notifications
        // - Handle bidirectional communication

        // Mock for now
        Ok(())
    }

    /// Refresh tools from an upstream server
    pub async fn refresh_tools(&self, alias: &str) -> Result<()> {
        info!("Refreshing tools from: {}", alias);

        let config = {
            let sessions = self.sessions.read().await;
            sessions
                .get(alias)
                .ok_or_else(|| UpstreamError::ServerNotFound(alias.to_string()))?
                .config
                .clone()
        };

        // Fetch tools based on transport
        let tools = match config.transport {
            Transport::Stdio => self.list_tools_stdio(alias).await?,
            Transport::Http | Transport::HttpSse => self.list_tools_http(alias).await?,
            Transport::AgentSkillsRepo | Transport::AgentSkillsFs => {
                // Agent Skills don't provide MCP tools, they provide skills
                // Skills are discovered by SkillStore directly
                debug!("Agent Skills transport - no MCP tools to refresh");
                Vec::new()
            }
        };

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
                last_seen: chrono::Utc::now(),
            };

            self.registry
                .register(record)
                .map_err(|e| UpstreamError::ProtocolError(e.to_string()))?;

            tool_ids.push(id);
        }

        // Update session
        {
            let mut sessions = self.sessions.write().await;
            if let Some(session) = sessions.get_mut(alias) {
                session.tools = tool_ids.clone();
            }
        }

        // Update server info
        self.registry.update_server(ServerInfo {
            alias: alias.to_string(),
            health: ServerHealth::Connected,
            tool_count: tool_ids.len(),
            last_refresh: chrono::Utc::now(),
            tags: vec![],
        });

        info!("Refreshed {} tools from {}", tool_ids.len(), alias);
        Ok(())
    }

    /// List tools via stdio
    async fn list_tools_stdio(&self, alias: &str) -> Result<Vec<ToolDefinition>> {
        let request_tx = {
            let sessions = self.sessions.read().await;
            sessions
                .get(alias)
                .and_then(|s| s.request_tx.clone())
                .ok_or_else(|| UpstreamError::ServerNotFound(alias.to_string()))?
        };

        let request_id = JsonValue::from(format!("list_tools_{}", chrono::Utc::now().timestamp()));
        let request = McpRequest {
            jsonrpc: "2.0".to_string(),
            id: request_id.clone(),
            method: "tools/list".to_string(),
            params: None,
        };

        let (response_tx, response_rx) = oneshot::channel();
        request_tx
            .send((request_id, request, response_tx))
            .map_err(|_| UpstreamError::ConnectionFailed("Failed to send request".to_string()))?;

        let response = tokio::time::timeout(std::time::Duration::from_secs(30), response_rx)
            .await
            .map_err(|_| UpstreamError::Timeout("tools/list request timed out".to_string()))?
            .map_err(|_| {
                UpstreamError::ConnectionFailed("Response channel closed".to_string())
            })??;

        // Parse tools from response
        let tools_array = response
            .get("tools")
            .and_then(|v| v.as_array())
            .ok_or_else(|| {
                UpstreamError::ProtocolError("Invalid tools/list response".to_string())
            })?;

        let mut tools = Vec::new();
        for tool_value in tools_array {
            let name = tool_value
                .get("name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| UpstreamError::ProtocolError("Tool missing name".to_string()))?
                .to_string();

            let description = tool_value
                .get("description")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            let input_schema = tool_value
                .get("inputSchema")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({"type": "object"}));

            tools.push(ToolDefinition {
                name,
                description,
                input_schema,
            });
        }

        Ok(tools)
    }

    /// List tools via HTTP
    async fn list_tools_http(&self, alias: &str) -> Result<Vec<ToolDefinition>> {
        let config = {
            let sessions = self.sessions.read().await;
            sessions
                .get(alias)
                .ok_or_else(|| UpstreamError::ServerNotFound(alias.to_string()))?
                .config
                .clone()
        };

        let url = config
            .url
            .as_ref()
            .ok_or_else(|| UpstreamError::ConnectionFailed("No URL configured".to_string()))?;

        let endpoint = format!("{}/mcp/tools/list", url.trim_end_matches('/'));

        let mut request = self.http_client.post(&endpoint);

        // Add auth if configured
        if let Some(auth) = &config.auth {
            if let Some(token) = &auth.token {
                request = request.bearer_auth(token);
            } else if let Some(env_var) = &auth.env {
                if let Ok(token) = std::env::var(env_var) {
                    request = request.bearer_auth(token);
                }
            }
        }

        let response = request
            .json(&serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/list"
            }))
            .send()
            .await
            .map_err(|e| UpstreamError::RequestFailed(format!("HTTP request failed: {}", e)))?;

        if !response.status().is_success() {
            return Err(UpstreamError::RequestFailed(format!(
                "HTTP {} response",
                response.status()
            )));
        }

        let response_json: McpResponse = response.json().await.map_err(|e| {
            UpstreamError::ProtocolError(format!("Failed to parse response: {}", e))
        })?;

        if let Some(error) = response_json.error {
            return Err(UpstreamError::RequestFailed(format!(
                "Server error: {}",
                error
            )));
        }

        let result = response_json
            .result
            .ok_or_else(|| UpstreamError::ProtocolError("No result in response".to_string()))?;

        let tools_array = result
            .get("tools")
            .and_then(|v| v.as_array())
            .ok_or_else(|| {
                UpstreamError::ProtocolError("Invalid tools/list response".to_string())
            })?;

        let mut tools = Vec::new();
        for tool_value in tools_array {
            let name = tool_value
                .get("name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| UpstreamError::ProtocolError("Tool missing name".to_string()))?
                .to_string();

            let description = tool_value
                .get("description")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            let input_schema = tool_value
                .get("inputSchema")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({"type": "object"}));

            tools.push(ToolDefinition {
                name,
                description,
                input_schema,
            });
        }

        Ok(tools)
    }

    /// Call a tool on an upstream server
    pub async fn call_tool(
        &self,
        server_alias: &str,
        tool_name: &str,
        arguments: JsonValue,
    ) -> Result<JsonValue> {
        let config = {
            let sessions = self.sessions.read().await;
            sessions
                .get(server_alias)
                .ok_or_else(|| UpstreamError::ServerNotFound(server_alias.to_string()))?
                .config
                .clone()
        };

        match config.transport {
            Transport::Stdio => {
                self.call_tool_stdio(server_alias, tool_name, arguments)
                    .await
            }
            Transport::Http | Transport::HttpSse => {
                self.call_tool_http(server_alias, tool_name, arguments)
                    .await
            }
            Transport::AgentSkillsRepo | Transport::AgentSkillsFs => {
                Err(UpstreamError::RequestFailed(
                    "Agent Skills transport does not support tool calls".to_string(),
                ))
            }
        }
    }

    /// Call tool via stdio
    async fn call_tool_stdio(
        &self,
        alias: &str,
        tool_name: &str,
        arguments: JsonValue,
    ) -> Result<JsonValue> {
        let request_tx = {
            let sessions = self.sessions.read().await;
            sessions
                .get(alias)
                .and_then(|s| s.request_tx.clone())
                .ok_or_else(|| UpstreamError::ServerNotFound(alias.to_string()))?
        };

        let request_id = JsonValue::from(format!("call_tool_{}", chrono::Utc::now().timestamp()));
        let request = McpRequest {
            jsonrpc: "2.0".to_string(),
            id: request_id.clone(),
            method: "tools/call".to_string(),
            params: Some(serde_json::json!({
                "name": tool_name,
                "arguments": arguments
            })),
        };

        let (response_tx, response_rx) = oneshot::channel();
        request_tx
            .send((request_id, request, response_tx))
            .map_err(|_| UpstreamError::ConnectionFailed("Failed to send request".to_string()))?;

        let response = tokio::time::timeout(std::time::Duration::from_secs(60), response_rx)
            .await
            .map_err(|_| UpstreamError::Timeout("tools/call request timed out".to_string()))?
            .map_err(|_| {
                UpstreamError::ConnectionFailed("Response channel closed".to_string())
            })??;

        Ok(response)
    }

    /// Call tool via HTTP
    async fn call_tool_http(
        &self,
        alias: &str,
        tool_name: &str,
        arguments: JsonValue,
    ) -> Result<JsonValue> {
        let config = {
            let sessions = self.sessions.read().await;
            sessions
                .get(alias)
                .ok_or_else(|| UpstreamError::ServerNotFound(alias.to_string()))?
                .config
                .clone()
        };

        let url = config
            .url
            .as_ref()
            .ok_or_else(|| UpstreamError::ConnectionFailed("No URL configured".to_string()))?;

        let endpoint = format!("{}/mcp/tools/call", url.trim_end_matches('/'));

        let mut request = self.http_client.post(&endpoint);

        // Add auth if configured
        if let Some(auth) = &config.auth {
            if let Some(token) = &auth.token {
                request = request.bearer_auth(token);
            } else if let Some(env_var) = &auth.env {
                if let Ok(token) = std::env::var(env_var) {
                    request = request.bearer_auth(token);
                }
            }
        }

        let response = request
            .json(&serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {
                    "name": tool_name,
                    "arguments": arguments
                }
            }))
            .send()
            .await
            .map_err(|e| UpstreamError::RequestFailed(format!("HTTP request failed: {}", e)))?;

        if !response.status().is_success() {
            return Err(UpstreamError::RequestFailed(format!(
                "HTTP {} response",
                response.status()
            )));
        }

        let response_json: McpResponse = response.json().await.map_err(|e| {
            UpstreamError::ProtocolError(format!("Failed to parse response: {}", e))
        })?;

        if let Some(error) = response_json.error {
            return Err(UpstreamError::RequestFailed(format!(
                "Server error: {}",
                error
            )));
        }

        response_json
            .result
            .ok_or_else(|| UpstreamError::ProtocolError("No result in response".to_string()))
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

    /// Disconnect from a server
    pub async fn disconnect(&self, alias: &str) -> Result<()> {
        info!("Disconnecting from: {}", alias);

        // Remove tools from registry
        self.registry.remove_server(alias);

        // Kill process if stdio
        {
            let mut sessions = self.sessions.write().await;
            if let Some(session) = sessions.get_mut(alias) {
                if let Some(mut process) = session.process.take() {
                    let _ = process.kill().await;
                }
            }
        }

        // Remove session
        self.sessions.write().await.remove(alias);

        Ok(())
    }

    /// Reconnect to a server (for failure recovery)
    pub async fn reconnect(&self, alias: &str) -> Result<()> {
        warn!("Reconnecting to: {}", alias);

        // Mark as degraded
        self.registry.mark_server_degraded(alias);

        // Attempt reconnection
        match self.connect(alias).await {
            Ok(_) => {
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


