//! Tests for WASM functionality and Sandbox Configuration
//!
//! Comprehensive tests covering:
//! - WASM sandbox creation and execution
//! - Sandbox configuration (default, override, per-tool/server)
//! - `UpstreamConfig` with `sandbox_config`
//! - `CallableRecord` with `sandbox_config`
//! - Integration tests

use jiff::Timestamp;
use skillsrs::core::registry::Registry;
use skillsrs::core::{
    BundledTool, CallableId, CallableKind, CallableRecord, CostHints, RiskTier, SchemaDigest,
};
use skillsrs::execution::upstream::{UpstreamConfig, UpstreamManager};
use skillsrs::execution::wasm::WasmSandbox;
use skillsrs::execution::{
    sandbox::{Sandbox, SandboxBackend, SandboxConfig, SandboxConfigOverride, SandboxError},
    Runtime,
};
use std::path::PathBuf;
use std::sync::Arc;

// ============================================================================
// Helper Functions
// ============================================================================

/// Create a test tool record for use in tests
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
fn create_test_tool_record(name: &str, server: &str) -> CallableRecord {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "path": { "type": "string" }
        },
        "required": ["path"]
    });
    let digest = SchemaDigest::from_schema(&schema).unwrap();
    let id = CallableId::tool(server, name, digest.as_str());

    CallableRecord {
        id,
        kind: CallableKind::Tool,
        fq_name: format!("{server}.{name}"),
        name: name.to_string(),
        title: Some("Test Tool".to_string()),
        description: Some("A test tool".to_string()),
        tags: vec![],
        input_schema: schema,
        output_schema: None,
        schema_digest: digest,
        server_alias: Some(server.to_string()),
        upstream_tool_name: Some(name.to_string()),
        skill_version: None,
        uses: vec![],
        skill_directory: None,
        bundled_tools: vec![],
        additional_files: vec![],
        cost_hints: CostHints::default(),
        risk_tier: RiskTier::ReadOnly,
        last_seen: Timestamp::now(),
        sandbox_config: None,
    }
}

/// Create a test skill record with bundled tool
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
fn create_test_skill_with_bundled_tool(name: &str, tool_name: &str) -> CallableRecord {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "input": { "type": "string" }
        }
    });
    let digest = SchemaDigest::from_schema(&schema).unwrap();
    let id = CallableId::skill(name, "1.0.0");

    let bundled_tool = BundledTool {
        name: tool_name.to_string(),
        description: "Test bundled tool".to_string(),
        command: vec!["echo".to_string(), "test".to_string()],
        schema: schema.clone(),
    };

    CallableRecord {
        id,
        kind: CallableKind::Skill,
        fq_name: name.to_string(),
        name: name.to_string(),
        title: Some("Test Skill".to_string()),
        description: Some("A test skill with bundled tool".to_string()),
        tags: vec!["test".to_string()],
        input_schema: schema,
        output_schema: None,
        schema_digest: digest,
        server_alias: None,
        upstream_tool_name: None,
        skill_version: Some("1.0.0".to_string()),
        uses: vec![],
        skill_directory: Some(PathBuf::from("/tmp/test-skill")),
        bundled_tools: vec![bundled_tool],
        additional_files: vec![],
        cost_hints: CostHints::default(),
        risk_tier: RiskTier::ReadOnly,
        last_seen: Timestamp::now(),
        sandbox_config: None,
    }
}

#[cfg(unix)]
fn wasm_fixture(source: &str) -> std::io::Result<(tempfile::TempDir, PathBuf)> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("fixture.wasm");
    std::fs::write(&path, source)?;
    Ok((directory, path))
}

// ============================================================================
// WASM Sandbox Tests
// ============================================================================

#[cfg(unix)]
#[tokio::test]
async fn malformed_module_is_rejected_by_the_compilation_worker() -> Result<(), anyhow::Error> {
    let (_directory, path) = wasm_fixture("not a wasm module")?;
    let result = WasmSandbox::new(SandboxConfig {
        backend: SandboxBackend::Wasm,
        timeout_ms: 10_000,
        ..Default::default()
    })
    .execute(&path, "{}")
    .await;
    anyhow::ensure!(
        matches!(&result, Err(SandboxError::ExecutionFailed(_))),
        "Malformed module was not rejected as a compilation error: {result:?}"
    );
    Ok(())
}

