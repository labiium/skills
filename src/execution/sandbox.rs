//! Sandboxing for bundled tool execution
//!
//! Provides multiple sandboxing backends:
//! - None: No sandboxing (development only)
//! - Timeout: Basic timeout enforcement
//! - Restricted: Environment/resource hardening, not filesystem/network containment
//! - Bubblewrap: Linux container-based sandboxing
//! - Docker: Docker container-based sandboxing
//! - WASM: WebAssembly-based sandboxing (wasmtime runtime)

use crate::execution::wasm::WasmSandbox;
use bollard::container::LogOutput;
use bollard::models::{
    ContainerCreateBody, HostConfig, HostConfigLogConfig, Mount, MountType, ResourcesUlimits,
};
use bollard::query_parameters::{
    AttachContainerOptions, CreateContainerOptions, KillContainerOptions, RemoveContainerOptions,
    StartContainerOptions, WaitContainerOptions,
};
use bollard::Docker;
use futures::stream::StreamExt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tracing::{error, info, warn};

const DEFAULT_MAX_OUTPUT_BYTES: u64 = 8 * 1024 * 1024;

fn decode_output(output: Vec<u8>) -> String {
    String::from_utf8(output)
        .unwrap_or_else(|error| String::from_utf8_lossy(error.as_bytes()).into_owned())
}

fn append_output(
    output: &mut Vec<u8>,
    bytes: &[u8],
    remaining: &mut u64,
    limit: u64,
) -> Result<()> {
    let count = u64::try_from(bytes.len()).map_err(std::io::Error::other)?;
    if count > *remaining {
        return Err(SandboxError::OutputLimitExceeded(limit));
    }
    let required = output
        .len()
        .checked_add(bytes.len())
        .ok_or(SandboxError::OutputLimitExceeded(limit))?;
    if required > output.capacity() {
        let capacity = output
            .capacity()
            .saturating_mul(2)
            .max(required)
            .min(usize::try_from(limit).unwrap_or(usize::MAX));
        output
            .try_reserve_exact(
                capacity
                    .checked_sub(output.len())
                    .ok_or(SandboxError::OutputLimitExceeded(limit))?,
            )
            .map_err(std::io::Error::other)?;
    }
    output.extend_from_slice(bytes);
    *remaining = remaining
        .checked_sub(count)
        .ok_or(SandboxError::OutputLimitExceeded(limit))?;
    Ok(())
}

async fn capture_native_output(
    stdout: &mut (impl AsyncRead + Unpin),
    stderr: &mut (impl AsyncRead + Unpin),
    out: &mut Vec<u8>,
    err: &mut Vec<u8>,
    limit: u64,
) -> Result<()> {
    let mut remaining = limit;
    let mut stdout_buffer = [0_u8; 4096];
    let mut stderr_buffer = [0_u8; 4096];
    let mut stdout_open = true;
    let mut stderr_open = true;
    while stdout_open || stderr_open {
        tokio::select! {
            result = stdout.read(&mut stdout_buffer), if stdout_open => {
                let count = result?;
                stdout_open = count != 0;
                let bytes = stdout_buffer.get(..count).ok_or_else(|| {
                    std::io::Error::other("stdout read exceeded its buffer")
                })?;
                append_output(out, bytes, &mut remaining, limit)?;
            }
            result = stderr.read(&mut stderr_buffer), if stderr_open => {
                let count = result?;
                stderr_open = count != 0;
                let bytes = stderr_buffer.get(..count).ok_or_else(|| {
                    std::io::Error::other("stderr read exceeded its buffer")
                })?;
                append_output(err, bytes, &mut remaining, limit)?;
            }
        }
    }
    Ok(())
}

async fn wait_for_container(docker: &Docker, id: &str) -> Result<Option<i32>> {
    let mut stream = docker.wait_container(
        id,
        Some(WaitContainerOptions {
            condition: "not-running".to_string(),
        }),
    );
    let status = match stream.next().await {
        Some(Ok(response)) => response.status_code,
        Some(Err(bollard::errors::Error::DockerContainerWaitError { code, .. })) => code,
        Some(Err(error)) => {
            return Err(SandboxError::ExecutionFailed(format!(
                "Docker wait failed: {error}"
            )));
        }
        None => {
            return Err(SandboxError::ExecutionFailed(
                "Docker wait stream closed without exit status".to_string(),
            ));
        }
    };
    Ok(i32::try_from(status).ok())
}

fn elapsed_millis(start: std::time::Instant) -> u64 {
    u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)
}

#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    reason = "Docker requires an integer nanocpu value after explicit finite and range checks"
)]
fn cpu_quota_to_nano_cpus(cpu_quota: f64) -> Result<Option<i64>> {
    if cpu_quota <= 0.0 {
        return Ok(None);
    }
    let nano_cpus = cpu_quota * 1_000_000_000.0;
    if !nano_cpus.is_finite() || nano_cpus > 9_223_372_036_854_775_807.0 {
        return Err(SandboxError::InvalidConfig(
            "Docker CPU quota is outside the supported range".to_string(),
        ));
    }
    Ok(Some(nano_cpus as i64))
}

