//! Sandboxing for bundled tool execution
//!
//! Provides multiple sandboxing backends:
//! - None: No sandboxing (development only)
//! - Timeout: Basic timeout enforcement
//! - Restricted: Limited filesystem/network access
//! - Bubblewrap: Linux container-based sandboxing
//! - WASM: WebAssembly-based sandboxing (future)

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use thiserror::Error;
use tokio::process::Command;
use tracing::{debug, warn};

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
    /// WASM runtime (future)
    #[allow(dead_code)]
    Wasm,
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
        }
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
            SandboxBackend::Wasm => Err(SandboxError::NotAvailable(
                "WASM backend not yet implemented".to_string(),
            )),
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
}