#[cfg(not(unix))]
#[tokio::test]
async fn wasm_workers_fail_closed_without_process_resource_limits() -> Result<(), anyhow::Error> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("worker.wasm");
    std::fs::write(&path, "(module)")?;
    let result = WasmSandbox::new(SandboxConfig {
        backend: SandboxBackend::Wasm,
        ..Default::default()
    })
    .execute(&path, "{}")
    .await;
    anyhow::ensure!(
        matches!(&result, Err(SandboxError::NotAvailable(_))),
        "WASM worker did not reject unsupported process limits: {result:?}"
    );
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn test_wasm_sandbox_memory_limits() -> Result<(), anyhow::Error> {
    for source in [
        r#"(module (memory (export "memory") 2)
            (func (export "malloc") (param i32) (result i32) i32.const 128)
            (func (export "run") (param i32 i32) (result i32) i32.const 0))"#,
        r#"(module (memory (export "memory") 1)
            (func (export "malloc") (param i32) (result i32) i32.const 128)
            (func (export "run") (param i32 i32) (result i32) i32.const 1 memory.grow drop i32.const 0))"#,
    ] {
        let (_directory, path) = wasm_fixture(source)?;
        let sandbox = WasmSandbox::new(SandboxConfig {
            backend: SandboxBackend::Wasm,
            max_memory_bytes: 65_536,
            ..Default::default()
        });
        anyhow::ensure!(
            sandbox.execute(&path, "{}").await.is_err(),
            "Initial or grown guest memory exceeded the configured cap"
        );
    }
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn test_wasm_sandbox_timeout() -> Result<(), anyhow::Error> {
    let (_directory, path) = wasm_fixture(
        r#"(module
        (memory (export "memory") 1)
        (func (export "malloc") (param i32) (result i32) i32.const 128)
        (func (export "run") (param i32 i32) (result i32)
            (loop $spin br $spin) i32.const 0))"#,
    )?;
    let sandbox = WasmSandbox::new(SandboxConfig {
        backend: SandboxBackend::Wasm,
        timeout_ms: 50,
        max_cpu_seconds: 0,
        ..Default::default()
    });
    let output = sandbox.execute(&path, "{}").await?;
    anyhow::ensure!(output.timed_out, "Guest did not time out");
    anyhow::ensure!(
        output.duration_ms < 5000,
        "Guest timeout took {} ms",
        output.duration_ms
    );
    Ok(())
}

// ============================================================================
// Sandbox Configuration Tests
// ============================================================================

#[test]
fn test_sandbox_config_default() {
    let config = SandboxConfig::default();

    // Check default values
    assert_eq!(
        config.backend,
        if cfg!(target_os = "linux") {
            SandboxBackend::Bubblewrap
        } else {
            SandboxBackend::Docker
        }
    );
    assert_eq!(config.timeout_ms, 30000); // 30 seconds
    assert!(!config.allow_network);
    assert_eq!(config.max_memory_bytes, 512 * 1024 * 1024); // 512 MB
    assert_eq!(config.max_cpu_seconds, 30);
    assert_eq!(config.max_output_bytes, 8 * 1024 * 1024);
    assert!(config.allow_read.is_empty());
    assert!(config.allow_write.is_empty());
}

#[test]
fn test_sandbox_config_with_override() {
    let base = SandboxConfig::default();
    let override_config = SandboxConfigOverride {
        preset: None,
        timeout_ms: Some(60000),
        backend: Some(SandboxBackend::Wasm),
        ..Default::default()
    };

    let merged = base.with_override(&override_config);

    // Override values should be applied
    assert_eq!(merged.timeout_ms, 60000);
    assert_eq!(merged.backend, SandboxBackend::Wasm);

    // Non-overridden values should retain defaults
    assert!(!merged.allow_network);
}

#[test]
fn test_sandbox_config_with_full_override() {
    let base = SandboxConfig::default();
    let override_config = SandboxConfigOverride {
        preset: None,
        timeout_ms: Some(120_000),
        backend: Some(SandboxBackend::Restricted),
        allow_network: Some(true),
        max_memory_bytes: Some(1024 * 1024 * 1024), // 1 GB
        max_cpu_seconds: Some(60),
        max_output_bytes: Some(4096),
        allow_read: vec![PathBuf::from("/home/user/projects")],
        allow_write: vec![PathBuf::from("/tmp")],
    };

    let merged = base.with_override(&override_config);

    assert_eq!(merged.timeout_ms, 120_000);
    assert_eq!(merged.backend, SandboxBackend::Restricted);
    assert!(merged.allow_network);
    assert_eq!(merged.max_memory_bytes, 1024 * 1024 * 1024);
    assert_eq!(merged.max_cpu_seconds, 60);
    assert_eq!(merged.max_output_bytes, 4096);
    assert_eq!(merged.allow_read.len(), 1);
    assert_eq!(merged.allow_write.len(), 1);
}

