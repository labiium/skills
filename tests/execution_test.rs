//! Tests for execution module: runtime, sandbox, upstream

use skillsrs::core::registry::Registry;
use skillsrs::core::{CallableId, CallableKind, CallableRecord, CostHints, RiskTier, SchemaDigest};
use skillsrs::execution::upstream::{Transport, UpstreamConfig, UpstreamManager};
use skillsrs::execution::{
    sandbox::{Sandbox, SandboxBackend, SandboxConfig},
    ExecContext, Runtime,
};
use std::sync::Arc;

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

#[tokio::test]
async fn test_upstream_manager_creation() {
    let registry = Arc::new(Registry::new());
    let manager = UpstreamManager::new(registry);

    let servers = manager.list_servers().await;
    assert_eq!(servers.len(), 0);
}

#[tokio::test]
async fn test_add_upstream_stdio() {
    let registry = Arc::new(Registry::new());
    let manager = UpstreamManager::new(registry.clone());

    let config = UpstreamConfig {
        alias: "test-server".to_string(),
        transport: Transport::Stdio,
        command: Some(vec!["echo".to_string(), "test".to_string()]),
        url: None,
        auth: None,
        repo: None,
        git_ref: None,
        skills: None,
        roots: None,
        tags: vec!["test".to_string()],
        sandbox_config: None,
    };

    // This will fail because echo is not a valid MCP server
    // In production, use a real MCP server for testing
    let result = manager.add_upstream(config).await;
    assert!(
        result.is_err(),
        "Expected connection to fail with non-MCP command"
    );
}

#[tokio::test]
async fn test_sandbox_basic() {
    let config = SandboxConfig {
        timeout_ms: 5000,
        backend: SandboxBackend::Timeout,
        ..Default::default()
    };

    let sandbox = Sandbox::new(config);

    // Test a simple command
    let result = sandbox
        .execute(
            "echo",
            &["hello".to_string()],
            std::path::Path::new("."),
            &[],
        )
        .await;

    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(output.stdout.contains("hello"));
    assert_eq!(output.exit_code, Some(0));
}

#[tokio::test]
async fn test_sandbox_env_vars() {
    let config = SandboxConfig::default();
    let sandbox = Sandbox::new(config);

    let env_vars = vec![("TEST_VAR".to_string(), "test_value".to_string())];

    let result = sandbox
        .execute("env", &[], std::path::Path::new("."), &env_vars)
        .await;

    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(output.stdout.contains("TEST_VAR=test_value"));
}

#[tokio::test]
async fn test_validation_missing_required() {
    let registry = Arc::new(Registry::new());
    let tool = create_test_tool_record("test-tool", "test-server");
    let id = tool.id.clone();
    registry.register(tool).unwrap();

    let upstream_manager = Arc::new(UpstreamManager::new(registry.clone()));
    let runtime = Runtime::new(registry, upstream_manager);

    let ctx = ExecContext {
        callable_id: id,
        arguments: serde_json::json!({}), // Missing 'path'
        timeout_ms: Some(5000),
        trace_enabled: false,
    };

    let result = runtime.execute(ctx).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.to_string().contains("Missing required") || err.to_string().contains("validation"));
}

#[tokio::test]
async fn test_callable_not_found() {
    let registry = Arc::new(Registry::new());
    let upstream_manager = Arc::new(UpstreamManager::new(registry.clone()));
    let runtime = Runtime::new(registry, upstream_manager);

    let ctx = ExecContext {
        callable_id: CallableId::from("nonexistent"),
        arguments: serde_json::json!({}),
        timeout_ms: Some(5000),
        trace_enabled: false,
    };

    let result = runtime.execute(ctx).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.to_string().contains("not found"));
}
