#![cfg(unix)]

use skillsrs::execution::sandbox::{SandboxBackend, SandboxConfig, SandboxError};
use skillsrs::execution::wasm::WasmSandbox;
#[cfg(target_os = "linux")]
use std::path::Path;
use std::path::PathBuf;
#[cfg(target_os = "linux")]
use std::time::Duration;

static WORKERS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[cfg(target_os = "linux")]
type WorkerTask =
    tokio::task::JoinHandle<Result<skillsrs::execution::sandbox::SandboxResult, SandboxError>>;

fn fixture(source: &str) -> std::io::Result<(tempfile::TempDir, PathBuf)> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("worker.wasm");
    std::fs::write(&path, source)?;
    Ok((directory, path))
}

fn config(timeout_ms: u64) -> SandboxConfig {
    SandboxConfig {
        backend: SandboxBackend::Wasm,
        timeout_ms,
        max_cpu_seconds: 0,
        ..SandboxConfig::default()
    }
}

const PRIVATE_STDIO: &str = r#"(module
    (import "wasi_snapshot_preview1" "fd_read"
        (func $read (param i32 i32 i32 i32) (result i32)))
    (import "wasi_snapshot_preview1" "fd_write"
        (func $write (param i32 i32 i32 i32) (result i32)))
    (import "wasi_snapshot_preview1" "environ_sizes_get"
        (func $environment (param i32 i32) (result i32)))
    (memory (export "memory") 1)
    (data (i32.const 0) "\01\00\00\00")
    (data (i32.const 32) "private-out")
    (data (i32.const 48) "private-err")
    (data (i32.const 64) "\04\00\00\00\01\00\00\00")
    (data (i32.const 88) "\20\00\00\00\0b\00\00\00")
    (data (i32.const 104) "\30\00\00\00\0b\00\00\00")
    (func (export "malloc") (param i32) (result i32) i32.const 1024)
    (func (export "run") (param i32 i32) (result i32)
        i32.const 96 i32.const 100 call $environment
        if unreachable end
        i32.const 96 i32.load if unreachable end
        i32.const 100 i32.load if unreachable end
        i32.const 0 i32.const 64 i32.const 1 i32.const 80 call $read
        if unreachable end
        i32.const 1 i32.const 88 i32.const 1 i32.const 80 call $write
        if unreachable end
        i32.const 2 i32.const 104 i32.const 1 i32.const 80 call $write
        if unreachable end
        i32.const 0))"#;

const INFINITE: &str = r#"(module
    (memory (export "memory") 1)
    (func (export "malloc") (param i32) (result i32) i32.const 128)
    (func (export "run") (param i32 i32) (result i32)
        (loop $forever br $forever) i32.const 0))"#;

#[tokio::test]
async fn worker_keeps_guest_stdio_and_environment_private() -> Result<(), anyhow::Error> {
    let _serial = WORKERS.lock().await;
    let (_directory, path) = fixture(PRIVATE_STDIO)?;
    let output = WasmSandbox::new(config(10_000))
        .execute(&path, "{input}")
        .await?;
    anyhow::ensure!(
        output.stdout == "private-out{",
        "Unexpected worker stdout: {:?}",
        output.stdout
    );
    anyhow::ensure!(
        output.stderr == "private-err",
        "Unexpected worker stderr: {:?}",
        output.stderr
    );
    anyhow::ensure!(
        output.exit_code == Some(0),
        "Unexpected worker exit code: {:?}",
        output.exit_code
    );
    anyhow::ensure!(!output.timed_out, "Worker unexpectedly timed out");
    Ok(())
}

#[tokio::test]
async fn worker_enforces_total_output_budget() -> Result<(), anyhow::Error> {
    let _serial = WORKERS.lock().await;
    let (_directory, path) = fixture(PRIVATE_STDIO)?;
    let mut limited = config(10_000);
    limited.max_output_bytes = 22;
    anyhow::ensure!(
        matches!(
            WasmSandbox::new(limited).execute(&path, "{input}").await,
            Err(SandboxError::OutputLimitExceeded(22))
        ),
        "Worker did not enforce the 22-byte output limit"
    );
    let mut exact = config(10_000);
    exact.max_output_bytes = 23;
    let output = WasmSandbox::new(exact).execute(&path, "{input}").await?;
    anyhow::ensure!(
        output.stdout == "private-out{",
        "Unexpected exact-limit stdout: {:?}",
        output.stdout
    );
    anyhow::ensure!(
        output.stderr == "private-err",
        "Unexpected exact-limit stderr: {:?}",
        output.stderr
    );
    Ok(())
}

#[tokio::test]
async fn worker_bounds_returned_memory_before_copying() -> Result<(), anyhow::Error> {
    let _serial = WORKERS.lock().await;
    let (_directory, path) = fixture(
        r#"(module
        (memory (export "memory") 1)
        (data (i32.const 0) "\ff\ff\ff\7f")
        (func (export "malloc") (param i32) (result i32) i32.const 128)
        (func (export "run") (param i32 i32) (result i32) i32.const 0))"#,
    )?;
    let mut limited = config(10_000);
    limited.max_output_bytes = 32;
    anyhow::ensure!(
        matches!(
            WasmSandbox::new(limited).execute(&path, "{}").await,
            Err(SandboxError::OutputLimitExceeded(32))
        ),
        "Worker did not reject returned memory exceeding 32 bytes"
    );
    Ok(())
}