#[test]
fn test_sandbox_config_for_tool() {
    let base = SandboxConfig::default();

    let server_override = SandboxConfigOverride {
        preset: None,
        timeout_ms: Some(45000),
        allow_network: Some(true),
        ..Default::default()
    };

    let tool_override = SandboxConfigOverride {
        preset: None,
        timeout_ms: Some(90000),
        max_memory_bytes: Some(1024 * 1024 * 1024),
        ..Default::default()
    };

    let merged = base.for_tool(Some(&tool_override), Some(&server_override));

    // Tool override should take precedence for timeout_ms
    assert_eq!(merged.timeout_ms, 90000);
    // Server override should be used for allow_network
    assert!(merged.allow_network);
    // Tool override should be used for max_memory_bytes
    assert_eq!(merged.max_memory_bytes, 1024 * 1024 * 1024);
    // Base default should be used for backend
    assert_eq!(merged.backend, base.backend);
}

#[test]
fn test_sandbox_config_for_tool_only() {
    let base = SandboxConfig::default();

    let tool_override = SandboxConfigOverride {
        preset: None,
        backend: Some(SandboxBackend::Wasm),
        ..Default::default()
    };

    let merged = base.for_tool(Some(&tool_override), None);

    assert_eq!(merged.backend, SandboxBackend::Wasm);
    assert_eq!(merged.timeout_ms, 30000); // Base default
}

#[test]
fn test_sandbox_config_for_server_only() {
    let base = SandboxConfig::default();

    let server_override = SandboxConfigOverride {
        preset: None,
        backend: Some(SandboxBackend::Bubblewrap),
        ..Default::default()
    };

    let merged = base.for_tool(None, Some(&server_override));

    assert_eq!(merged.backend, SandboxBackend::Bubblewrap);
    assert_eq!(merged.timeout_ms, 30000); // Base default
}

#[test]
fn test_sandbox_config_for_tool_precedence() {
    // Test precedence: base < server < tool
    let base = SandboxConfig::default(); // timeout_ms: 30000

    let server_override = SandboxConfigOverride {
        preset: None,
        timeout_ms: Some(60000),
        max_memory_bytes: Some(512 * 1024 * 1024),
        ..Default::default()
    };

    let tool_override = SandboxConfigOverride {
        preset: None,
        timeout_ms: Some(120_000),
        max_memory_bytes: Some(2048 * 1024 * 1024),
        ..Default::default()
    };

    let merged = base.for_tool(Some(&tool_override), Some(&server_override));

    // Tool should take precedence for both
    assert_eq!(merged.timeout_ms, 120_000);
    assert_eq!(merged.max_memory_bytes, 2048 * 1024 * 1024);
}

// ============================================================================
// SandboxConfigOverride Serialization Tests
// ============================================================================

#[test]
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
fn test_sandbox_config_override_deserialization() {
    let yaml = r"
        backend: wasm
        timeout_ms: 60000
        allow_network: true
        max_memory_bytes: 536870912
        max_cpu_seconds: 30
        allow_read:
          - /home/user/projects
          - /tmp/readonly
        allow_write:
          - /tmp
    ";

    let config: SandboxConfigOverride = serde_yaml::from_str(yaml).unwrap();

    assert_eq!(config.backend, Some(SandboxBackend::Wasm));
    assert_eq!(config.timeout_ms, Some(60000));
    assert_eq!(config.allow_network, Some(true));
    assert_eq!(config.max_memory_bytes, Some(536_870_912));
    assert_eq!(config.max_cpu_seconds, Some(30));
    assert_eq!(config.allow_read.len(), 2);
    assert_eq!(config.allow_write.len(), 1);
}

#[test]
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
fn test_sandbox_config_override_serialization() {
    let config = SandboxConfigOverride {
        preset: None,
        backend: Some(SandboxBackend::Restricted),
        timeout_ms: Some(45000),
        allow_network: Some(false),
        max_memory_bytes: None,
        max_cpu_seconds: None,
        max_output_bytes: Some(8192),
        allow_read: vec![PathBuf::from("/data")],
        allow_write: vec![],
    };

    let yaml = serde_yaml::to_string(&config).unwrap();

    assert!(yaml.contains("restricted"));
    assert!(yaml.contains("45000"));
    assert!(yaml.contains("false"));
    assert!(yaml.contains("/data"));
    assert!(yaml.contains("max_output_bytes: 8192"));
}

// ============================================================================
// Upstream Config with Sandbox Tests
// ============================================================================

