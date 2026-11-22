//! WASM Sandbox Implementation
//!
//! Provides WebAssembly-based sandboxing using wasmtime:
//! - Memory limits enforcement via wasmtime config
//! - CPU/fuel limits for execution time
//! - Controlled filesystem access via WASI preopens
//! - Network blocking (no WASI network capabilities)
//! - JSON argument passing to WASM functions
//! - Result extraction from WASM memory

use crate::execution::sandbox::{SandboxConfig, SandboxError, SandboxResult};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};
use wasmtime::{
    Config, Engine, Linker, Memory, Module, Store, StoreLimits, StoreLimitsBuilder, UpdateDeadline,
};
use wasmtime_wasi::{
    p1::{self, WasiP1Ctx},
    p2::pipe::{MemoryInputPipe, MemoryOutputPipe},
    FsPerms, WasiCtxBuilder,
};

const MAX_REQUEST_BYTES: u64 = 16 * 1024 * 1024;
const COMPILER_MEMORY_BYTES: u64 = 1024 * 1024 * 1024;
const DEFAULT_GUEST_MEMORY_BYTES: u64 = 512 * 1024 * 1024;
const DEFAULT_CPU_SECONDS: u64 = 60;
const MAX_DIAGNOSTIC_BYTES: u64 = 64 * 1024;

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum WorkerOperation {
    #[default]
    Execute,
    Inspect,
}