#[cfg(target_os = "linux")]
fn worker_processes() -> std::io::Result<std::collections::BTreeSet<u32>> {
    let mut workers = std::collections::BTreeSet::new();
    for thread in std::fs::read_dir("/proc/self/task")? {
        let children = match std::fs::read_to_string(thread?.path().join("children")) {
            Ok(children) => children,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        for child in children.split_whitespace() {
            let Ok(pid) = child.parse::<u32>() else {
                continue;
            };
            let Ok(command) = std::fs::read(format!("/proc/{pid}/cmdline")) else {
                continue;
            };
            if command
                .split(|byte| *byte == 0)
                .any(|argument| argument == b"__wasm-worker")
            {
                workers.insert(pid);
            }
        }
    }
    Ok(workers)
}

#[cfg(target_os = "linux")]
async fn new_worker(previous: &std::collections::BTreeSet<u32>) -> Result<u32, anyhow::Error> {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(pid) = worker_processes()?.difference(previous).next() {
                return Ok::<_, std::io::Error>(*pid);
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await?
    .map_err(Into::into)
}

#[cfg(target_os = "linux")]
async fn terminated(pid: u32) -> Result<(), anyhow::Error> {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
                Ok(status) => {
                    if status.rsplit_once(')').is_some_and(|(_, state)| {
                        state
                            .trim_start()
                            .chars()
                            .next()
                            .is_some_and(|state| matches!(state, 'Z' | 'X'))
                    }) {
                        return Ok::<_, std::io::Error>(());
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(error) => return Err(error),
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await??;
    Ok(())
}

#[cfg(target_os = "linux")]
fn expensive_module() -> String {
    let mut source = String::from("(module (memory (export \"memory\") 1) ");
    for _ in 0..40_000 {
        source.push_str("(func (param i32) (result i32) local.get 0 i32.const 1 i32.add) ");
    }
    source.push_str("(func (export \"malloc\") (param i32) (result i32) i32.const 128) ");
    source.push_str("(func (export \"run\") (param i32 i32) (result i32) i32.const 0))");
    source
}

#[cfg(target_os = "linux")]
async fn start_worker(
    path: &Path,
    settings: SandboxConfig,
) -> Result<(u32, WorkerTask), anyhow::Error> {
    let previous = worker_processes()?;
    let path = path.to_path_buf();
    let task = tokio::spawn(async move { WasmSandbox::new(settings).execute(&path, "{}").await });
    let pid = match new_worker(&previous).await {
        Ok(pid) => pid,
        Err(error) => {
            task.abort();
            let _ = task.await;
            return Err(error);
        }
    };
    Ok((pid, task))
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn expensive_compilation_timeout_terminates_the_worker() -> Result<(), anyhow::Error> {
    let _serial = WORKERS.lock().await;
    let (_directory, path) = fixture(&expensive_module())?;
    let (pid, task) = start_worker(&path, config(1_000)).await?;
    let environment = std::fs::read(format!("/proc/{pid}/environ"))?;
    let mut entries: Vec<_> = environment
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
        .collect();
    entries.sort_unstable();
    anyhow::ensure!(
        entries
            == [
                b"LANG=C.UTF-8".as_slice(),
                b"TOKIO_WORKER_THREADS=1".as_slice()
            ],
        "Unexpected worker environment: {entries:?}"
    );
    let limits = std::fs::read_to_string(format!("/proc/{pid}/limits"))?;
    anyhow::ensure!(
        limits
            .lines()
            .any(|line| line.starts_with("Max address space") && line.contains("1610612736")),
        "Worker address-space limit is missing: {limits}"
    );
    anyhow::ensure!(
        limits
            .lines()
            .any(|line| line.starts_with("Max cpu time") && line.contains("60")),
        "Worker CPU-time limit is missing: {limits}"
    );
    let output = tokio::time::timeout(Duration::from_secs(5), task).await???;
    anyhow::ensure!(output.timed_out, "Expensive compilation did not time out");
    terminated(pid).await?;
    Ok(())
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn cancelling_expensive_compilation_terminates_the_worker() -> Result<(), anyhow::Error> {
    let _serial = WORKERS.lock().await;
    let (_directory, path) = fixture(&expensive_module())?;
    let (pid, task) = start_worker(&path, config(30_000)).await?;
    tokio::time::sleep(Duration::from_millis(100)).await;
    anyhow::ensure!(
        !task.is_finished(),
        "expensive compilation must still be active before cancellation"
    );
    task.abort();
    anyhow::ensure!(
        task.await.is_err_and(|error| error.is_cancelled()),
        "Compilation task was not cancelled"
    );
    terminated(pid).await?;
    Ok(())
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn cancelling_infinite_execution_terminates_the_worker() -> Result<(), anyhow::Error> {
    let _serial = WORKERS.lock().await;
    let (_directory, path) = fixture(INFINITE)?;
    let (pid, task) = start_worker(&path, config(30_000)).await?;
    tokio::time::sleep(Duration::from_millis(250)).await;
    anyhow::ensure!(
        !task.is_finished(),
        "Infinite execution finished before cancellation"
    );
    task.abort();
    anyhow::ensure!(
        task.await.is_err_and(|error| error.is_cancelled()),
        "Infinite execution task was not cancelled"
    );
    terminated(pid).await?;
    Ok(())
}

#[tokio::test]
async fn infinite_execution_timeout_is_reported() -> Result<(), anyhow::Error> {
    let _serial = WORKERS.lock().await;
    let (_directory, path) = fixture(INFINITE)?;
    let output = WasmSandbox::new(config(250)).execute(&path, "{}").await?;
    anyhow::ensure!(output.timed_out, "Infinite execution did not time out");
    Ok(())
}