#[test]
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
fn test_upstream_config_with_sandbox() {
    let yaml = r#"
        alias: test-server
        transport: stdio
        command: ["echo", "test"]
        tags: []
        sandbox_config:
          backend: restricted
          timeout_ms: 60000
          allow_network: false
          max_memory_bytes: 536870912
          allow_read:
            - /tmp
    "#;

    let config: UpstreamConfig = serde_yaml::from_str(yaml).unwrap();

    assert!(config.sandbox_config.is_some());
    let sandbox = config.sandbox_config.unwrap();
    assert_eq!(sandbox.timeout_ms, Some(60000));
    assert_eq!(sandbox.backend, Some(SandboxBackend::Restricted));
    assert_eq!(sandbox.allow_network, Some(false));
    assert_eq!(sandbox.max_memory_bytes, Some(536_870_912));
    assert_eq!(sandbox.allow_read.len(), 1);
}

#[test]
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
fn test_upstream_config_with_bubblewrap_sandbox() {
    let yaml = r#"
        alias: bubblewrap-server
        transport: stdio
        command: ["python", "script.py"]
        tags: []
        sandbox_config:
          backend: bubblewrap
          timeout_ms: 120000
          allow_network: true
          allow_read:
            - /usr
            - /lib
          allow_write:
            - /tmp/output
    "#;

    let config: UpstreamConfig = serde_yaml::from_str(yaml).unwrap();

    assert!(config.sandbox_config.is_some());
    let sandbox = config.sandbox_config.unwrap();
    assert_eq!(sandbox.backend, Some(SandboxBackend::Bubblewrap));
    assert_eq!(sandbox.timeout_ms, Some(120_000));
    assert_eq!(sandbox.allow_network, Some(true));
    assert_eq!(sandbox.allow_read.len(), 2);
    assert_eq!(sandbox.allow_write.len(), 1);
}

#[test]
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
fn test_upstream_config_with_wasm_sandbox() {
    let yaml = r#"
        alias: wasm-server
        transport: stdio
        command: ["wasmtime", "module.wasm"]
        tags: []
        sandbox_config:
          backend: wasm
          timeout_ms: 30000
          max_memory_bytes: 268435456
    "#;

    let config: UpstreamConfig = serde_yaml::from_str(yaml).unwrap();

    assert!(config.sandbox_config.is_some());
    let sandbox = config.sandbox_config.unwrap();
    assert_eq!(sandbox.backend, Some(SandboxBackend::Wasm));
    assert_eq!(sandbox.timeout_ms, Some(30000));
    assert_eq!(sandbox.max_memory_bytes, Some(268_435_456));
}

#[test]
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
fn test_upstream_config_without_sandbox() {
    let yaml = r#"
        alias: plain-server
        transport: stdio
        command: ["echo", "test"]
        tags: []
    "#;

    let config: UpstreamConfig = serde_yaml::from_str(yaml).unwrap();
    assert!(config.sandbox_config.is_none());
}

#[tokio::test]
async fn test_server_config_retrieval() {
    let registry = Arc::new(Registry::new());
    let manager = UpstreamManager::new(registry);

    // Initially no servers
    let servers = manager.list_servers().await;
    assert_eq!(servers.len(), 0);

    // get_server_config should return None for non-existent server
    let config = manager.get_server_config("nonexistent").await;
    assert!(config.is_none());
}

// ============================================================================
// CallableRecord with Sandbox Tests
// ============================================================================

#[test]
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
fn test_callable_record_with_sandbox_config() {
    // Test that CallableRecord can have sandbox_config
    let mut record = create_test_tool_record("test-tool", "test-server");

    record.sandbox_config = Some(SandboxConfigOverride {
        preset: None,
        backend: Some(SandboxBackend::Bubblewrap),
        timeout_ms: Some(30000),
        ..Default::default()
    });

    assert!(record.sandbox_config.is_some());
    let sandbox = record.sandbox_config.as_ref().unwrap();
    assert_eq!(sandbox.backend, Some(SandboxBackend::Bubblewrap));
    assert_eq!(sandbox.timeout_ms, Some(30000));
}

#[test]
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
fn test_callable_record_sandbox_serialization() {
    // Test serialization/deserialization of CallableRecord with sandbox
    let mut record = create_test_tool_record("test-tool", "test-server");
    record.sandbox_config = Some(SandboxConfigOverride {
        preset: None,
        backend: Some(SandboxBackend::Wasm),
        timeout_ms: Some(45000),
        max_memory_bytes: Some(1024 * 1024 * 512),
        ..Default::default()
    });

    // Serialize to JSON
    let json = serde_json::to_string(&record).unwrap();

    // Deserialize back
    let deserialized: CallableRecord = serde_json::from_str(&json).unwrap();

    assert!(deserialized.sandbox_config.is_some());
    let sandbox = deserialized.sandbox_config.as_ref().unwrap();
    assert_eq!(sandbox.backend, Some(SandboxBackend::Wasm));
    assert_eq!(sandbox.timeout_ms, Some(45000));
    assert_eq!(sandbox.max_memory_bytes, Some(1024 * 1024 * 512));
}

