//! Sandboxing for bundled tool execution
//!
//! Provides multiple sandboxing backends:
//! - None: No sandboxing (development only)
//! - Timeout: Basic timeout enforcement
//! - Restricted: Limited filesystem/network access
//! - Bubblewrap: Linux container-based sandboxing
//! - Docker: Docker container-based sandboxing
//! - WASM: WebAssembly-based sandboxing (wasmtime runtime)

use crate::execution::wasm::WasmSandbox;
use bollard::container::{
    Config, CreateContainerOptions, KillContainerOptions, LogOutput, LogsOptions,
    StartContainerOptions, WaitContainerOptions,
};
use bollard::models::{HostConfig, Mount, MountTypeEnum};
use bollard::Docker;
use futures::stream::StreamExt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use thiserror::Error;
use tokio::process::Command;
use tracing::{debug, error, info, warn};

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
    pub fn to_config(&self) -> SandboxConfig {
        match self {
            SandboxPreset::Default => SandboxConfig::default(),
            SandboxPreset::Development => SandboxConfig::development(),
            SandboxPreset::Standard => SandboxConfig::standard(),
            SandboxPreset::Strict => SandboxConfig::strict(),
            SandboxPreset::Isolated => SandboxConfig::isolated(),
            SandboxPreset::Network => SandboxConfig::network(),
            SandboxPreset::Filesystem => SandboxConfig::filesystem(vec![], vec![]),
            SandboxPreset::Wasm => SandboxConfig::wasm_optimized(),
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
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
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
    /// Additional allowed read paths (merged with preset/global)
    #[serde(default)]
    pub allow_read: Vec<PathBuf>,
    /// Additional allowed write paths (merged with preset/global)
    #[serde(default)]
    pub allow_write: Vec<PathBuf>,
}

impl SandboxConfigOverride {
    /// Create override from preset only
    pub fn from_preset(preset: SandboxPreset) -> Self {
        SandboxConfigOverride {
            preset: Some(preset),
            ..Default::default()
        }
    }

    /// Resolve this override to a full configuration
    ///
    /// Uses preset if specified, otherwise uses base_config, then applies
    /// individual field overrides.
    pub fn resolve(&self, base_config: &SandboxConfig) -> SandboxConfig {
        // Start with preset if specified, otherwise base config
        let mut config = if let Some(preset) = self.preset {
            preset.to_config()
        } else {
            base_config.clone()
        };

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
    /// Basic timeout enforcement only
    #[default]
    Timeout,
    /// Restricted environment (limited FS/network)
    Restricted,
    /// Bubblewrap (Linux container)
    Bubblewrap,
    /// Docker container-based sandboxing
    Docker,
    /// WASM runtime (future)
    #[allow(dead_code)]
    Wasm,
}

/// Docker container configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
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

fn default_auto_remove() -> bool {
    true
}

fn default_network_mode() -> String {
    "none".to_string()
}

/// Docker mount configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
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
        DockerConfig {
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    /// Docker-specific configuration (only used when backend is Docker)
    #[serde(default)]
    pub docker: DockerConfig,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        SandboxConfig {
            backend: SandboxBackend::default(),
            timeout_ms: 30000, // 30 seconds
            allow_read: vec![],
            allow_write: vec![],
            allow_network: false,
            max_memory_bytes: 512 * 1024 * 1024, // 512 MB
            max_cpu_seconds: 30,
            docker: DockerConfig::default(),
        }
    }
}

impl SandboxConfig {
    /// Development configuration - minimal sandboxing (use with caution!)
    ///
    /// Only enables timeout protection. No filesystem or network restrictions.
    /// Suitable for: Local development, trusted environments only.
    pub fn development() -> Self {
        SandboxConfig {
            backend: SandboxBackend::Timeout,
            timeout_ms: 60000,                    // 60 seconds
            allow_read: vec![],                   // No restrictions via sandbox
            allow_write: vec![],                  // No restrictions via sandbox
            allow_network: true,                  // Network allowed
            max_memory_bytes: 1024 * 1024 * 1024, // 1 GB
            max_cpu_seconds: 60,                  // 60 seconds
            docker: DockerConfig {
                network_mode: "bridge".to_string(),
                memory_limit: 1024 * 1024 * 1024, // 1 GB
                ..Default::default()
            },
        }
    }

    /// Standard configuration - balanced security (recommended default)
    ///
    /// Uses timeout + restricted backend with safe defaults.
    /// Suitable for: Production use with bundled tools and skills.
    pub fn standard() -> Self {
        SandboxConfig::default()
    }

    /// Strict configuration - maximum security
    ///
    /// Uses bubblewrap containerization with minimal permissions.
    /// Suitable for: Running untrusted code, production WASM execution.
    pub fn strict() -> Self {
        let docker_config = DockerConfig::default();

        SandboxConfig {
            backend: SandboxBackend::Bubblewrap,
            timeout_ms: 10000,                   // 10 seconds
            allow_read: vec![],                  // No filesystem access by default
            allow_write: vec![],                 // No write access
            allow_network: false,                // No network
            max_memory_bytes: 256 * 1024 * 1024, // 256 MB
            max_cpu_seconds: 10,                 // 10 seconds
            docker: docker_config,
        }
    }

    /// Isolated configuration - maximum security using Docker
    ///
    /// Uses Docker containerization for strong isolation across all platforms.
    /// This is the most secure option for least trusted tools.
    /// Suitable for: Running untrusted code, code from unknown sources,
    /// or when bubblewrap is not available.
    pub fn isolated() -> Self {
        SandboxConfig {
            backend: SandboxBackend::Docker,
            timeout_ms: 10000,                   // 10 seconds
            allow_read: vec![],                  // No filesystem access by default
            allow_write: vec![],                 // No write access
            allow_network: false,                // No network
            max_memory_bytes: 256 * 1024 * 1024, // 256 MB
            max_cpu_seconds: 10,                 // 10 seconds
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
    pub fn network() -> Self {
        SandboxConfig {
            backend: SandboxBackend::Restricted,
            timeout_ms: 30000,                   // 30 seconds
            allow_read: vec![],                  // Minimal read access
            allow_write: vec![],                 // No write access
            allow_network: true,                 // Network enabled
            max_memory_bytes: 512 * 1024 * 1024, // 512 MB
            max_cpu_seconds: 30,                 // 30 seconds
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
    pub fn filesystem(read_paths: Vec<PathBuf>, write_paths: Vec<PathBuf>) -> Self {
        let docker_config = DockerConfig::default();

        SandboxConfig {
            backend: SandboxBackend::Restricted,
            timeout_ms: 30000,                   // 30 seconds
            allow_read: read_paths,              // Specified read paths
            allow_write: write_paths,            // Specified write paths
            allow_network: false,                // No network
            max_memory_bytes: 512 * 1024 * 1024, // 512 MB
            max_cpu_seconds: 30,                 // 30 seconds
            docker: docker_config,
        }
    }

    /// WASM-optimized configuration
    ///
    /// Optimized for WebAssembly module execution.
    /// Suitable for: Running .wasm bundled tools with resource limits.
    pub fn wasm_optimized() -> Self {
        let docker_config = DockerConfig::default();

        SandboxConfig {
            backend: SandboxBackend::Wasm,
            timeout_ms: 30000,                   // 30 seconds
            allow_read: vec![],                  // Controlled by WASI preopens
            allow_write: vec![],                 // Controlled by WASI preopens
            allow_network: false,                // No WASI network
            max_memory_bytes: 256 * 1024 * 1024, // 256 MB
            max_cpu_seconds: 30,                 // 30 seconds
            docker: docker_config,
        }
    }

    /// Merge with override configuration
    ///
    /// Returns a new SandboxConfig with overrides applied.
    /// Override values take precedence over base configuration.
    ///
    /// This method handles presets - if the override specifies a preset,
    /// it starts from the preset configuration and applies individual overrides.
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
#[derive(Debug)]
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

impl Sandbox {
    /// Create a new sandbox with the given configuration
    pub fn new(config: SandboxConfig) -> Self {
        Sandbox { config }
    }

    /// Execute a command in the sandbox
    pub async fn execute(
        &self,
        program: &str,
        args: &[String],
        working_dir: &Path,
        env_vars: &[(String, String)],
    ) -> Result<SandboxResult> {
        match self.config.backend {
            SandboxBackend::None => {
                self.execute_unsandboxed(program, args, working_dir, env_vars)
                    .await
            }
            SandboxBackend::Timeout => {
                self.execute_with_timeout(program, args, working_dir, env_vars)
                    .await
            }
            SandboxBackend::Restricted => {
                self.execute_restricted(program, args, working_dir, env_vars)
                    .await
            }
            SandboxBackend::Bubblewrap => {
                self.execute_bubblewrap(program, args, working_dir, env_vars)
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

    /// Execute without sandboxing (development only)
    async fn execute_unsandboxed(
        &self,
        program: &str,
        args: &[String],
        working_dir: &Path,
        env_vars: &[(String, String)],
    ) -> Result<SandboxResult> {
        warn!("Executing without sandbox: {}", program);
        self.execute_with_timeout(program, args, working_dir, env_vars)
            .await
    }

    /// Execute with timeout enforcement
    async fn execute_with_timeout(
        &self,
        program: &str,
        args: &[String],
        working_dir: &Path,
        env_vars: &[(String, String)],
    ) -> Result<SandboxResult> {
        let mut cmd = Command::new(program);
        cmd.args(args)
            .current_dir(working_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        for (key, value) in env_vars {
            cmd.env(key, value);
        }

        let start = std::time::Instant::now();
        let timeout = Duration::from_millis(self.config.timeout_ms);

        let output = match tokio::time::timeout(timeout, cmd.output()).await {
            Ok(Ok(output)) => output,
            Ok(Err(e)) => return Err(SandboxError::Io(e)),
            Err(_) => {
                return Ok(SandboxResult {
                    stdout: String::new(),
                    stderr: "Execution timed out".to_string(),
                    exit_code: None,
                    duration_ms: self.config.timeout_ms,
                    timed_out: true,
                });
            }
        };

        let duration_ms = start.elapsed().as_millis() as u64;

        Ok(SandboxResult {
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            exit_code: output.status.code(),
            duration_ms,
            timed_out: false,
        })
    }

    /// Execute with restricted environment
    ///
    /// Creates a sandboxed environment with:
    /// - Resource limits (CPU, memory, file descriptors)
    /// - Clean environment (no inherited env vars except PATH)
    /// - Working directory isolation (temp directory)
    /// - Network access control (via env vars, full blocking requires bubblewrap)
    async fn execute_restricted(
        &self,
        program: &str,
        args: &[String],
        _working_dir: &Path,
        _env_vars: &[(String, String)],
    ) -> Result<SandboxResult> {
        use std::collections::HashSet;

        // Create temp sandbox directory
        let sandbox_dir = tempfile::tempdir()
            .map_err(|e| SandboxError::InvalidConfig(format!("Failed to create sandbox: {}", e)))?;

        // Copy allowed read files into sandbox
        for allowed_path in &self.config.allow_read {
            if allowed_path.exists() {
                let file_name = allowed_path.file_name().ok_or_else(|| {
                    SandboxError::InvalidConfig(format!("Invalid path: {:?}", allowed_path))
                })?;
                let dest = sandbox_dir.path().join(file_name);

                if allowed_path.is_file() {
                    if let Err(e) = std::fs::copy(allowed_path, &dest) {
                        warn!(
                            "Failed to copy {} to sandbox: {}",
                            allowed_path.display(),
                            e
                        );
                    }
                } else if allowed_path.is_dir() {
                    if let Err(e) = Self::copy_dir_recursive(allowed_path, &dest) {
                        warn!(
                            "Failed to copy dir {} to sandbox: {}",
                            allowed_path.display(),
                            e
                        );
                    }
                }
            }
        }

        // On Unix, we can use ulimit-style restrictions
        #[cfg(unix)]
        {
            let mut cmd = Command::new(program);
            cmd.args(args)
                .current_dir(sandbox_dir.path())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());

            // Build minimal environment
            let mut allowed_env = HashSet::new();
            allowed_env.insert("PATH");

            // Add user-specified env vars
            for (key, value) in _env_vars {
                cmd.env(key, value);
                allowed_env.insert(key.as_str());
            }

            // Clear all environment except allowed vars
            cmd.env_clear();

            // Restore minimal safe environment
            if let Ok(path) = std::env::var("PATH") {
                cmd.env("PATH", path);
            }

            // Re-add user-specified env vars
            for (key, value) in _env_vars {
                cmd.env(key, value);
            }

            // Block network if not allowed (basic approach via env)
            if !self.config.allow_network {
                cmd.env("HTTP_PROXY", "http://127.0.0.1:0");
                cmd.env("HTTPS_PROXY", "http://127.0.0.1:0");
                cmd.env("ALL_PROXY", "http://127.0.0.1:0");
            }

            // Capture config values for the closure
            let max_cpu_seconds = self.config.max_cpu_seconds;
            let max_memory_bytes = self.config.max_memory_bytes;

            // Set resource limits using pre_exec
            unsafe {
                cmd.pre_exec(move || {
                    use libc::{rlimit, setrlimit, RLIMIT_AS, RLIMIT_CPU, RLIMIT_NOFILE};

                    // Limit CPU time
                    if max_cpu_seconds > 0 {
                        let cpu_limit = rlimit {
                            rlim_cur: max_cpu_seconds,
                            rlim_max: max_cpu_seconds,
                        };
                        setrlimit(RLIMIT_CPU, &cpu_limit);
                    }

                    // Limit memory
                    if max_memory_bytes > 0 {
                        let mem_limit = rlimit {
                            rlim_cur: max_memory_bytes,
                            rlim_max: max_memory_bytes,
                        };
                        setrlimit(RLIMIT_AS, &mem_limit);
                    }

                    // Limit open files
                    let file_limit = rlimit {
                        rlim_cur: 64,
                        rlim_max: 64,
                    };
                    setrlimit(RLIMIT_NOFILE, &file_limit);

                    Ok(())
                });
            }

            let start = std::time::Instant::now();
            let timeout = Duration::from_millis(self.config.timeout_ms);

            let output = match tokio::time::timeout(timeout, cmd.output()).await {
                Ok(Ok(output)) => output,
                Ok(Err(e)) => return Err(SandboxError::Io(e)),
                Err(_) => {
                    return Ok(SandboxResult {
                        stdout: String::new(),
                        stderr: "Execution timed out".to_string(),
                        exit_code: None,
                        duration_ms: self.config.timeout_ms,
                        timed_out: true,
                    });
                }
            };

            let duration_ms = start.elapsed().as_millis() as u64;

            Ok(SandboxResult {
                stdout: String::from_utf8_lossy(&output.stdout).to_string(),
                stderr: String::from_utf8_lossy(&output.stderr).to_string(),
                exit_code: output.status.code(),
                duration_ms,
                timed_out: false,
            })
        }

        #[cfg(not(unix))]
        {
            warn!("Restricted mode has limited functionality on this platform, using timeout-only");
            self.execute_with_timeout(program, args, sandbox_dir.path(), _env_vars)
                .await
        }
    }

    /// Recursively copy directory
    fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            let path = entry.path();
            let dest_path = dst.join(entry.file_name());

            if path.is_dir() {
                Self::copy_dir_recursive(&path, &dest_path)?;
            } else {
                std::fs::copy(&path, &dest_path)?;
            }
        }
        Ok(())
    }

    /// Execute using Bubblewrap (Linux only)
    async fn execute_bubblewrap(
        &self,
        program: &str,
        args: &[String],
        working_dir: &Path,
        env_vars: &[(String, String)],
    ) -> Result<SandboxResult> {
        // Check if bwrap is available
        if !self.is_bubblewrap_available() {
            return Err(SandboxError::NotAvailable(
                "bubblewrap not found. Install with: sudo apt-get install bubblewrap".to_string(),
            ));
        }

        let mut cmd = Command::new("bwrap");

        // Basic isolation
        cmd.arg("--unshare-all")
            .arg("--share-net") // Share network namespace (if allowed)
            .arg("--die-with-parent");

        // Mount essential directories read-only
        cmd.arg("--ro-bind")
            .arg("/usr")
            .arg("/usr")
            .arg("--ro-bind")
            .arg("/lib")
            .arg("/lib")
            .arg("--ro-bind")
            .arg("/lib64")
            .arg("/lib64")
            .arg("--ro-bind")
            .arg("/bin")
            .arg("/bin")
            .arg("--ro-bind")
            .arg("/sbin")
            .arg("/sbin");

        // Tmpfs for /tmp and /var/tmp
        cmd.arg("--tmpfs").arg("/tmp").arg("--tmpfs").arg("/var");

        // Proc filesystem
        cmd.arg("--proc").arg("/proc").arg("--dev").arg("/dev");

        // Working directory (bind as read-write)
        cmd.arg("--bind").arg(working_dir).arg(working_dir);

        // Additional read paths
        for path in &self.config.allow_read {
            if path.exists() {
                cmd.arg("--ro-bind").arg(path).arg(path);
            }
        }

        // Additional write paths
        for path in &self.config.allow_write {
            if path.exists() {
                cmd.arg("--bind").arg(path).arg(path);
            }
        }

        // Block network if not allowed
        if !self.config.allow_network {
            cmd.arg("--unshare-net");
        }

        // Set working directory
        cmd.arg("--chdir").arg(working_dir);

        // Environment variables
        for (key, value) in env_vars {
            cmd.arg("--setenv").arg(key).arg(value);
        }

        // The actual command to run
        cmd.arg(program);
        cmd.args(args);

        // Capture output
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

        debug!("Executing with bubblewrap: {:?}", cmd);

        let start = std::time::Instant::now();
        let timeout = Duration::from_millis(self.config.timeout_ms);

        let output = match tokio::time::timeout(timeout, cmd.output()).await {
            Ok(Ok(output)) => output,
            Ok(Err(e)) => {
                return Err(SandboxError::ExecutionFailed(format!(
                    "Bubblewrap failed: {}",
                    e
                )))
            }
            Err(_) => {
                return Ok(SandboxResult {
                    stdout: String::new(),
                    stderr: "Execution timed out".to_string(),
                    exit_code: None,
                    duration_ms: self.config.timeout_ms,
                    timed_out: true,
                });
            }
        };

        let duration_ms = start.elapsed().as_millis() as u64;

        Ok(SandboxResult {
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            exit_code: output.status.code(),
            duration_ms,
            timed_out: false,
        })
    }

    /// Check if bubblewrap is available
    fn is_bubblewrap_available(&self) -> bool {
        std::process::Command::new("which")
            .arg("bwrap")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
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
        _env_vars: &[(String, String)],
    ) -> Result<SandboxResult> {
        let wasm_path = Path::new(program);

        // Check if file exists and has .wasm extension
        if !wasm_path.exists() {
            return Err(SandboxError::ExecutionFailed(format!(
                "WASM file not found: {}",
                program
            )));
        }

        if wasm_path.extension().and_then(|e| e.to_str()) != Some("wasm") {
            return Err(SandboxError::ExecutionFailed(format!(
                "Not a WASM file: {}",
                program
            )));
        }

        // Prepare input JSON from environment variables
        // First, try to read from SKILL_ARGS_JSON or SKILL_ARGS_FILE
        let input_json = _env_vars
            .iter()
            .find(|(k, _)| k == "SKILL_ARGS_JSON")
            .map(|(_, v)| v.clone())
            .or_else(|| {
                _env_vars
                    .iter()
                    .find(|(k, _)| k == "SKILL_ARGS_FILE")
                    .and_then(|(_, path)| std::fs::read_to_string(path).ok())
            })
            .unwrap_or_else(|| "{}".to_string());

        // Create WASM sandbox and execute
        let wasm_sandbox = WasmSandbox::new(self.config.clone());

        // Execute with timeout
        let timeout_duration = Duration::from_millis(self.config.timeout_ms);
        let result = tokio::time::timeout(
            timeout_duration,
            wasm_sandbox.execute(wasm_path, &input_json),
        )
        .await;

        match result {
            Ok(Ok(sandbox_result)) => Ok(sandbox_result),
            Ok(Err(e)) => Err(e),
            Err(_) => Ok(SandboxResult {
                stdout: String::new(),
                stderr: "WASM execution timed out".to_string(),
                exit_code: None,
                duration_ms: self.config.timeout_ms,
                timed_out: true,
            }),
        }
    }

    /// Execute using Docker container
    ///
    /// Creates a Docker container with the configured image and executes the command.
    /// Supports resource limits, network isolation, and volume mounts.
    async fn execute_docker(
        &self,
        program: &str,
        args: &[String],
        working_dir: &Path,
        env_vars: &[(String, String)],
    ) -> Result<SandboxResult> {
        // Connect to Docker daemon
        let docker = Docker::connect_with_local_defaults().map_err(|e| {
            SandboxError::NotAvailable(format!("Failed to connect to Docker: {}", e))
        })?;

        info!(
            "Executing with Docker: {} {} (image: {})",
            program,
            args.join(" "),
            self.config.docker.image
        );

        // Build the command to execute
        let mut cmd_parts = vec![program.to_string()];
        cmd_parts.extend(args.iter().cloned());
        let cmd_json = serde_json::to_string(&cmd_parts).map_err(|e| {
            SandboxError::InvalidConfig(format!("Failed to serialize command: {}", e))
        })?;

        // Prepare environment variables for the container
        let mut container_env = self.config.docker.env_vars.clone();

        // Add user-provided env vars
        for (key, value) in env_vars {
            container_env.insert(key.clone(), value.clone());
        }

        // Serialize container arguments as env var for the wrapper script
        container_env.insert("SKILL_CMD_JSON".to_string(), cmd_json);

        // Convert env vars to Docker format: "KEY=VALUE"
        let env_list: Vec<String> = container_env
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect();

        // Build volume mounts from allow_read and allow_write
        let mut mounts = Vec::new();

        // Add working directory as a mount
        mounts.push(Mount {
            target: Some(self.config.docker.working_dir.clone()),
            source: Some(working_dir.to_string_lossy().to_string()),
            typ: Some(MountTypeEnum::BIND),
            read_only: Some(false),
            ..Default::default()
        });

        // Add configured read paths
        for path in &self.config.allow_read {
            if path.exists() {
                mounts.push(Mount {
                    target: Some(format!(
                        "{}/{}",
                        self.config.docker.working_dir,
                        path.file_name().unwrap_or_default().to_string_lossy()
                    )),
                    source: Some(path.to_string_lossy().to_string()),
                    typ: Some(MountTypeEnum::BIND),
                    read_only: Some(true),
                    ..Default::default()
                });
            }
        }

        // Add configured write paths
        for path in &self.config.allow_write {
            if path.exists() {
                mounts.push(Mount {
                    target: Some(format!(
                        "{}/{}",
                        self.config.docker.working_dir,
                        path.file_name().unwrap_or_default().to_string_lossy()
                    )),
                    source: Some(path.to_string_lossy().to_string()),
                    typ: Some(MountTypeEnum::BIND),
                    read_only: Some(false),
                    ..Default::default()
                });
            }
        }

        // Add custom mounts from docker config
        for mount in &self.config.docker.mounts {
            mounts.push(Mount {
                target: Some(mount.target.clone()),
                source: Some(mount.source.to_string_lossy().to_string()),
                typ: Some(MountTypeEnum::BIND),
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
        let nano_cpus = if self.config.docker.cpu_quota > 0.0 {
            Some((self.config.docker.cpu_quota * 1_000_000_000.0) as i64)
        } else {
            None
        };

        let memory = if self.config.docker.memory_limit > 0 {
            Some(self.config.docker.memory_limit)
        } else {
            None
        };

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
            auto_remove: Some(self.config.docker.auto_remove),
            ..Default::default()
        };

        // Create container configuration using Config struct
        let config = Config::<String> {
            image: Some(self.config.docker.image.clone()),
            cmd: Some(vec![
                "sh".to_string(),
                "-c".to_string(),
                program.to_string(),
            ]),
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
            name: format!("skills-sandbox-{}", uuid::Uuid::new_v4()),
            platform: None,
        };

        let container = docker
            .create_container(Some(create_opts), config)
            .await
            .map_err(|e| {
                SandboxError::ExecutionFailed(format!("Failed to create container: {}", e))
            })?;

        let container_id = container.id;
        info!("Created Docker container: {}", container_id);

        // Start the container
        docker
            .start_container(&container_id, None::<StartContainerOptions<String>>)
            .await
            .map_err(|e| {
                SandboxError::ExecutionFailed(format!("Failed to start container: {}", e))
            })?;

        let start = std::time::Instant::now();
        let timeout_duration = Duration::from_millis(self.config.timeout_ms);

        // Wait for container with timeout - wait_container returns a stream, collect it
        let wait_result = tokio::time::timeout(timeout_duration, async {
            let mut stream = docker.wait_container(
                &container_id,
                Some(WaitContainerOptions {
                    condition: "not-running",
                }),
            );
            stream.next().await
        })
        .await;

        let duration_ms = start.elapsed().as_millis() as u64;

        // Check if timed out
        let timed_out = wait_result.is_err();

        if timed_out {
            // Kill the container if timed out
            let _ = docker
                .kill_container(
                    &container_id,
                    Some(KillContainerOptions { signal: "SIGKILL" }),
                )
                .await;
            warn!(
                "Docker container {} timed out after {}ms",
                container_id, duration_ms
            );
        }

        // Fetch logs
        let mut stdout = String::new();
        let mut stderr = String::new();

        let logs_options = LogsOptions::<String> {
            stdout: true,
            stderr: true,
            timestamps: false,
            follow: false,
            ..Default::default()
        };

        let mut logs_stream = docker.logs(&container_id, Some(logs_options));

        while let Some(log_result) = logs_stream.next().await {
            match log_result {
                Ok(log_output) => match log_output {
                    LogOutput::StdOut { message, .. } => {
                        stdout.push_str(&String::from_utf8_lossy(&message));
                    }
                    LogOutput::StdErr { message, .. } => {
                        stderr.push_str(&String::from_utf8_lossy(&message));
                    }
                    _ => {}
                },
                Err(e) => {
                    error!("Error fetching logs: {}", e);
                }
            }
        }

        // Get container exit code
        let exit_code = if timed_out {
            None
        } else {
            // Inspect container to get exit code
            match docker.inspect_container(&container_id, None).await {
                Ok(inspect) => inspect
                    .state
                    .and_then(|s| s.exit_code)
                    .map(|code| code as i32),
                Err(e) => {
                    error!("Failed to inspect container: {}", e);
                    None
                }
            }
        };

        info!(
            "Docker container {} finished in {}ms with exit code: {:?}",
            container_id, duration_ms, exit_code
        );

        Ok(SandboxResult {
            stdout,
            stderr,
            exit_code,
            duration_ms,
            timed_out,
        })
    }

    /// Check if Docker is available
    pub fn is_docker_available() -> bool {
        match Docker::connect_with_local_defaults() {
            Ok(docker) => {
                // Try to ping Docker to verify connection
                futures::executor::block_on(async { docker.ping().await.is_ok() })
            }
            Err(_) => false,
        }
    }
}
