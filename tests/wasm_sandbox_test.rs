//! Tests for WASM functionality and Sandbox Configuration
//!
//! Comprehensive tests covering:
//! - WASM sandbox creation and execution
//! - Sandbox configuration (default, override, per-tool/server)
//! - UpstreamConfig with sandbox_config
//! - CallableRecord with sandbox_config
//! - Integration tests

use skillsrs::core::registry::Registry;
use skillsrs::core::{
    BundledTool, CallableId, CallableKind, CallableRecord, CostHints, RiskTier, SchemaDigest,
};
use skillsrs::execution::upstream::{UpstreamConfig, UpstreamManager};
use skillsrs::execution::wasm::{WasmModuleInfo, WasmSandbox};
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
        id: id.clone(),
        kind: CallableKind::Tool,
        fq_name: format!("{}.{}", server, name),
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
        last_seen: chrono::Utc::now(),
        sandbox_config: None,
    }
}

/// Create a test skill record with bundled tool
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
        id: id.clone(),
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
        last_seen: chrono::Utc::now(),
        sandbox_config: None,
    }
}

/// Returns a path to a test WASM module (placeholder - returns non-existent path)
/// In a real test environment, this would create a valid WASM module
#[allow(dead_code)]
fn create_wasm_test_module_path() -> PathBuf {
    PathBuf::from("/tmp/test_module.wasm")
}

// ============================================================================
// WASM Sandbox Tests
// ============================================================================

#[tokio::test]
async fn test_wasm_sandbox_basic() {
    // Test WASM sandbox creation
    let config = SandboxConfig {
        backend: SandboxBackend::Wasm,
        timeout_ms: 30000,
        max_memory_bytes: 512 * 1024 * 1024,
        ..Default::default()
    };

    let wasm_sandbox = WasmSandbox::new(config);
    // The sandbox was created successfully

    // Test that execution returns error for non-existent file
    let result = wasm_sandbox
        .execute(PathBuf::from("/nonexistent.wasm").as_path(), "{}")
        .await;
    assert!(result.is_err());
    match result {
        Err(SandboxError::ExecutionFailed(msg)) => {
            assert!(msg.contains("not found") || msg.contains("WASM file"));
        }
        Err(_) => { /* Other errors are also acceptable */ }
        Ok(_) => panic!("Expected error for non-existent file"),
    }
}

#[tokio::test]
async fn test_wasm_sandbox_validation() {
    // Test validation of non-existent file
    let nonexistent = PathBuf::from("/nonexistent/test.wasm");
    let result = WasmSandbox::validate(&nonexistent);
    assert!(result.is_err());

    // Test validation of invalid file (not wasm)
    let invalid_path = PathBuf::from("/tmp/not_wasm.txt");
    // Create a text file
    std::fs::write(&invalid_path, "not a wasm module").ok();

    let result = WasmSandbox::validate(&invalid_path);
    assert!(result.is_err());

    // Cleanup
    let _ = std::fs::remove_file(&invalid_path);
}

#[test]
fn test_wasm_module_info() {
    // Test WasmModuleInfo structure
    let info = WasmModuleInfo {
        imports: vec!["env".to_string(), "wasi_snapshot_preview1".to_string()],
        exports: vec!["memory".to_string(), "run".to_string(), "init".to_string()],
        has_run_export: true,
        has_memory_export: true,
    };

    assert_eq!(info.imports.len(), 2);
    assert_eq!(info.exports.len(), 3);
    assert!(info.has_run_export);
    assert!(info.has_memory_export);
    assert!(info.exports.contains(&"run".to_string()));
    assert!(info.exports.contains(&"memory".to_string()));
}