#[derive(Serialize, Deserialize)]
struct WorkerRequest {
    path: PathBuf,
    input: String,
    config: SandboxConfig,
    #[serde(default)]
    operation: WorkerOperation,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
enum WorkerError {
    ExecutionFailed(String),
    InvalidConfig(String),
    NotAvailable(String),
    OutputLimitExceeded(u64),
}

impl From<SandboxError> for WorkerError {
    fn from(error: SandboxError) -> Self {
        match error {
            SandboxError::InvalidConfig(message) => Self::InvalidConfig(message),
            SandboxError::NotAvailable(message) => Self::NotAvailable(message),
            SandboxError::OutputLimitExceeded(limit) => Self::OutputLimitExceeded(limit),
            error => Self::ExecutionFailed(error.to_string()),
        }
    }
}

impl From<WorkerError> for SandboxError {
    fn from(error: WorkerError) -> Self {
        match error {
            WorkerError::ExecutionFailed(message) => Self::ExecutionFailed(message),
            WorkerError::InvalidConfig(message) => Self::InvalidConfig(message),
            WorkerError::NotAvailable(message) => Self::NotAvailable(message),
            WorkerError::OutputLimitExceeded(limit) => Self::OutputLimitExceeded(limit),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum WorkerReply {
    Success(SandboxResult),
    Failure { error: WorkerError },
}

struct WorkerProcess {
    child: Child,
    #[cfg(unix)]
    group: i32,
    armed: bool,
}

impl Drop for WorkerProcess {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        #[cfg(unix)]
        {
            // The worker cannot spawn native children through WASI or leave this group.
            unsafe { libc::kill(self.group, libc::SIGKILL) };
        }
        let _ = self.child.start_kill();
    }
}

const fn cpu_limit(config: &SandboxConfig) -> u64 {
    if config.max_cpu_seconds == 0 {
        DEFAULT_CPU_SECONDS
    } else {
        config.max_cpu_seconds
    }
}

fn wall_limit(config: &SandboxConfig) -> Duration {
    let cpu_ms = cpu_limit(config).saturating_mul(1000);
    Duration::from_millis(if config.timeout_ms == 0 {
        cpu_ms
    } else {
        config.timeout_ms.min(cpu_ms)
    })
}

fn protocol_limit(config: &SandboxConfig) -> Result<u64, SandboxError> {
    // JSON can expand one output byte to six bytes (for example, a NUL escape).
    config
        .max_output_bytes
        .checked_mul(6)
        .and_then(|bytes| bytes.checked_add(MAX_DIAGNOSTIC_BYTES))
        .ok_or_else(|| SandboxError::InvalidConfig("WASM output limit is too large".to_string()))
}

fn worker_executable() -> Result<PathBuf, SandboxError> {
    let unavailable = |message| {
        SandboxError::NotAvailable(format!(
        "WASM worker unavailable: {message}; set SKILLS_WASM_WORKER to an absolute skills executable"
    ))
    };
    if let Some(path) = std::env::var_os("SKILLS_WASM_WORKER") {
        let path = PathBuf::from(path);
        if !path.is_absolute() {
            return Err(unavailable(
                "SKILLS_WASM_WORKER must be absolute".to_string(),
            ));
        }
        let path = path
            .canonicalize()
            .map_err(|error| unavailable(error.to_string()))?;
        return if path.is_file() {
            Ok(path)
        } else {
            Err(unavailable("SKILLS_WASM_WORKER is not a file".to_string()))
        };
    }
    let current = std::env::current_exe().map_err(|error| unavailable(error.to_string()))?;
    if current.file_stem().is_some_and(|name| name == "skills") {
        return Ok(current);
    }
    if let Some(dependencies) = current.parent().filter(|path| path.ends_with("deps")) {
        if let Some(directory) = dependencies.parent() {
            let sibling = directory.join(format!("skills{}", std::env::consts::EXE_SUFFIX));
            if sibling.is_file() {
                return sibling
                    .canonicalize()
                    .map_err(|error| unavailable(error.to_string()));
            }
        }
    }
    Err(unavailable(
        "no skills executable alongside the library test".to_string(),
    ))
}

#[cfg(unix)]
fn resource_limits(config: &SandboxConfig) -> Result<(libc::rlimit, libc::rlimit), SandboxError> {
    let guest = if config.max_memory_bytes == 0 {
        DEFAULT_GUEST_MEMORY_BYTES
    } else {
        config.max_memory_bytes
    };
    let memory = guest.checked_add(COMPILER_MEMORY_BYTES).ok_or_else(|| {
        SandboxError::InvalidConfig("WASM process memory limit overflow".to_string())
    })?;
    let memory = libc::rlim_t::try_from(memory)
        .map_err(|error| SandboxError::InvalidConfig(error.to_string()))?;
    let cpu = libc::rlim_t::try_from(cpu_limit(config))
        .map_err(|error| SandboxError::InvalidConfig(error.to_string()))?;
    Ok((
        libc::rlimit {
            rlim_cur: memory,
            rlim_max: memory,
        },
        libc::rlimit {
            rlim_cur: cpu,
            rlim_max: cpu,
        },
    ))
}

#[cfg(unix)]
fn apply_resource_limits(memory: libc::rlimit, cpu: libc::rlimit) -> std::io::Result<()> {
    let core = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // This function also runs after fork; keep it allocation-free and signal-safe.
    for (resource, limit) in [
        (libc::RLIMIT_AS, memory),
        (libc::RLIMIT_CPU, cpu),
        (libc::RLIMIT_CORE, core),
    ] {
        if unsafe { libc::setrlimit(resource, &raw const limit) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

fn worker_command(request: &WorkerRequest) -> Result<Command, SandboxError> {
    #[cfg(not(unix))]
    return Err(SandboxError::NotAvailable(
        "WASM workers require Unix process resource limits on this platform".to_string(),
    ));
    #[cfg(unix)]
    {
        let (memory, cpu) = resource_limits(&request.config)?;
        let mut command = Command::new(worker_executable()?);
        command
            .arg("__wasm-worker")
            .env_clear()
            .env("LANG", "C.UTF-8")
            .env("TOKIO_WORKER_THREADS", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .process_group(0);
        // Only signal-safe resource-limit syscalls run between fork and exec.
        unsafe {
            command.pre_exec(move || apply_resource_limits(memory, cpu));
        }
        Ok(command)
    }
}

async fn read_protocol(
    stream: impl AsyncRead + Unpin,
    limit: u64,
) -> Result<Vec<u8>, SandboxError> {
    let mut bytes = Vec::new();
    stream
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .await?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > limit {
        return Err(SandboxError::ExecutionFailed(
            "WASM worker exceeded its bounded protocol channel".to_string(),
        ));
    }
    Ok(bytes)
}

async fn execute_worker(request: WorkerRequest) -> Result<SandboxResult, SandboxError> {
    let start = Instant::now();
    let limit = protocol_limit(&request.config)?;
    let wall = wall_limit(&request.config);
    let deadline = start.checked_add(wall).ok_or_else(|| {
        SandboxError::InvalidConfig("WASM deadline is outside the supported range".to_string())
    })?;
    let encoded = serde_json::to_vec(&request)
        .map_err(|error| SandboxError::InvalidConfig(error.to_string()))?;
    if u64::try_from(encoded.len()).unwrap_or(u64::MAX) > MAX_REQUEST_BYTES {
        return Err(SandboxError::InvalidConfig(
            "WASM worker request exceeds 16 MiB".to_string(),
        ));
    }
    let child = worker_command(&request)?.spawn()?;
    #[cfg(unix)]
    let group = child
        .id()
        .and_then(|id| i32::try_from(id).ok())
        .and_then(i32::checked_neg)
        .filter(|group| *group < 0)
        .ok_or_else(|| {
            SandboxError::ExecutionFailed("WASM worker has no process ID".to_string())
        })?;
    let mut process = WorkerProcess {
        child,
        #[cfg(unix)]
        group,
        armed: true,
    };
    let mut stdin = process.child.stdin.take().ok_or_else(|| {
        SandboxError::ExecutionFailed("WASM worker stdin is unavailable".to_string())
    })?;
    let stdout = process.child.stdout.take().ok_or_else(|| {
        SandboxError::ExecutionFailed("WASM worker stdout is unavailable".to_string())
    })?;
    let stderr = process.child.stderr.take().ok_or_else(|| {
        SandboxError::ExecutionFailed("WASM worker stderr is unavailable".to_string())
    })?;
    let exchange = async {
        tokio::try_join!(
            async {
                stdin.write_all(&encoded).await?;
                stdin.shutdown().await?;
                drop(stdin);
                Ok::<_, SandboxError>(())
            },
            read_protocol(stdout, limit),
            read_protocol(stderr, MAX_DIAGNOSTIC_BYTES),
            async { process.child.wait().await.map_err(SandboxError::from) },
        )
    };
    let exchange =
        tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), exchange).await;
    let ((), stdout, stderr, status) = if let Ok(result) = exchange {
        result?
    } else {
        #[cfg(unix)]
        unsafe {
            libc::kill(process.group, libc::SIGKILL)
        };
        let _ = process.child.start_kill();
        process.child.wait().await?;
        process.armed = false;
        return Ok(SandboxResult {
            stdout: String::new(),
            stderr: "WASM compilation or execution timed out".to_string(),
            exit_code: None,
            duration_ms: u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
            timed_out: true,
        });
    };
    process.armed = false;
    finish_worker(&stdout, &stderr, status, start)
}

fn finish_worker(
    stdout: &[u8],
    stderr: &[u8],
    status: std::process::ExitStatus,
    start: Instant,
) -> Result<SandboxResult, SandboxError> {
    if !status.success() {
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            if status
                .signal()
                .is_some_and(|signal| signal == libc::SIGXCPU || signal == libc::SIGKILL)
            {
                return Ok(SandboxResult {
                    stdout: String::new(),
                    stderr: "WASM worker exhausted its process resource budget".to_string(),
                    exit_code: None,
                    duration_ms: u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
                    timed_out: true,
                });
            }
        }
        return Err(SandboxError::ExecutionFailed(format!(
            "WASM worker exited with {status}: {}",
            String::from_utf8_lossy(stderr)
        )));
    }
    match serde_json::from_slice::<WorkerReply>(stdout).map_err(|error| {
        SandboxError::ExecutionFailed(format!("Invalid WASM worker reply: {error}"))
    })? {
        WorkerReply::Success(mut result) => {
            result.duration_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
            Ok(result)
        }
        WorkerReply::Failure { error } => Err(error.into()),
    }
}

/// Handle the private `skills __wasm-worker` stdin/stdout protocol.
///
/// # Errors
/// Returns protocol or process-limit setup failures. Guest failures are encoded in the reply.
pub async fn run_worker() -> Result<(), SandboxError> {
    let input = read_protocol(tokio::io::stdin(), MAX_REQUEST_BYTES).await?;
    let request: WorkerRequest = serde_json::from_slice(&input).map_err(|error| {
        SandboxError::InvalidConfig(format!("Invalid WASM worker request: {error}"))
    })?;
    #[cfg(not(unix))]
    return Err(SandboxError::NotAvailable(
        "WASM workers require Unix process resource limits on this platform".to_string(),
    ));
    #[cfg(unix)]
    {
        let (memory, cpu) = resource_limits(&request.config)?;
        apply_resource_limits(memory, cpu)?;
        let limit = protocol_limit(&request.config)?;
        let result = match request.operation {
            WorkerOperation::Execute => {
                WasmSandbox::new(request.config)
                    .execute_in_worker(&request.path, &request.input)
                    .await
            }
            WorkerOperation::Inspect => {
                WasmSandbox::inspect_in_worker(&request.path).and_then(|info| {
                    Ok(SandboxResult {
                        stdout: serde_json::to_string(&info)
                            .map_err(|error| SandboxError::ExecutionFailed(error.to_string()))?,
                        stderr: String::new(),
                        exit_code: Some(0),
                        duration_ms: 0,
                        timed_out: false,
                    })
                })
            }
        };
        let reply = match result {
            Ok(result) => WorkerReply::Success(result),
            Err(error) => WorkerReply::Failure {
                error: error.into(),
            },
        };
        let output = serde_json::to_vec(&reply)
            .map_err(|error| SandboxError::ExecutionFailed(error.to_string()))?;
        if u64::try_from(output.len()).unwrap_or(u64::MAX) > limit {
            return Err(SandboxError::ExecutionFailed(
                "WASM worker reply exceeds protocol limit".to_string(),
            ));
        }
        let mut stdout = tokio::io::stdout();
        stdout.write_all(&output).await?;
        stdout.flush().await?;
        Ok(())
    }
}

/// WASM sandbox for executing WebAssembly modules
pub struct WasmSandbox {
    config: SandboxConfig,
}

/// Input/output structure for WASM functions
#[derive(Serialize, Deserialize, Debug)]
pub struct WasmIo {
    /// Input JSON as string
    pub input: String,
    /// Output JSON as string
    pub output: Option<String>,
    /// Error message if any
    pub error: Option<String>,
}

/// WASM execution context using Wasmtime's `WASIp1` adapter.
pub struct WasmContext {
    wasi_ctx: WasiP1Ctx,
    limits: StoreLimits,
}

impl WasmContext {
    /// Create a new context with the given WASI configuration
    #[must_use]
    pub fn new(wasi_ctx: WasiP1Ctx) -> Self {
        Self {
            wasi_ctx,
            limits: StoreLimitsBuilder::new().build(),
        }
    }
}

struct InterruptOnDrop {
    engine: Engine,
    cancelled: Arc<AtomicBool>,
}

impl Drop for InterruptOnDrop {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        self.engine.increment_epoch();
    }
}

struct EpochTicker {
    stop: Option<std::sync::mpsc::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl EpochTicker {
    fn start(engine: Engine) -> Self {
        let (stop, receive) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            while receive.recv_timeout(Duration::from_millis(10))
                == Err(std::sync::mpsc::RecvTimeoutError::Timeout)
            {
                engine.increment_epoch();
            }
        });
        Self {
            stop: Some(stop),
            thread: Some(thread),
        }
    }
}

impl Drop for EpochTicker {
    fn drop(&mut self) {
        self.stop.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl WasmSandbox {
    /// Create a new WASM sandbox with the given configuration
    #[must_use]
    pub const fn new(config: SandboxConfig) -> Self {
        Self { config }
    }

    /// Execute a WASM module with the given input arguments
    ///
    /// The WASM module should export a function:
    ///   fn run(input: &str) -> String
    ///
    /// Input and output are JSON strings.
    /// # Errors
    /// Returns an error if the operation cannot be completed.
    pub async fn execute(
        &self,
        wasm_path: &Path,
        input_json: &str,
    ) -> Result<SandboxResult, SandboxError> {
        if !wasm_path.is_file() {
            return Err(SandboxError::ExecutionFailed(format!(
                "WASM file not found: {}",
                wasm_path.display()
            )));
        }
        if u64::try_from(input_json.len()).unwrap_or(u64::MAX) > MAX_REQUEST_BYTES {
            return Err(SandboxError::InvalidConfig(
                "WASM worker request exceeds 16 MiB".to_string(),
            ));
        }
        execute_worker(WorkerRequest {
            path: wasm_path.canonicalize()?,
            input: input_json.to_string(),
            config: self.config.clone(),
            operation: WorkerOperation::Execute,
        })
        .await
    }

    async fn execute_in_worker(
        &self,
        wasm_path: &Path,
        input_json: &str,
    ) -> Result<SandboxResult, SandboxError> {
        let start = Instant::now();
        let mut engine_config = Config::new();
        engine_config
            .consume_fuel(self.config.max_cpu_seconds > 0)
            .epoch_interruption(true)
            .parallel_compilation(false)
            .memory_reservation(0)
            .memory_guard_size(0)
            .memory_reservation_for_growth(0);
        let engine = Engine::new(&engine_config)
            .map_err(|error| SandboxError::NotAvailable(error.to_string()))?;
        let cancelled = Arc::new(AtomicBool::new(false));
        let _interrupt = InterruptOnDrop {
            engine: engine.clone(),
            cancelled: cancelled.clone(),
        };
        let _ticker = EpochTicker::start(engine.clone());
        let deadline = Some(start.checked_add(wall_limit(&self.config)).ok_or_else(|| {
            SandboxError::InvalidConfig("WASM deadline is outside the supported range".to_string())
        })?);
        // Compilation blocks only this disposable worker, never the service runtime.
        let module = Module::from_file(&engine, wasm_path).map_err(|error| {
            SandboxError::ExecutionFailed(format!("Cannot load WASM module: {error}"))
        })?;
        let execution = Self::execute_guest(
            module,
            input_json,
            &self.config,
            &engine,
            &cancelled,
            start,
            deadline,
        );
        if let Some(deadline) = deadline {
            tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), execution)
                .await
                .unwrap_or_else(|_| {
                    Ok(SandboxResult {
                        stdout: String::new(),
                        stderr: "WASM execution timed out".to_string(),
                        exit_code: None,
                        duration_ms: u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
                        timed_out: true,
                    })
                })
        } else {
            execution.await
        }
    }