#[test]
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
fn test_skill_with_bundled_tool_sandbox() {
    // Test skill with bundled tool and sandbox config
    let mut record = create_test_skill_with_bundled_tool("test-skill", "bundled-tool");

    record.sandbox_config = Some(SandboxConfigOverride {
        preset: None,
        backend: Some(SandboxBackend::Restricted),
        allow_network: Some(true),
        ..Default::default()
    });

    assert!(!record.bundled_tools.is_empty());
    assert!(record.sandbox_config.is_some());

    let sandbox = record.sandbox_config.as_ref().unwrap();
    assert_eq!(sandbox.backend, Some(SandboxBackend::Restricted));
    assert_eq!(sandbox.allow_network, Some(true));
}

// ============================================================================
// Sandbox Backend Variants Tests
// ============================================================================

#[test]
fn test_sandbox_backend_variants() {
    // Test all sandbox backends
    let backends = vec![
        SandboxBackend::None,
        SandboxBackend::Timeout,
        SandboxBackend::Restricted,
        SandboxBackend::Bubblewrap,
        SandboxBackend::Wasm,
    ];

    for backend in backends {
        let config = SandboxConfig {
            backend,
            timeout_ms: 1000,
            ..Default::default()
        };

        let _sandbox = Sandbox::new(config);
        // Sandbox was created successfully

        // Just verify the struct was created
        // We can't easily verify internal state without additional methods
    }
}

#[test]
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
fn test_sandbox_backend_serialization() {
    // Test that all backends serialize/deserialize correctly
    let backends = vec![
        (SandboxBackend::None, "none"),
        (SandboxBackend::Timeout, "timeout"),
        (SandboxBackend::Restricted, "restricted"),
        (SandboxBackend::Bubblewrap, "bubblewrap"),
        (SandboxBackend::Wasm, "wasm"),
    ];

    for (backend, expected_str) in backends {
        let json = serde_json::to_string(&backend).unwrap();
        assert_eq!(json, format!("\"{expected_str}\""));

        let deserialized: SandboxBackend = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, backend);
    }
}

// ============================================================================
// Sandbox Execution Tests
// ============================================================================

#[cfg(unix)]
#[tokio::test]
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
async fn test_sandbox_execute_timeout() {
    let config = SandboxConfig {
        backend: SandboxBackend::Timeout,
        timeout_ms: 100, // Very short timeout
        allow_network: true,
        ..Default::default()
    };

    let sandbox = Sandbox::new(config);

    // Execute a slow command that should timeout
    // Using `sleep 1` which should take 1000ms, longer than our 100ms timeout
    let result = sandbox
        .execute("sleep", &["1".to_string()], std::path::Path::new("."), &[])
        .await;

    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(output.timed_out);
    assert!(output.stderr.contains("timed out") || output.stderr.contains("timeout"));
}

#[cfg(unix)]
#[tokio::test]
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
async fn test_unsandboxed_zero_timeout_does_not_apply_timeout_gate() {
    let sandbox = Sandbox::new(SandboxConfig {
        backend: SandboxBackend::None,
        timeout_ms: 0,
        allow_network: true,
        max_memory_bytes: 0,
        max_cpu_seconds: 0,
        ..Default::default()
    });
    let output = sandbox
        .execute(
            "printf",
            &["open".to_string()],
            std::path::Path::new("."),
            &[],
        )
        .await
        .unwrap();
    assert_eq!(output.stdout, "open");
    assert!(!output.timed_out);
}