#[tokio::test]
async fn test_wasm_sandbox_memory_limits() {
    // Test that memory limits are properly configured
    let config = SandboxConfig {
        backend: SandboxBackend::Wasm,
        timeout_ms: 30000,
        max_memory_bytes: 1024 * 1024 * 100, // 100 MB
        ..Default::default()
    };

    let wasm_sandbox = WasmSandbox::new(config);
    // Memory limits are configured but execution of non-existent file will fail
    let result = wasm_sandbox
        .execute(PathBuf::from("/tmp/test.wasm").as_path(), "{}")
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_wasm_sandbox_timeout() {
    // Test that timeout is properly configured
    let config = SandboxConfig {
        backend: SandboxBackend::Wasm,
        timeout_ms: 1000, // 1 second timeout
        max_memory_bytes: 512 * 1024 * 1024,
        ..Default::default()
    };

    let wasm_sandbox = WasmSandbox::new(config);
    // Timeout is configured but execution of non-existent file will fail before timeout
    let result = wasm_sandbox
        .execute(PathBuf::from("/tmp/test.wasm").as_path(), "{}")
        .await;
    assert!(result.is_err());
}

// ============================================================================
// Sandbox Configuration Tests
// ============================================================================

#[test]
fn test_sandbox_config_default() {
    let config = SandboxConfig::default();

    // Check default values
    assert_eq!(config.backend, SandboxBackend::Timeout);
    assert_eq!(config.timeout_ms, 30000); // 30 seconds
    assert!(!config.allow_network);
    assert_eq!(config.max_memory_bytes, 512 * 1024 * 1024); // 512 MB
    assert_eq!(config.max_cpu_seconds, 30);
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
        timeout_ms: Some(120000),
        backend: Some(SandboxBackend::Restricted),
        allow_network: Some(true),
        max_memory_bytes: Some(1024 * 1024 * 1024), // 1 GB
        max_cpu_seconds: Some(60),
        allow_read: vec![PathBuf::from("/home/user/projects")],
        allow_write: vec![PathBuf::from("/tmp")],
    };

    let merged = base.with_override(&override_config);

    assert_eq!(merged.timeout_ms, 120000);
    assert_eq!(merged.backend, SandboxBackend::Restricted);
    assert!(merged.allow_network);
    assert_eq!(merged.max_memory_bytes, 1024 * 1024 * 1024);
    assert_eq!(merged.max_cpu_seconds, 60);
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
    assert_eq!(merged.backend, SandboxBackend::Timeout);
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
        timeout_ms: Some(120000),
        max_memory_bytes: Some(2048 * 1024 * 1024),
        ..Default::default()
    };

    let merged = base.for_tool(Some(&tool_override), Some(&server_override));

    // Tool should take precedence for both
    assert_eq!(merged.timeout_ms, 120000);
    assert_eq!(merged.max_memory_bytes, 2048 * 1024 * 1024);
}

// ============================================================================
// SandboxConfigOverride Serialization Tests
// ============================================================================

#[test]
fn test_sandbox_config_override_deserialization() {
    let yaml = r#"
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
    "#;

    let config: SandboxConfigOverride = serde_yaml::from_str(yaml).unwrap();

    assert_eq!(config.backend, Some(SandboxBackend::Wasm));
    assert_eq!(config.timeout_ms, Some(60000));
    assert_eq!(config.allow_network, Some(true));
    assert_eq!(config.max_memory_bytes, Some(536870912));
    assert_eq!(config.max_cpu_seconds, Some(30));
    assert_eq!(config.allow_read.len(), 2);
    assert_eq!(config.allow_write.len(), 1);
}

#[test]
fn test_sandbox_config_override_serialization() {
    let config = SandboxConfigOverride {
        preset: None,
        backend: Some(SandboxBackend::Restricted),
        timeout_ms: Some(45000),
        allow_network: Some(false),
        max_memory_bytes: None,
        max_cpu_seconds: None,
        allow_read: vec![PathBuf::from("/data")],
        allow_write: vec![],
    };

    let yaml = serde_yaml::to_string(&config).unwrap();

    assert!(yaml.contains("restricted"));
    assert!(yaml.contains("45000"));
    assert!(yaml.contains("false"));
    assert!(yaml.contains("/data"));
}

// ============================================================================
// Upstream Config with Sandbox Tests
// ============================================================================

#[test]
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
    assert_eq!(sandbox.max_memory_bytes, Some(536870912));
    assert_eq!(sandbox.allow_read.len(), 1);
}

#[test]
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
    assert_eq!(sandbox.timeout_ms, Some(120000));
    assert_eq!(sandbox.allow_network, Some(true));
    assert_eq!(sandbox.allow_read.len(), 2);
    assert_eq!(sandbox.allow_write.len(), 1);
}

#[test]
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
    assert_eq!(sandbox.max_memory_bytes, Some(268435456));
}

#[test]
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
        assert_eq!(json, format!("\"{}\"", expected_str));

        let deserialized: SandboxBackend = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, backend);
    }
}

// ============================================================================
// Sandbox Execution Tests
// ============================================================================

#[tokio::test]
async fn test_sandbox_execute_timeout() {
    let config = SandboxConfig {
        backend: SandboxBackend::Timeout,
        timeout_ms: 100, // Very short timeout
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

#[tokio::test]
async fn test_sandbox_execute_with_env_vars() {
    let config = SandboxConfig::default();
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
    let config = SandboxConfig::default();
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

    let _runtime = Runtime::with_sandbox_config(registry.clone(), upstream_manager, custom_config);

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
    assert_eq!(merged.backend, SandboxBackend::Timeout);
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

#[test]
fn test_sandbox_error_display() {
    let errors = vec![
        SandboxError::ExecutionFailed("test error".to_string()),
        SandboxError::Timeout(5000),
        SandboxError::NotAvailable("feature not available".to_string()),
        SandboxError::InvalidConfig("bad config".to_string()),
    ];

    for error in errors {
        let msg = error.to_string();
        assert!(!msg.is_empty());
        // Should not panic
    }
}

#[test]
fn test_sandbox_error_from_io() {
    let io_error = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
    let sandbox_error: SandboxError = io_error.into();

    assert!(matches!(sandbox_error, SandboxError::Io(_)));
}