    async fn execute_guest(
        module: Module,
        input_json: &str,
        config: &SandboxConfig,
        engine: &Engine,
        cancelled: &Arc<AtomicBool>,
        start: Instant,
        deadline: Option<Instant>,
    ) -> Result<SandboxResult, SandboxError> {
        let pipe_capacity = usize::try_from(config.max_output_bytes)
            .ok()
            .and_then(|limit| limit.checked_add(1))
            .ok_or_else(|| {
                SandboxError::InvalidConfig("WASM output limit is too large".to_string())
            })?;
        let stdout = MemoryOutputPipe::new(pipe_capacity);
        let stderr = MemoryOutputPipe::new(pipe_capacity);
        let mut context = WasmContext::new(Self::guest_wasi(input_json, config, &stdout, &stderr)?);
        let mut limits = StoreLimitsBuilder::new().trap_on_grow_failure(true);
        if config.max_memory_bytes > 0 {
            let bytes = usize::try_from(config.max_memory_bytes)
                .map_err(|error| SandboxError::InvalidConfig(error.to_string()))?;
            limits = limits.memory_size(bytes).memories(1).table_elements(
                bytes
                    .checked_div(std::mem::size_of::<usize>())
                    .ok_or_else(|| {
                        SandboxError::InvalidConfig("Invalid WASM table element size".to_string())
                    })?,
            );
        }
        context.limits = limits.build();
        let mut store = Store::new(engine, context);
        store.limiter(|context| &mut context.limits);
        let interrupted = cancelled.clone();
        store.epoch_deadline_callback(move |_| {
            if interrupted.load(Ordering::Acquire)
                || deadline.is_some_and(|deadline| Instant::now() >= deadline)
            {
                wasmtime::bail!("WASM execution deadline exceeded or execution cancelled");
            }
            Ok(UpdateDeadline::Yield(1))
        });
        store.set_epoch_deadline(1);
        if config.max_cpu_seconds > 0 {
            store
                .set_fuel(config.max_cpu_seconds.saturating_mul(10_000_000))
                .map_err(|error| SandboxError::InvalidConfig(error.to_string()))?;
            store
                .fuel_async_yield_interval(Some(10_000))
                .map_err(|error| SandboxError::InvalidConfig(error.to_string()))?;
        }
        let mut linker = Linker::new(engine);
        p1::add_to_linker_async(&mut linker, |context: &mut WasmContext| {
            &mut context.wasi_ctx
        })
        .map_err(|error| SandboxError::ExecutionFailed(error.to_string()))?;
        let execution: wasmtime::Result<String> = async {
            if cancelled.load(Ordering::Acquire)
                || deadline.is_some_and(|deadline| Instant::now() >= deadline)
            {
                wasmtime::bail!("WASM execution deadline exceeded or execution cancelled");
            }
            let instance = linker.instantiate_async(&mut store, &module).await?;
            let memory = instance
                .get_memory(&mut store, "memory")
                .ok_or_else(|| wasmtime::Error::msg("WASM module must export memory"))?;
            let run = instance.get_typed_func::<(i32, i32), i32>(&mut store, "run")?;
            let input_pointer =
                Self::allocate_memory(&mut store, &instance, memory, input_json.len()).await?;
            memory.write(&mut store, input_pointer, input_json.as_bytes())?;
            let pointer = run
                .call_async(
                    &mut store,
                    (
                        i32::try_from(input_pointer)?,
                        i32::try_from(input_json.len())?,
                    ),
                )
                .await?;
            Self::read_string_from_memory(
                &store,
                memory,
                usize::try_from(pointer)?,
                config.max_output_bytes,
            )
            .map_err(Into::into)
        }
        .await;
        Self::finish_guest(
            execution, &stdout, &stderr, config, cancelled, start, deadline,
        )
    }