#[cfg(unix)]
#[tokio::test]
async fn native_output_limit_kills_stdout_stderr_and_child_group() -> Result<(), anyhow::Error> {
    for backend in [
        SandboxBackend::None,
        SandboxBackend::Timeout,
        SandboxBackend::Restricted,
    ] {
        for redirect in ["", ">&2"] {
            let directory = tempfile::tempdir()?;
            let sandbox = Sandbox::new(SandboxConfig {
                backend,
                timeout_ms: 0,
                allow_network: true,
                max_memory_bytes: 0,
                max_cpu_seconds: 0,
                max_output_bytes: 1024,
                ..Default::default()
            });
            let result = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                sandbox.execute(
                    "sh",
                    &[
                        "-c".to_string(),
                        format!(
                            "(sleep 0.3; printf late > marker) & while :; do printf 12345678 {redirect}; done"
                        ),
                    ],
                    directory.path(),
                    &[],
                ),
            )
            .await?;
            anyhow::ensure!(
                matches!(result, Err(SandboxError::OutputLimitExceeded(1024))),
                "Native output did not exceed the 1024-byte limit"
            );
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            anyhow::ensure!(
                !directory.path().join("marker").exists(),
                "Native descendant survived the output limit"
            );
        }
    }
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn native_output_limit_counts_both_streams_and_accepts_exact_limit(
) -> Result<(), anyhow::Error> {
    for (limit, command, expected) in [
        (8, "printf 1234; printf 5678 >&2", Some(("1234", "5678"))),
        (7, "printf 1234; printf 5678 >&2", None),
        (0, ":", Some(("", ""))),
        (0, "printf x", None),
    ] {
        let sandbox = Sandbox::new(SandboxConfig {
            backend: SandboxBackend::Timeout,
            allow_network: true,
            max_output_bytes: limit,
            ..Default::default()
        });
        let result = sandbox
            .execute(
                "sh",
                &["-c".to_string(), command.to_string()],
                std::path::Path::new("."),
                &[],
            )
            .await;
        if let Some((stdout, stderr)) = expected {
            let output = result?;
            anyhow::ensure!(
                output.stdout == stdout,
                "Unexpected stdout: {:?}; expected {stdout:?}",
                output.stdout
            );
            anyhow::ensure!(
                output.stderr == stderr,
                "Unexpected stderr: {:?}; expected {stderr:?}",
                output.stderr
            );
            anyhow::ensure!(
                !output.timed_out,
                "Exact-limit execution unexpectedly timed out"
            );
        } else {
            anyhow::ensure!(
                matches!(result, Err(SandboxError::OutputLimitExceeded(value)) if value == limit),
                "Native output did not enforce the {limit}-byte limit"
            );
        }
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires a Docker daemon and the alpine:latest image"]
async fn docker_output_limit_kills_stdout_stderr_and_descendants() -> Result<(), anyhow::Error> {
    for redirect in ["", ">&2"] {
        let directory = tempfile::tempdir()?;
        let marker = directory.path().join("marker");
        let sandbox = Sandbox::new(SandboxConfig {
            backend: SandboxBackend::Docker,
            timeout_ms: 0,
            max_cpu_seconds: 0,
            max_output_bytes: 1024,
            allow_write: vec![directory.path().to_path_buf()],
            ..Default::default()
        });
        let args = [
            "-c".to_string(),
            format!(
                "(sleep 0.3; printf late > \"$1\") & while :; do printf 12345678 {redirect}; done"
            ),
            "sh".to_string(),
            marker.to_string_lossy().into_owned(),
        ];
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            sandbox.execute("sh", &args, directory.path(), &[]),
        )
        .await?;
        anyhow::ensure!(
            matches!(result, Err(SandboxError::OutputLimitExceeded(1024))),
            "Docker output did not exceed the 1024-byte limit"
        );
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        anyhow::ensure!(
            !marker.exists(),
            "Docker descendant survived the output limit"
        );
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires a Docker daemon and the alpine:latest image"]
async fn docker_disables_daemon_logs_and_preserves_bounded_capture() -> Result<(), anyhow::Error> {
    use bollard::query_parameters::{ListContainersOptions, RemoveContainerOptions};

    let token = uuid::Uuid::new_v4().to_string();
    let mut config = SandboxConfig {
        backend: SandboxBackend::Docker,
        max_output_bytes: 8,
        ..Default::default()
    };
    config.docker.auto_remove = false;
    let result = Sandbox::new(config)
        .execute(
            "sh",
            &[
                "-c".to_string(),
                "printf 1234; printf 5678 >&2; exit 3".to_string(),
                token.clone(),
            ],
            std::path::Path::new("."),
            &[],
        )
        .await;

    let docker = bollard::Docker::connect_with_local_defaults()?;
    let containers = docker
        .list_containers(Some(ListContainersOptions {
            all: true,
            filters: Some(std::collections::HashMap::from([(
                "name".to_string(),
                vec!["skills-sandbox-".to_string()],
            )])),
            ..Default::default()
        }))
        .await?;
    let id = containers
        .into_iter()
        .find_map(|container| {
            container
                .command
                .is_some_and(|command| command.contains(&token))
                .then_some(container.id)
                .flatten()
        })
        .ok_or_else(|| anyhow::anyhow!("Retained test container not found"))?;
    let inspection = docker.inspect_container(&id, None).await;
    docker
        .remove_container(
            &id,
            Some(RemoveContainerOptions {
                force: true,
                ..Default::default()
            }),
        )
        .await?;
    let inspection = inspection?;
    anyhow::ensure!(
        inspection
            .host_config
            .and_then(|host| host.log_config)
            .and_then(|log| log.typ)
            .as_deref()
            == Some("none"),
        "Docker daemon logging was not disabled"
    );
    let output = result?;
    anyhow::ensure!(
        output.stdout == "1234",
        "Unexpected Docker stdout: {:?}",
        output.stdout
    );
    anyhow::ensure!(
        output.stderr == "5678",
        "Unexpected Docker stderr: {:?}",
        output.stderr
    );
    anyhow::ensure!(
        output.exit_code == Some(3),
        "Unexpected Docker exit code: {:?}",
        output.exit_code
    );
    anyhow::ensure!(!output.timed_out, "Docker execution unexpectedly timed out");
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
async fn test_sandbox_execute_with_env_vars() {
    let config = SandboxConfig::development();
    let sandbox = Sandbox::new(config);

    let env_vars = vec![
        ("TEST_VAR1".to_string(), "value1".to_string()),
        ("TEST_VAR2".to_string(), "value2".to_string()),
    ];

    let result = sandbox
        .execute("env", &[], std::path::Path::new("."), &env_vars)
        .await;

    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(output.stdout.contains("TEST_VAR1=value1"));
    assert!(output.stdout.contains("TEST_VAR2=value2"));
}

#[tokio::test]
async fn test_sandbox_execute_nonexistent() {
    let config = SandboxConfig::development();
    let sandbox = Sandbox::new(config);

    // Execute a non-existent command
    let result = sandbox
        .execute(
            "this_command_definitely_does_not_exist",
            &[],
            std::path::Path::new("."),
            &[],
        )
        .await;

    // Should return an error (IO error for command not found)
    assert!(result.is_err());
}

// ============================================================================
// Integration Tests
// ============================================================================

#[tokio::test]
async fn test_runtime_with_custom_sandbox_config() {
    let registry = Arc::new(Registry::new());
    let upstream_manager = Arc::new(UpstreamManager::new(registry.clone()));

    // Create runtime with custom sandbox config
    let custom_config = SandboxConfig {
        backend: SandboxBackend::Timeout,
        timeout_ms: 5000,
        ..Default::default()
    };

    let _runtime = Runtime::with_sandbox_config(registry, upstream_manager, custom_config);

    // Runtime was created successfully
    // We can't easily verify internal config without getter methods
}

#[test]
fn test_sandbox_config_override_empty() {
    // Test that empty override doesn't change anything
    let base = SandboxConfig::default();
    let empty_override = SandboxConfigOverride::default();

    let merged = base.with_override(&empty_override);

    // All values should remain at defaults
    assert_eq!(merged.backend, base.backend);
    assert_eq!(merged.timeout_ms, 30000);
    assert!(!merged.allow_network);
}

#[test]
fn test_sandbox_config_chained_overrides() {
    // Test chained overrides
    let base = SandboxConfig::default();

    let first = SandboxConfigOverride {
        preset: None,
        timeout_ms: Some(60000),
        backend: Some(SandboxBackend::Restricted),
        ..Default::default()
    };

    let second = SandboxConfigOverride {
        preset: None,
        timeout_ms: Some(90000),
        allow_network: Some(true),
        ..Default::default()
    };

    // Apply first override
    let merged1 = base.with_override(&first);
    assert_eq!(merged1.timeout_ms, 60000);
    assert_eq!(merged1.backend, SandboxBackend::Restricted);
    assert!(!merged1.allow_network);

    // Apply second override to the result
    let merged2 = merged1.with_override(&second);
    assert_eq!(merged2.timeout_ms, 90000); // Second override wins
    assert_eq!(merged2.backend, SandboxBackend::Restricted); // From first
    assert!(merged2.allow_network); // From second
}

#[test]
fn test_sandbox_config_path_merging() {
    // Test that paths are merged, not replaced
    let base = SandboxConfig {
        allow_read: vec![PathBuf::from("/base/read")],
        allow_write: vec![PathBuf::from("/base/write")],
        ..Default::default()
    };

    let override_config = SandboxConfigOverride {
        preset: None,
        allow_read: vec![PathBuf::from("/override/read")],
        allow_write: vec![PathBuf::from("/override/write")],
        ..Default::default()
    };

    let merged = base.with_override(&override_config);

    // Both base and override paths should be present
    assert_eq!(merged.allow_read.len(), 2);
    assert_eq!(merged.allow_write.len(), 2);
    assert!(merged.allow_read.contains(&PathBuf::from("/base/read")));
    assert!(merged.allow_read.contains(&PathBuf::from("/override/read")));
}

// ============================================================================
// Error Handling Tests
// ============================================================================

#[cfg(unix)]
#[tokio::test]
async fn wasi_stdio_is_private_and_captured() -> Result<(), anyhow::Error> {
    let (_directory, path) = wasm_fixture(
        r#"(module
        (import "wasi_snapshot_preview1" "fd_write" (func $write (param i32 i32 i32 i32) (result i32)))
        (memory (export "memory") 1)
        (data (i32.const 0) "\40\00\00\00\0d\00\00\00")
        (data (i32.const 64) "private-stdio")
        (data (i32.const 128) "\02\00\00\00ok")
        (func (export "malloc") (param i32) (result i32) i32.const 512)
        (func (export "run") (param i32 i32) (result i32)
            i32.const 1 i32.const 0 i32.const 1 i32.const 16 call $write drop
            i32.const 2 i32.const 0 i32.const 1 i32.const 16 call $write drop
            i32.const 128))"#,
    )?;
    let result = WasmSandbox::new(SandboxConfig::wasm_optimized())
        .execute(&path, "{}")
        .await?;
    anyhow::ensure!(
        result.stdout == "private-stdiook",
        "Unexpected private stdout: {:?}",
        result.stdout
    );
    anyhow::ensure!(
        result.stderr == "private-stdio",
        "Unexpected private stderr: {:?}",
        result.stderr
    );
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn wasm_fuel_budget_stops_busy_guest_without_wall_timeout() -> Result<(), anyhow::Error> {
    let (_directory, path) = wasm_fixture(
        r#"(module
        (memory (export "memory") 1)
        (func (export "malloc") (param i32) (result i32) i32.const 128)
        (func (export "run") (param i32 i32) (result i32) (loop $spin br $spin) i32.const 0))"#,
    )?;
    let sandbox = WasmSandbox::new(SandboxConfig {
        backend: SandboxBackend::Wasm,
        timeout_ms: 0,
        max_cpu_seconds: 1,
        ..Default::default()
    });
    let result = sandbox.execute(&path, "{}").await?;
    anyhow::ensure!(result.timed_out, "Fuel budget did not stop the guest");
    anyhow::ensure!(
        result.exit_code.is_none(),
        "Fuel-exhausted guest returned an exit code: {:?}",
        result.exit_code
    );
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn wasi_stdin_contains_only_invocation_input() -> Result<(), anyhow::Error> {
    let (_directory, path) = wasm_fixture(
        r#"(module
        (import "wasi_snapshot_preview1" "fd_read" (func $read (param i32 i32 i32 i32) (result i32)))
        (memory (export "memory") 1)
        (data (i32.const 0) "\84\00\00\00\40\00\00\00")
        (func (export "malloc") (param i32) (result i32) i32.const 512)
        (func (export "run") (param i32 i32) (result i32)
            i32.const 0 i32.const 0 i32.const 1 i32.const 128 call $read drop
            i32.const 128))"#,
    )?;
    let input = r#"{"private":"input"}"#;
    let result = WasmSandbox::new(SandboxConfig::wasm_optimized())
        .execute(&path, input)
        .await?;
    anyhow::ensure!(
        result.stdout == input,
        "Guest stdin differed from invocation input: {:?}",
        result.stdout
    );
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn wasi_host_waits_are_cancelled_at_effective_deadline() -> Result<(), anyhow::Error> {
    let (_directory, path) = wasm_fixture(
        r#"(module
        (import "wasi_snapshot_preview1" "poll_oneoff" (func $poll (param i32 i32 i32 i32) (result i32)))
        (memory (export "memory") 1)
        (data (i32.const 16) "\01\00\00\00\00\00\00\00\00\ac\23\fc\06\00\00\00")
        (func (export "malloc") (param i32) (result i32) i32.const 512)
        (func (export "run") (param i32 i32) (result i32)
            i32.const 0 i32.const 64 i32.const 1 i32.const 128 call $poll drop
            i32.const 128))"#,
    )?;
    let sandbox = WasmSandbox::new(SandboxConfig {
        backend: SandboxBackend::Wasm,
        timeout_ms: 200,
        max_cpu_seconds: 0,
        ..Default::default()
    });
    let result = sandbox.execute(&path, "{}").await?;
    anyhow::ensure!(result.timed_out, "Host wait did not time out");
    anyhow::ensure!(
        result.duration_ms < 5000,
        "Host wait timeout took {} ms",
        result.duration_ms
    );
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn cancelling_unbounded_wasm_drops_guest_execution() -> Result<(), anyhow::Error> {
    let (directory, path) = wasm_fixture(
        r#"(module
        (memory (export "memory") 1)
        (func (export "malloc") (param i32) (result i32) i32.const 128)
        (func (export "run") (param i32 i32) (result i32) (loop $spin br $spin) i32.const 0))"#,
    )?;
    let execution = tokio::spawn(async move {
        let _directory = directory;
        WasmSandbox::new(SandboxConfig {
            backend: SandboxBackend::Wasm,
            timeout_ms: 0,
            max_cpu_seconds: 0,
            ..Default::default()
        })
        .execute(&path, "{}")
        .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    execution.abort();
    anyhow::ensure!(
        execution.await.is_err(),
        "Aborted guest execution completed successfully"
    );
    Ok(())
}