/// Sandbox configuration override for per-server and per-tool settings
///
/// This struct allows overriding sandbox settings at the server or tool level.
/// All fields are optional - if a field is None, the global default is used.
///
/// Example YAML configuration:
/// ```yaml
/// upstreams:
///   - alias: filesystem
///     transport: stdio
///     command: ["npx", "-y", "@modelcontextprotocol/server-filesystem", "."]
///     sandbox_config:
///       backend: restricted
///       allow_read:
///         - /home/user/projects
///       allow_write:
///         - /tmp
///       timeout_ms: 60000
///
///   - alias: brave-search
///     transport: stdio
///     command: ["npx", "-y", "@modelcontextprotocol/server-brave-search"]
///     sandbox_config:
///       backend: timeout
///       allow_network: true
///       timeout_ms: 30000
/// ```
/// Predefined sandbox configuration presets for common use cases
///
/// Use these to quickly configure sandboxing without specifying individual options.
/// Presets can be further customized with specific overrides if needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SandboxPreset {
    /// Zero configuration - uses global defaults (standard)
    #[default]
    Default,
    /// Development mode - minimal sandboxing, maximum convenience
    Development,
    /// Standard mode - balanced security (recommended for production)
    Standard,
    /// Strict mode - maximum security for untrusted code (Linux bubblewrap)
    Strict,
    /// Isolated mode - maximum security using Docker containers (cross-platform)
    /// Use this for least trusted tools that require strong isolation
    Isolated,
    /// Network-enabled mode - for API/web tools
    Network,
    /// Filesystem-enabled mode - for file manipulation tools
    Filesystem,
    /// WASM-optimized mode - for WebAssembly execution
    Wasm,
}

impl SandboxPreset {
    /// Convert preset to full configuration
    #[must_use]
    pub fn to_config(&self) -> SandboxConfig {
        match self {
            Self::Default => SandboxConfig::default(),
            Self::Development => SandboxConfig::development(),
            Self::Standard => SandboxConfig::standard(),
            Self::Strict => SandboxConfig::strict(),
            Self::Isolated => SandboxConfig::isolated(),
            Self::Network => SandboxConfig::network(),
            Self::Filesystem => SandboxConfig::filesystem(vec![], vec![]),
            Self::Wasm => SandboxConfig::wasm_optimized(),
        }
    }
}

/// Sandbox configuration override for per-server and per-tool settings
///
/// This struct allows overriding sandbox settings at the server or tool level.
/// All fields are optional - if a field is None, the global default is used.
///
/// For minimal configuration, use the `preset` field. For fine-grained control,
/// use individual fields. You can combine both: preset + specific overrides.
///
/// Example YAML configuration:
/// ```yaml
/// # Simple: Just use a preset
/// upstreams:
///   - alias: filesystem
///     transport: stdio
///     command: ["npx", "-y", "@modelcontextprotocol/server-filesystem", "."]
///     sandbox_config:
///       preset: filesystem
///       allow_read:
///         - /home/user/projects
///       allow_write:
///         - /tmp
///
/// # Even simpler: No config needed for standard security!
///   - alias: brave-search
///     transport: stdio
///     command: ["npx", "-y", "@modelcontextprotocol/server-brave-search"]
///     # Uses standard preset automatically (timeout, no network, 512MB RAM)
///
/// # Advanced: Full control
///   - alias: untrusted-tool
///     transport: stdio
///     command: ["untrusted-mcp-server"]
///     sandbox_config:
///       preset: strict
///       timeout_ms: 5000
///       max_memory_bytes: 134217728
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SandboxConfigOverride {
    /// Preset configuration to use as base
    ///
    /// If specified, this preset is applied first, then other fields override it.
    /// If not specified, uses global defaults.
    #[serde(default)]
    pub preset: Option<SandboxPreset>,

    /// Backend to use (overrides preset/global default)
    pub backend: Option<SandboxBackend>,
    /// Timeout in milliseconds (overrides preset/global default)
    pub timeout_ms: Option<u64>,
    /// Allow network access (overrides preset/global default)
    #[serde(default)]
    pub allow_network: Option<bool>,
    /// Max memory in bytes (overrides preset/global default)
    pub max_memory_bytes: Option<u64>,
    /// Max CPU time in seconds (overrides preset/global default)
    pub max_cpu_seconds: Option<u64>,
    /// Maximum combined captured stdout/stderr bytes (zero permits no output)
    pub max_output_bytes: Option<u64>,
    /// Additional allowed read paths (merged with preset/global)
    #[serde(default)]
    pub allow_read: Vec<PathBuf>,
    /// Additional allowed write paths (merged with preset/global)
    #[serde(default)]
    pub allow_write: Vec<PathBuf>,
}

impl SandboxConfigOverride {
    /// Create override from preset only
    #[must_use]
    pub fn from_preset(preset: SandboxPreset) -> Self {
        Self {
            preset: Some(preset),
            ..Default::default()
        }
    }

    /// Resolve this override to a full configuration
    ///
    /// Uses preset if specified, otherwise uses `base_config`, then applies
    /// individual field overrides.
    #[must_use]
    pub fn resolve(&self, base_config: &SandboxConfig) -> SandboxConfig {
        // Start with preset if specified, otherwise base config
        let mut config = self
            .preset
            .map_or_else(|| base_config.clone(), |preset| preset.to_config());

        // Apply individual overrides
        if let Some(backend) = self.backend {
            config.backend = backend;
        }
        if let Some(timeout_ms) = self.timeout_ms {
            config.timeout_ms = timeout_ms;
        }
        if let Some(allow_network) = self.allow_network {
            config.allow_network = allow_network;
        }
        if let Some(max_memory_bytes) = self.max_memory_bytes {
            config.max_memory_bytes = max_memory_bytes;
        }
        if let Some(max_cpu_seconds) = self.max_cpu_seconds {
            config.max_cpu_seconds = max_cpu_seconds;
        }
        if let Some(max_output_bytes) = self.max_output_bytes {
            config.max_output_bytes = max_output_bytes;
        }
        config.allow_read.extend(self.allow_read.clone());
        config.allow_write.extend(self.allow_write.clone());

        config
    }
}