    fn finish_guest(
        execution: wasmtime::Result<String>,
        stdout: &MemoryOutputPipe,
        stderr: &MemoryOutputPipe,
        config: &SandboxConfig,
        cancelled: &AtomicBool,
        start: Instant,
        deadline: Option<Instant>,
    ) -> Result<SandboxResult, SandboxError> {
        let stdout_bytes = stdout.contents();
        let stderr_bytes = stderr.contents();
        let captured_bytes = u64::try_from(stdout_bytes.len())
            .unwrap_or(u64::MAX)
            .saturating_add(u64::try_from(stderr_bytes.len()).unwrap_or(u64::MAX));
        if captured_bytes > config.max_output_bytes {
            return Err(SandboxError::OutputLimitExceeded(config.max_output_bytes));
        }
        let mut captured_stdout = String::from_utf8_lossy(&stdout_bytes).into_owned();
        let mut captured_stderr = String::from_utf8_lossy(&stderr_bytes).into_owned();
        match execution {
            Ok(output) => {
                if u64::try_from(output.len()).unwrap_or(u64::MAX)
                    > config.max_output_bytes.saturating_sub(captured_bytes)
                {
                    return Err(SandboxError::OutputLimitExceeded(config.max_output_bytes));
                }
                captured_stdout.push_str(&output);
                Ok(SandboxResult {
                    stdout: captured_stdout,
                    stderr: captured_stderr,
                    exit_code: Some(0),
                    duration_ms: u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
                    timed_out: false,
                })
            }
            Err(error) => {
                if matches!(
                    error.downcast_ref::<SandboxError>(),
                    Some(SandboxError::OutputLimitExceeded(_))
                ) || error
                    .to_string()
                    .contains("write beyond capacity of MemoryOutputPipe")
                {
                    return Err(SandboxError::OutputLimitExceeded(config.max_output_bytes));
                }
                let timed_out = cancelled.load(Ordering::Acquire)
                    || deadline.is_some_and(|deadline| Instant::now() >= deadline)
                    || error
                        .downcast_ref::<wasmtime::Trap>()
                        .is_some_and(|trap| *trap == wasmtime::Trap::OutOfFuel);
                if timed_out {
                    captured_stderr
                        .push_str("WASM execution timed out or exhausted its CPU budget");
                    Ok(SandboxResult {
                        stdout: captured_stdout,
                        stderr: captured_stderr,
                        exit_code: None,
                        duration_ms: u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
                        timed_out: true,
                    })
                } else {
                    Err(SandboxError::ExecutionFailed(format!(
                        "WASM execution failed: {error:#}"
                    )))
                }
            }
        }
    }

    fn guest_wasi(
        input_json: &str,
        config: &SandboxConfig,
        stdout: &MemoryOutputPipe,
        stderr: &MemoryOutputPipe,
    ) -> Result<WasiP1Ctx, SandboxError> {
        let mut wasi = WasiCtxBuilder::new();
        wasi.stdin(MemoryInputPipe::new(input_json.as_bytes().to_vec()))
            .stdout(stdout.clone())
            .stderr(stderr.clone());
        let mut directories = std::collections::BTreeMap::new();
        for path in &config.allow_read {
            directories.insert(path.canonicalize()?, false);
        }
        for path in &config.allow_write {
            directories.insert(path.canonicalize()?, true);
        }
        for (path, writable) in directories {
            let guest = path.to_str().ok_or_else(|| {
                SandboxError::InvalidConfig("WASI paths must be UTF-8".to_string())
            })?;
            let permissions = if writable {
                FsPerms::ReadWrite
            } else {
                FsPerms::ReadOnly
            };
            wasi.preopened_dir(&path, guest, permissions)
                .map_err(|error| {
                    SandboxError::InvalidConfig(format!(
                        "Cannot grant WASI directory {}: {error}",
                        path.display()
                    ))
                })?;
        }
        Ok(wasi.build_p1())
    }

    /// Allocate memory in the WASM module
    async fn allocate_memory(
        store: &mut Store<WasmContext>,
        instance: &wasmtime::Instance,
        memory: Memory,
        size: usize,
    ) -> Result<usize, SandboxError> {
        // Try to find a malloc export (common convention)
        if let Ok(malloc) = instance.get_typed_func::<i32, i32>(&mut *store, "malloc") {
            let allocation_size = i32::try_from(size).map_err(|_| {
                SandboxError::ExecutionFailed("WASM allocation exceeds i32::MAX".to_string())
            })?;
            let ptr = malloc
                .call_async(store, allocation_size)
                .await
                .map_err(|e| {
                    SandboxError::ExecutionFailed(format!("Memory allocation failed: {e}"))
                })?;
            return usize::try_from(ptr).map_err(|_| {
                SandboxError::ExecutionFailed(
                    "WASM allocator returned a negative pointer".to_string(),
                )
            });
        }

        // Alternative: Try to use memory.grow directly
        let current_size = memory.data_size(&*store);
        let needed_pages = size.div_ceil(65536); // WASM page size = 64KB
        let current_pages = current_size / 65536;

        let needed_pages = u64::try_from(needed_pages).map_err(|_| {
            SandboxError::ExecutionFailed("WASM page count exceeds u64::MAX".to_string())
        })?;
        memory.grow(store, needed_pages).map_err(|error| {
            SandboxError::ExecutionFailed(format!("Cannot allocate WASM input memory: {error}"))
        })?;
        current_pages
            .checked_mul(65_536)
            .ok_or_else(|| SandboxError::ExecutionFailed("WASM memory offset overflow".to_string()))
    }