#[derive(Error, Debug)]
pub enum SandboxError {
    #[error("Sandbox execution failed: {0}")]
    ExecutionFailed(String),

    #[error("Timeout exceeded: {0}ms")]
    Timeout(u64),

    #[error("Captured output exceeded the {0}-byte limit")]
    OutputLimitExceeded(u64),

    #[error("Sandbox not available: {0}")]
    NotAvailable(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Invalid configuration: {0}")]
    InvalidConfig(String),
}

pub type Result<T> = std::result::Result<T, SandboxError>;

/// Sandboxing backend
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SandboxBackend {
    /// No sandboxing (development only)
    None,
    /// Trusted-code hardening only; detached descendants can escape process-group cleanup.
    Timeout,
    /// Trusted-code hardening only; no hostile-code or detached-descendant containment.
    Restricted,
    /// Bubblewrap (Linux container); fails closed when unavailable.
    #[cfg_attr(target_os = "linux", default)]
    Bubblewrap,
    /// Docker container-based sandboxing
    #[cfg_attr(not(target_os = "linux"), default)]
    Docker,
    /// WebAssembly with capability-scoped WASI and enforced guest resource limits.
    Wasm,
}

impl SandboxBackend {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Timeout => "timeout",
            Self::Restricted => "restricted",
            Self::Bubblewrap => "bubblewrap",
            Self::Docker => "docker",
            Self::Wasm => "wasm",
        }
    }
}

/// Docker container configuration
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DockerConfig {
    /// Docker image to use
    pub image: String,
    /// Container memory limit in bytes (0 = unlimited)
    pub memory_limit: i64,
    /// CPU quota (number of CPUs, e.g., 0.5 for half a CPU)
    pub cpu_quota: f64,
    /// Environment variables for the container
    pub env_vars: HashMap<String, String>,
    /// Working directory inside the container
    pub working_dir: String,
    /// Auto-remove container after execution
    #[serde(default = "default_auto_remove")]
    pub auto_remove: bool,
    /// Network mode ("none", "bridge", "host", or custom network name)
    #[serde(default = "default_network_mode")]
    pub network_mode: String,
    /// Container entrypoint override
    pub entrypoint: Option<Vec<String>>,
    /// Additional mount points
    #[serde(default)]
    pub mounts: Vec<DockerMount>,
}

const fn default_auto_remove() -> bool {
    true
}

fn default_network_mode() -> String {
    "none".to_string()
}

/// Docker mount configuration
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DockerMount {
    /// Source path on host
    pub source: PathBuf,
    /// Target path in container
    pub target: String,
    /// Mount read-only
    #[serde(default)]
    pub read_only: bool,
}

impl Default for DockerConfig {
    fn default() -> Self {
        Self {
            image: "alpine:latest".to_string(),
            memory_limit: 256 * 1024 * 1024, // 256 MB
            cpu_quota: 1.0,
            env_vars: HashMap::new(),
            working_dir: "/workspace".to_string(),
            auto_remove: true,
            network_mode: "none".to_string(),
            entrypoint: None,
            mounts: Vec::new(),
        }
    }
}

/// Sandbox configuration
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SandboxConfig {
    /// Sandboxing backend
    pub backend: SandboxBackend,
    /// Maximum execution time (milliseconds)
    pub timeout_ms: u64,
    /// Allowed read paths
    pub allow_read: Vec<PathBuf>,
    /// Allowed write paths
    pub allow_write: Vec<PathBuf>,
    /// Allow network access
    pub allow_network: bool,
    /// Maximum memory (bytes, 0 = unlimited)
    pub max_memory_bytes: u64,
    /// Maximum CPU time (seconds, 0 = unlimited)
    pub max_cpu_seconds: u64,
    /// Maximum combined raw stdout/stderr bytes, including WASM output.
    /// Defaults to 8 MiB; zero permits no output, not unlimited output.
    pub max_output_bytes: u64,
    /// Docker-specific configuration (only used when backend is Docker)
    #[serde(default)]
    pub docker: DockerConfig,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            backend: SandboxBackend::default(),
            timeout_ms: 30000, // 30 seconds
            allow_read: vec![],
            allow_write: vec![],
            allow_network: false,
            max_memory_bytes: 512 * 1024 * 1024, // 512 MB
            max_cpu_seconds: 30,
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
            docker: DockerConfig::default(),
        }
    }
}

impl SandboxConfig {
    /// Development configuration - minimal sandboxing (use with caution!)
    ///
    /// Only enables timeout protection. No filesystem or network restrictions.
    /// Suitable for: Local development, trusted environments only.
    #[must_use]
    pub fn development() -> Self {
        Self {
            backend: SandboxBackend::Timeout,
            timeout_ms: 60000,
            allow_read: vec![],
            allow_write: vec![],
            allow_network: true,
            max_memory_bytes: 1024 * 1024 * 1024, // 1 GB
            max_cpu_seconds: 60,                  // 60 seconds
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
            docker: DockerConfig {
                network_mode: "bridge".to_string(),
                memory_limit: 1024 * 1024 * 1024, // 1 GB
                ..Default::default()
            },
        }
    }

    /// Standard configuration: enforced filesystem/network containment.
    /// Requires Bubblewrap on Linux or Docker on other platforms; never falls back.
    #[must_use]
    pub fn standard() -> Self {
        Self::default()
    }