    /// Read a length-prefixed string from WASM memory
    fn read_string_from_memory(
        store: &Store<WasmContext>,
        memory: Memory,
        ptr: usize,
        output_limit: u64,
    ) -> Result<String, SandboxError> {
        let memory_data = memory.data(store);

        // Read length-prefixed string (first 4 bytes as u32 length)
        let string_start = ptr.checked_add(4).ok_or_else(|| {
            SandboxError::ExecutionFailed("WASM output pointer overflow".to_string())
        })?;
        let length_bytes: [u8; 4] = memory_data
            .get(ptr..string_start)
            .and_then(|bytes| bytes.try_into().ok())
            .ok_or_else(|| SandboxError::ExecutionFailed("Invalid memory pointer".to_string()))?;
        let length = usize::try_from(u32::from_le_bytes(length_bytes)).map_err(|_| {
            SandboxError::ExecutionFailed("WASM output length exceeds usize::MAX".to_string())
        })?;
        if u64::try_from(length).unwrap_or(u64::MAX) > output_limit {
            return Err(SandboxError::OutputLimitExceeded(output_limit));
        }
        let string_end = string_start.checked_add(length).ok_or_else(|| {
            SandboxError::ExecutionFailed("WASM output range overflow".to_string())
        })?;
        let output_bytes = memory_data.get(string_start..string_end).ok_or_else(|| {
            SandboxError::ExecutionFailed("String exceeds memory bounds".to_string())
        })?;

        String::from_utf8(output_bytes.to_vec()).map_err(|e| {
            SandboxError::ExecutionFailed(format!("Invalid UTF-8 in WASM output: {e}"))
        })
    }

    /// Validate a WASM module without executing it
    /// # Errors
    /// Returns an error if the operation cannot be completed.
    pub fn validate(wasm_path: &Path) -> Result<(), SandboxError> {
        let info = Self::inspect(wasm_path)?;
        if !info.has_run_export {
            return Err(SandboxError::ExecutionFailed(
                "WASM module must export 'run' function".to_string(),
            ));
        }

        if !info.has_memory_export {
            return Err(SandboxError::ExecutionFailed(
                "WASM module must export 'memory'".to_string(),
            ));
        }

        Ok(())
    }

    /// Load and inspect a WASM module
    /// # Errors
    /// Returns an error if the operation cannot be completed.
    pub fn inspect(wasm_path: &Path) -> Result<WasmModuleInfo, SandboxError> {
        let request = WorkerRequest {
            path: wasm_path.canonicalize()?,
            input: String::new(),
            config: SandboxConfig::wasm_optimized(),
            operation: WorkerOperation::Inspect,
        };
        // Keep the synchronous inspection API usable from inside a Tokio runtime.
        let result = std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?
                .block_on(execute_worker(request))
        })
        .join()
        .map_err(|_| {
            SandboxError::ExecutionFailed("WASM inspection thread failed".to_string())
        })??;
        if result.timed_out {
            return Err(SandboxError::ExecutionFailed(
                "WASM inspection timed out".to_string(),
            ));
        }
        serde_json::from_str(&result.stdout).map_err(|error| {
            SandboxError::ExecutionFailed(format!("Invalid WASM inspection: {error}"))
        })
    }