    /// Strict configuration - maximum security
    ///
    /// Uses bubblewrap containerization with minimal permissions.
    /// Suitable for: Running untrusted code, production WASM execution.
    #[must_use]
    pub fn strict() -> Self {
        let docker_config = DockerConfig::default();

        Self {
            backend: SandboxBackend::Bubblewrap,
            timeout_ms: 10000,                   // 10 seconds
            allow_read: vec![],                  // No filesystem access by default
            allow_write: vec![],                 // No write access
            allow_network: false,                // No network
            max_memory_bytes: 256 * 1024 * 1024, // 256 MB
            max_cpu_seconds: 10,                 // 10 seconds
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
            docker: docker_config,
        }
    }

    /// Isolated configuration - maximum security using Docker
    ///
    /// Uses Docker containerization for strong isolation across all platforms.
    /// This is the most secure option for least trusted tools.
    /// Suitable for: Running untrusted code, code from unknown sources,
    /// or when bubblewrap is not available.
    #[must_use]
    pub fn isolated() -> Self {
        Self {
            backend: SandboxBackend::Docker,
            timeout_ms: 10000,                   // 10 seconds
            allow_read: vec![],                  // No filesystem access by default
            allow_write: vec![],                 // No write access
            allow_network: false,                // No network
            max_memory_bytes: 256 * 1024 * 1024, // 256 MB
            max_cpu_seconds: 10,                 // 10 seconds
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
            docker: DockerConfig {
                image: "alpine:latest".to_string(),
                memory_limit: 256 * 1024 * 1024,  // 256 MB
                cpu_quota: 0.5,                   // Half a CPU
                network_mode: "none".to_string(), // No network
                auto_remove: true,
                ..Default::default()
            },
        }
    }

    /// Network-enabled configuration
    ///
    /// Allows network access with other restrictions in place.
    /// Suitable for: Web search, API clients, fetch tools.
    #[must_use]
    pub fn network() -> Self {
        Self {
            backend: Self::default().backend,
            timeout_ms: 30000,                   // 30 seconds
            allow_read: vec![],                  // Minimal read access
            allow_write: vec![],                 // No write access
            allow_network: true,                 // Network enabled
            max_memory_bytes: 512 * 1024 * 1024, // 512 MB
            max_cpu_seconds: 30,                 // 30 seconds
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
            docker: DockerConfig {
                network_mode: "bridge".to_string(),
                ..Default::default()
            },
        }
    }

    /// Filesystem-enabled configuration
    ///
    /// Allows controlled filesystem access with other restrictions.
    /// Suitable for: File editors, code generators, data processors.
    #[must_use]
    pub fn filesystem(read_paths: Vec<PathBuf>, write_paths: Vec<PathBuf>) -> Self {
        let docker_config = DockerConfig::default();

        Self {
            backend: Self::default().backend,
            timeout_ms: 30000,                   // 30 seconds
            allow_read: read_paths,              // Specified read paths
            allow_write: write_paths,            // Specified write paths
            allow_network: false,                // No network
            max_memory_bytes: 512 * 1024 * 1024, // 512 MB
            max_cpu_seconds: 30,                 // 30 seconds
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
            docker: docker_config,
        }
    }

    /// WASM-optimized configuration
    ///
    /// Optimized for WebAssembly module execution.
    /// Suitable for: Running .wasm bundled tools with resource limits.
    #[must_use]
    pub fn wasm_optimized() -> Self {
        let docker_config = DockerConfig::default();

        Self {
            backend: SandboxBackend::Wasm,
            timeout_ms: 30000,                   // 30 seconds
            allow_read: vec![],                  // Controlled by WASI preopens
            allow_write: vec![],                 // Controlled by WASI preopens
            allow_network: false,                // No WASI network
            max_memory_bytes: 256 * 1024 * 1024, // 256 MB
            max_cpu_seconds: 30,                 // 30 seconds
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
            docker: docker_config,
        }
    }

    /// Merge with override configuration
    ///
    /// Returns a new `SandboxConfig` with overrides applied.
    /// Override values take precedence over base configuration.
    ///
    /// This method handles presets - if the override specifies a preset,
    /// it starts from the preset configuration and applies individual overrides.
    #[must_use]
    pub fn with_override(&self, override_config: &SandboxConfigOverride) -> Self {
        override_config.resolve(self)
    }

    /// Create config for specific tool from base + overrides
    ///
    /// Applies overrides in order of precedence:
    /// 1. Global default (self)
    /// 2. Server override (if provided)
    /// 3. Tool override (if provided)
    ///
    /// Each override can specify a preset and/or individual field overrides.
    #[must_use]
    pub fn for_tool(
        &self,
        tool_override: Option<&SandboxConfigOverride>,
        server_override: Option<&SandboxConfigOverride>,
    ) -> Self {
        let mut config = self.clone();

        // Apply server override first
        if let Some(server_config) = server_override {
            config = server_config.resolve(&config);
        }

        // Apply tool override second (takes precedence)
        if let Some(tool_config) = tool_override {
            config = tool_config.resolve(&config);
        }

        config
    }
}

/// Execution result from sandbox
#[derive(Debug, Serialize, Deserialize)]
pub struct SandboxResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    pub timed_out: bool,
}

/// Sandboxed executor
pub struct Sandbox {
    config: SandboxConfig,
}

/// Retain this value for the lifetime of the process so its private home/tmp survives.
pub struct PreparedCommand {
    command: Command,
    _directory: Option<tempfile::TempDir>,
}

impl PreparedCommand {
    pub const fn command_mut(&mut self) -> &mut Command {
        &mut self.command
    }