    fn inspect_in_worker(wasm_path: &Path) -> Result<WasmModuleInfo, SandboxError> {
        let mut config = Config::new();
        config.parallel_compilation(false);
        let engine = Engine::new(&config).map_err(|e| {
            SandboxError::NotAvailable(format!("Failed to create wasmtime engine: {e}"))
        })?;

        let module = Module::from_file(&engine, wasm_path).map_err(|e| {
            SandboxError::ExecutionFailed(format!("Failed to load WASM module: {e}"))
        })?;

        let imports: Vec<String> = module.imports().map(|i| i.name().to_string()).collect();
        let exports: Vec<String> = module.exports().map(|e| e.name().to_string()).collect();

        Ok(WasmModuleInfo {
            imports,
            exports,
            has_run_export: module.get_export("run").is_some(),
            has_memory_export: module.get_export("memory").is_some(),
        })
    }
}

/// Information about a WASM module
#[derive(Debug, Serialize, Deserialize)]
pub struct WasmModuleInfo {
    /// Imported functions
    pub imports: Vec<String>,
    /// Exported functions
    pub exports: Vec<String>,
    /// Has the required 'run' export
    pub has_run_export: bool,
    /// Has the required 'memory' export
    pub has_memory_export: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wasm_module_info() {
        // This is a minimal valid WASM module info test
        let info = WasmModuleInfo {
            imports: vec![],
            exports: vec!["memory".to_string(), "run".to_string()],
            has_run_export: true,
            has_memory_export: true,
        };
        assert!(info.has_run_export);
        assert!(info.has_memory_export);
    }
}