    /// # Errors
    /// Returns spawn errors, including failure to apply configured resource limits.
    pub fn spawn(&mut self) -> Result<(tokio::process::Child, ProcessGroup)> {
        let child = self.command.spawn()?;
        let group = ProcessGroup { id: child.id() };
        Ok((child, group))
    }
}

/// Kills descendants still in the original Unix process group on exit or cancellation.
///
/// Native trusted-code modes cannot stop descendants that create a new session/group.
/// Bubblewrap additionally confines descendants to its private PID namespace.
pub struct ProcessGroup {
    id: Option<u32>,
}

impl ProcessGroup {
    /// # Errors
    /// Returns OS errors when the process group cannot be terminated.
    pub fn terminate(&self) -> std::io::Result<()> {
        #[cfg(unix)]
        if let Some(id) = self.id {
            let id = i32::try_from(id).map_err(std::io::Error::other)?;
            let group = id
                .checked_neg()
                .filter(|group| *group < 0)
                .ok_or_else(|| std::io::Error::other("Process group ID must be positive"))?;
            // Each prepared process starts a fresh process group before exec.
            if unsafe { libc::kill(group, libc::SIGKILL) } != 0 {
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() != Some(libc::ESRCH) {
                    return Err(error);
                }
            }
        }
        Ok(())
    }
}

impl Drop for ProcessGroup {
    fn drop(&mut self) {
        if let Err(error) = self.terminate() {
            error!("Failed to terminate subprocess group: {error}");
        }
    }
}

struct ContainerCleanup {
    docker: Docker,
    id: String,
    armed: bool,
}

impl Drop for ContainerCleanup {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let docker = self.docker.clone();
        let id = self.id.clone();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                if let Err(error) = docker
                    .remove_container(
                        &id,
                        Some(RemoveContainerOptions {
                            force: true,
                            ..Default::default()
                        }),
                    )
                    .await
                {
                    error!("Failed to remove sandbox container {id}: {error}");
                }
            });
        }
    }
}

impl Sandbox {
    /// Create a new sandbox with the given configuration
    #[must_use]
    pub const fn new(config: SandboxConfig) -> Self {
        Self { config }
    }

    /// Execute a command in the sandbox
    /// # Errors
    /// Returns an error if the operation cannot be completed.
    pub async fn execute(
        &self,
        program: &str,
        args: &[String],
        working_dir: &Path,
        env_vars: &[(String, String)],
    ) -> Result<SandboxResult> {
        match self.config.backend {
            SandboxBackend::None
            | SandboxBackend::Timeout
            | SandboxBackend::Restricted
            | SandboxBackend::Bubblewrap => {
                self.execute_native(program, args, working_dir, env_vars)
                    .await
            }
            SandboxBackend::Docker => {
                self.execute_docker(program, args, working_dir, env_vars)
                    .await
            }
            SandboxBackend::Wasm => {
                self.execute_wasm(program, args, working_dir, env_vars)
                    .await
            }
        }
    }

    /// Prepare the same enforced command boundary used for bundled scripts and MCP stdio.
    /// # Errors
    /// Rejects unenforceable policies and unsupported backends before spawning.
    #[expect(
        clippy::too_many_lines,
        reason = "command preparation defines one ordered capability boundary"
    )]
    pub fn prepare_command(
        &self,
        program: &str,
        args: &[String],
        working_dir: &Path,
        env_vars: &[(String, String)],
    ) -> Result<PreparedCommand> {
        if matches!(
            self.config.backend,
            SandboxBackend::Docker | SandboxBackend::Wasm
        ) {
            return Err(SandboxError::NotAvailable(
                "This backend cannot expose a native stdio command".to_string(),
            ));
        }
        if matches!(
            self.config.backend,
            SandboxBackend::None | SandboxBackend::Timeout | SandboxBackend::Restricted
        ) && (!self.config.allow_network
            || !self.config.allow_read.is_empty()
            || !self.config.allow_write.is_empty())
        {
            return Err(SandboxError::InvalidConfig(
                "Native none/timeout/restricted modes cannot enforce filesystem/network restrictions; use bubblewrap, Docker, or WASM".to_string(),
            ));
        }
        if self.config.backend == SandboxBackend::None
            && (self.config.max_memory_bytes > 0 || self.config.max_cpu_seconds > 0)
        {
            return Err(SandboxError::InvalidConfig("The none backend does not enforce resource limits; use timeout or an isolated backend".to_string()));
        }
        #[cfg(not(unix))]
        if self.config.backend != SandboxBackend::None || self.config.timeout_ms > 0 {
            return Err(SandboxError::NotAvailable(
                "Native process-group/resource hardening requires Unix; use Docker".to_string(),
            ));
        }
        let directory = if self.config.backend == SandboxBackend::None {
            None
        } else {
            Some(tempfile::tempdir()?)
        };
        let private_path = directory.as_ref().map(tempfile::TempDir::path);
        let working_dir = working_dir.canonicalize()?;
        let mut command = if self.config.backend == SandboxBackend::Bubblewrap {
            if !cfg!(target_os = "linux") {
                return Err(SandboxError::NotAvailable(
                    "Bubblewrap requires Linux".to_string(),
                ));
            }
            let mut command = Command::new("bwrap");
            command
                .arg("--unshare-all")
                .arg("--die-with-parent")
                .arg("--new-session");
            if self.config.allow_network {
                command.arg("--share-net");
            }
            for path in ["/usr", "/lib", "/lib64", "/bin", "/sbin"] {
                if Path::new(path).exists() {
                    command.arg("--ro-bind").arg(path).arg(path);
                }
            }
            command
                .arg("--proc")
                .arg("/proc")
                .arg("--dev")
                .arg("/dev")
                .arg("--tmpfs")
                .arg("/tmp")
                .arg("--ro-bind")
                .arg(&working_dir)
                .arg(&working_dir);
            for path in &self.config.allow_read {
                let path = path.canonicalize()?;
                command.arg("--ro-bind").arg(&path).arg(&path);
            }
            for path in &self.config.allow_write {
                let path = path.canonicalize()?;
                command.arg("--bind").arg(&path).arg(&path);
            }
            let private_path = private_path.ok_or_else(|| {
                SandboxError::InvalidConfig(
                    "Bubblewrap requires a private invocation directory".to_string(),
                )
            })?;
            command
                .arg("--bind")
                .arg(private_path)
                .arg(private_path)
                .arg("--chdir")
                .arg(&working_dir)
                .arg("--")
                .arg(program)
                .args(args);
            command
        } else {
            let mut command = Command::new(program);
            command.args(args);
            command
        };
        command
            .current_dir(&working_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(private_path) = private_path {
            command
                .env_clear()
                .env("PATH", "/usr/local/bin:/usr/bin:/bin")
                .env("LANG", "C.UTF-8")
                .env("HOME", private_path)
                .env("TMPDIR", private_path)
                .env("TMP", private_path)
                .env("TEMP", private_path);
        }
        for (key, value) in env_vars {
            command.env(key, value);
        }
        #[cfg(unix)]
        {
            command.process_group(0);
            if self.config.backend != SandboxBackend::None {
                let cpu = libc::rlim_t::try_from(self.config.max_cpu_seconds)
                    .map_err(std::io::Error::other)?;
                let memory = libc::rlim_t::try_from(self.config.max_memory_bytes)
                    .map_err(std::io::Error::other)?;
                // Only async-signal-safe syscalls may run in this post-fork closure.
                unsafe {
                    command.pre_exec(move || {
                        for (resource, limit) in [
                            (libc::RLIMIT_CPU, cpu),
                            (libc::RLIMIT_AS, memory),
                            (libc::RLIMIT_NOFILE, 256),
                        ] {
                            if limit > 0 {
                                let limits = libc::rlimit {
                                    rlim_cur: limit,
                                    rlim_max: limit,
                                };
                                if libc::setrlimit(resource, &raw const limits) != 0 {
                                    return Err(std::io::Error::last_os_error());
                                }
                            }
                        }
                        Ok(())
                    });
                }
            }
        }
        Ok(PreparedCommand {
            command,
            _directory: directory,
        })
    }

    async fn execute_native(
        &self,
        program: &str,
        args: &[String],
        working_dir: &Path,
        env_vars: &[(String, String)],
    ) -> Result<SandboxResult> {
        let mut prepared = self.prepare_command(program, args, working_dir, env_vars)?;
        let start = std::time::Instant::now();
        let (mut child, group) = prepared.spawn()?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| SandboxError::ExecutionFailed("Missing captured stdout".to_string()))?;
        let mut stderr = child
            .stderr
            .take()
            .ok_or_else(|| SandboxError::ExecutionFailed("Missing captured stderr".to_string()))?;
        let mut out = Vec::new();
        let mut err = Vec::new();
        let wait = async {
            tokio::try_join!(
                async { child.wait().await.map_err(SandboxError::from) },
                capture_native_output(
                    &mut stdout,
                    &mut stderr,
                    &mut out,
                    &mut err,
                    self.config.max_output_bytes,
                ),
            )
        };
        let result = if self.config.timeout_ms == 0 {
            Some(wait.await)
        } else {
            tokio::time::timeout(Duration::from_millis(self.config.timeout_ms), wait)
                .await
                .ok()
        };
        group.terminate()?;
        if !matches!(&result, Some(Ok(_))) {
            child.kill().await.ok();
            child.wait().await?;
        }
        let Some(result) = result else {
            return Ok(SandboxResult {
                stdout: decode_output(out),
                stderr: "Execution timed out".to_string(),
                exit_code: None,
                duration_ms: elapsed_millis(start),
                timed_out: true,
            });
        };
        let (status, ()) = result?;
        Ok(SandboxResult {
            stdout: decode_output(out),
            stderr: decode_output(err),
            exit_code: status.code(),
            duration_ms: elapsed_millis(start),
            timed_out: false,
        })
    }

    /// Execute a WASM module using wasmtime
    ///
    /// The program path should point to a .wasm file.
    /// Arguments are passed as JSON via environment variables or stdin.
    async fn execute_wasm(
        &self,
        program: &str,
        _args: &[String],
        _working_dir: &Path,
        env_vars: &[(String, String)],
    ) -> Result<SandboxResult> {
        let wasm_path = Path::new(program);

        // Check if file exists and has .wasm extension
        if !wasm_path.exists() {
            return Err(SandboxError::ExecutionFailed(format!(
                "WASM file not found: {program}"
            )));
        }

        if wasm_path.extension().and_then(|e| e.to_str()) != Some("wasm") {
            return Err(SandboxError::ExecutionFailed(format!(
                "Not a WASM file: {program}"
            )));
        }

        // Prepare input JSON from environment variables
        // First, try to read from SKILL_ARGS_JSON or SKILL_ARGS_FILE
        let input_json = env_vars
            .iter()
            .find(|(k, _)| k == "SKILL_ARGS_JSON")
            .map(|(_, v)| v.clone())
            .or_else(|| {
                env_vars
                    .iter()
                    .find(|(k, _)| k == "SKILL_ARGS_FILE")
                    .and_then(|(_, path)| std::fs::read_to_string(path).ok())
            })
            .unwrap_or_else(|| "{}".to_string());

        // WasmSandbox owns interruption and waits for blocking work to stop.
        WasmSandbox::new(self.config.clone())
            .execute(wasm_path, &input_json)
            .await
    }

    /// Execute using Docker container
    ///
    /// Creates a Docker container with the configured image and executes the command.
    /// Supports resource limits, network isolation, and volume mounts.
    #[expect(
        clippy::too_many_lines,
        reason = "Docker lifecycle cleanup and result collection are kept in one ordered operation"
    )]
    async fn execute_docker(
        &self,
        program: &str,
        args: &[String],
        working_dir: &Path,
        env_vars: &[(String, String)],
    ) -> Result<SandboxResult> {
        // Connect to Docker daemon
        let docker = Docker::connect_with_local_defaults()
            .map_err(|e| SandboxError::NotAvailable(format!("Failed to connect to Docker: {e}")))?;

        info!(
            "Executing with Docker: {} {} (image: {})",
            program,
            args.join(" "),
            self.config.docker.image
        );

        // Build the command to execute
        let mut cmd_parts = vec![program.to_string()];
        cmd_parts.extend(args.iter().cloned());

        // Prepare environment variables for the container
        let mut container_env = self.config.docker.env_vars.clone();
        container_env
            .entry("HOME".to_string())
            .or_insert_with(|| "/tmp".to_string());
        container_env
            .entry("TMPDIR".to_string())
            .or_insert_with(|| "/tmp".to_string());

        // Add user-provided env vars
        for (key, value) in env_vars {
            container_env.insert(key.clone(), value.clone());
        }

        // Convert env vars to Docker format: "KEY=VALUE"
        let env_list: Vec<String> = container_env
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect();

        // Build volume mounts from allow_read and allow_write
        let mut mounts = Vec::new();
        let host_working_dir = working_dir.canonicalize()?;
        if host_working_dir != Path::new(&self.config.docker.working_dir) {
            mounts.push(Mount {
                target: Some(host_working_dir.to_string_lossy().into_owned()),
                source: Some(host_working_dir.to_string_lossy().into_owned()),
                typ: Some(MountType::BIND),
                read_only: Some(true),
                ..Default::default()
            });
        }

        // Add working directory as a mount
        mounts.push(Mount {
            target: Some(self.config.docker.working_dir.clone()),
            source: Some(working_dir.canonicalize()?.to_string_lossy().into_owned()),
            typ: Some(MountType::BIND),
            read_only: Some(true),
            ..Default::default()
        });

        // Add configured read paths
        for path in &self.config.allow_read {
            let path = path.canonicalize()?;
            mounts.push(Mount {
                target: Some(path.to_string_lossy().into_owned()),
                source: Some(path.to_string_lossy().into_owned()),
                typ: Some(MountType::BIND),
                read_only: Some(true),
                ..Default::default()
            });
        }

        // Add configured write paths
        for path in &self.config.allow_write {
            let path = path.canonicalize()?;
            mounts.push(Mount {
                target: Some(path.to_string_lossy().into_owned()),
                source: Some(path.to_string_lossy().into_owned()),
                typ: Some(MountType::BIND),
                read_only: Some(false),
                ..Default::default()
            });
        }

        // Add custom mounts from docker config
        for mount in &self.config.docker.mounts {
            mounts.push(Mount {
                target: Some(mount.target.clone()),
                source: Some(mount.source.canonicalize()?.to_string_lossy().into_owned()),
                typ: Some(MountType::BIND),
                read_only: Some(mount.read_only),
                ..Default::default()
            });
        }

        // Determine network mode based on config
        let network_mode = if self.config.allow_network {
            self.config.docker.network_mode.clone()
        } else {
            "none".to_string()
        };

        // Convert timeout to nanoseconds for Docker
        let nano_cpus = cpu_quota_to_nano_cpus(self.config.docker.cpu_quota)?;

        let memory = match (
            (self.config.max_memory_bytes > 0).then_some(self.config.max_memory_bytes),
            u64::try_from(self.config.docker.memory_limit)
                .ok()
                .filter(|limit| *limit > 0),
        ) {
            (Some(global), Some(container)) => Some(global.min(container)),
            (global, container) => global.or(container),
        }
        .map(i64::try_from)
        .transpose()
        .map_err(|error| SandboxError::InvalidConfig(error.to_string()))?;

        let cpu_limit = (self.config.max_cpu_seconds > 0)
            .then(|| i64::try_from(self.config.max_cpu_seconds))
            .transpose()
            .map_err(|error| SandboxError::InvalidConfig(error.to_string()))?;
        mounts.sort_unstable_by(|left, right| {
            left.target
                .cmp(&right.target)
                .then_with(|| left.read_only.cmp(&right.read_only))
        });
        mounts.dedup_by(|left, right| left.target == right.target);
        // Create host config
        let host_config = HostConfig {
            mounts: if mounts.is_empty() {
                None
            } else {
                Some(mounts)
            },
            network_mode: Some(network_mode),
            nano_cpus,
            memory,
            auto_remove: Some(false),
            // Attach before start instead of accumulating daemon-side container logs.
            log_config: Some(HostConfigLogConfig {
                typ: Some("none".to_string()),
                ..Default::default()
            }),
            ulimits: cpu_limit.map(|limit| {
                vec![ResourcesUlimits {
                    name: Some("cpu".to_string()),
                    soft: Some(limit),
                    hard: Some(limit),
                }]
            }),
            readonly_rootfs: Some(true),
            cap_drop: Some(vec!["ALL".to_string()]),
            security_opt: Some(vec!["no-new-privileges:true".to_string()]),
            tmpfs: Some(HashMap::from([(
                "/tmp".to_string(),
                "rw,nosuid,nodev,size=64m".to_string(),
            )])),
            ..Default::default()
        };

        // Create container configuration using the current Docker API model.
        let config = ContainerCreateBody {
            image: Some(self.config.docker.image.clone()),
            cmd: Some(cmd_parts),
            attach_stdout: Some(true),
            attach_stderr: Some(true),
            tty: Some(false),
            working_dir: Some(self.config.docker.working_dir.clone()),
            env: if env_list.is_empty() {
                None
            } else {
                Some(env_list)
            },
            host_config: Some(host_config),
            entrypoint: self.config.docker.entrypoint.clone(),
            ..Default::default()
        };

        // Create the container
        let create_opts = CreateContainerOptions {
            name: Some(format!("skills-sandbox-{}", uuid::Uuid::new_v4())),
            platform: String::new(),
        };

        let container = docker
            .create_container(Some(create_opts), config)
            .await
            .map_err(|e| {
                SandboxError::ExecutionFailed(format!("Failed to create container: {e}"))
            })?;

        let container_id = container.id;
        let mut cleanup = ContainerCleanup {
            docker: docker.clone(),
            id: container_id.clone(),
            armed: true,
        };
        info!("Created Docker container: {}", container_id);

        let mut attached = docker
            .attach_container(
                &container_id,
                Some(AttachContainerOptions {
                    stdout: true,
                    stderr: true,
                    stream: true,
                    ..Default::default()
                }),
            )
            .await
            .map_err(|error| {
                SandboxError::ExecutionFailed(format!("Cannot attach sandbox output: {error}"))
            })?;

        // Start the container
        docker
            .start_container(&container_id, None::<StartContainerOptions>)
            .await
            .map_err(|e| {
                SandboxError::ExecutionFailed(format!("Failed to start container: {e}"))
            })?;

        let start = std::time::Instant::now();
        let limit_ms = match (
            self.config.timeout_ms,
            self.config.max_cpu_seconds.saturating_mul(1000),
        ) {
            (0, cpu) => cpu,
            (timeout, 0) => timeout,
            (timeout, cpu) => timeout.min(cpu),
        };
        let mut out = Vec::new();
        let mut err = Vec::new();
        let capture = async {
            let mut remaining = self.config.max_output_bytes;
            while let Some(output) = attached.output.next().await {
                let output = output.map_err(|error| {
                    SandboxError::ExecutionFailed(format!("Cannot read sandbox output: {error}"))
                })?;
                match output {
                    LogOutput::StdOut { message } | LogOutput::Console { message } => {
                        append_output(
                            &mut out,
                            &message,
                            &mut remaining,
                            self.config.max_output_bytes,
                        )?;
                    }
                    LogOutput::StdErr { message } => {
                        append_output(
                            &mut err,
                            &message,
                            &mut remaining,
                            self.config.max_output_bytes,
                        )?;
                    }
                    LogOutput::StdIn { .. } => {}
                }
            }
            Ok::<(), SandboxError>(())
        };
        let wait = async { tokio::try_join!(wait_for_container(&docker, &container_id), capture) };
        let wait_result = if limit_ms == 0 {
            Some(wait.await)
        } else {
            tokio::time::timeout(Duration::from_millis(limit_ms), wait)
                .await
                .ok()
        };
        let duration_ms = elapsed_millis(start);
        let timed_out = wait_result.is_none();
        let exit_code = match wait_result {
            Some(Ok((exit_code, ()))) => exit_code,
            Some(Err(error)) => {
                docker
                    .remove_container(
                        &container_id,
                        Some(RemoveContainerOptions {
                            force: true,
                            ..Default::default()
                        }),
                    )
                    .await
                    .map_err(|cleanup_error| {
                        SandboxError::ExecutionFailed(format!(
                            "{error}; cannot remove sandbox container: {cleanup_error}"
                        ))
                    })?;
                cleanup.armed = false;
                return Err(error);
            }
            None => None,
        };

        if timed_out {
            // Kill the container if timed out
            docker
                .kill_container(
                    &container_id,
                    Some(KillContainerOptions {
                        signal: "SIGKILL".to_string(),
                    }),
                )
                .await
                .map_err(|error| {
                    SandboxError::ExecutionFailed(format!(
                        "Cannot kill timed-out container: {error}"
                    ))
                })?;
            wait_for_container(&docker, &container_id).await?;
            warn!(
                "Docker container {} timed out after {}ms",
                container_id, duration_ms
            );
        }

        info!(
            "Docker container {} finished in {}ms with exit code: {:?}",
            container_id, duration_ms, exit_code
        );
        if self.config.docker.auto_remove {
            docker
                .remove_container(
                    &container_id,
                    Some(RemoveContainerOptions {
                        force: true,
                        ..Default::default()
                    }),
                )
                .await
                .map_err(|error| {
                    SandboxError::ExecutionFailed(format!(
                        "Cannot remove sandbox container: {error}"
                    ))
                })?;
        }
        // A caller opting to retain stopped containers still gets cancellation cleanup.
        cleanup.armed = false;

        Ok(SandboxResult {
            stdout: decode_output(out),
            stderr: decode_output(err),
            exit_code,
            duration_ms,
            timed_out,
        })
    }

    /// Check if Docker is available
    #[must_use]
    pub fn is_docker_available() -> bool {
        Docker::connect_with_local_defaults()
            .is_ok_and(|docker| futures::executor::block_on(async { docker.ping().await.is_ok() }))
    }
}
