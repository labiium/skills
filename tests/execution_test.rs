//! Tests for execution module: runtime, sandbox, upstream

use jiff::Timestamp;
use skillsrs::core::registry::Registry;
use skillsrs::core::{
    BundledTool, CallableId, CallableKind, CallableRecord, CostHints, ExecutionStatus, RiskTier,
    SchemaDigest, ToolResult, ToolResultContent,
};
use skillsrs::execution::upstream::{Transport, UpstreamConfig, UpstreamManager};
use skillsrs::execution::{
    sandbox::{Sandbox, SandboxBackend, SandboxConfig},
    ExecContext, Runtime,
};
use std::sync::Arc;

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
        required: false,
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
        description: None,
    };

    // This will fail because echo is not a valid MCP server
    // In production, use a real MCP server for testing
    let result = manager.add_upstream(config).await;
    assert!(
        result.is_err(),
        "Expected connection to fail with non-MCP command"
    );
}

#[cfg(unix)]
#[tokio::test]
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
async fn test_sandbox_basic() {
    let config = SandboxConfig {
        timeout_ms: 5000,
        backend: SandboxBackend::Timeout,
        allow_network: true,
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

#[cfg(not(unix))]
#[tokio::test]
async fn native_hardening_fails_closed_without_process_group_limits() {
    for backend in [
        SandboxBackend::Timeout,
        SandboxBackend::Restricted,
        SandboxBackend::Bubblewrap,
    ] {
        let sandbox = Sandbox::new(SandboxConfig {
            backend,
            allow_network: true,
            ..Default::default()
        });
        let result = sandbox
            .execute("must-not-launch", &[], std::path::Path::new("."), &[])
            .await;
        assert!(matches!(
            result,
            Err(skillsrs::execution::sandbox::SandboxError::NotAvailable(_))
        ));
    }
}

#[cfg(unix)]
#[tokio::test]
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
async fn test_sandbox_env_vars() {
    let config = SandboxConfig::development();
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
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
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

#[test]
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
fn test_validate_callable_arguments_success() {
    let registry = Arc::new(Registry::new());
    let tool = create_test_tool_record("test-tool", "test-server");
    let id = tool.id.clone();
    registry.register(tool).unwrap();

    let upstream_manager = Arc::new(UpstreamManager::new(registry.clone()));
    let runtime = Runtime::new(registry, upstream_manager);

    runtime
        .validate_callable_arguments(&id, &serde_json::json!({ "path": "/tmp/file.txt" }))
        .unwrap();
}

#[test]
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
fn test_validate_callable_arguments_missing_required() {
    let registry = Arc::new(Registry::new());
    let tool = create_test_tool_record("test-tool", "test-server");
    let id = tool.id.clone();
    registry.register(tool).unwrap();

    let upstream_manager = Arc::new(UpstreamManager::new(registry.clone()));
    let runtime = Runtime::new(registry, upstream_manager);

    let err = runtime
        .validate_callable_arguments(&id, &serde_json::json!({}))
        .unwrap_err();
    assert!(err.to_string().contains("Missing required argument: path"));
}

#[test]
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
fn test_validate_callable_arguments_requires_object_when_schema_has_required_fields() {
    let registry = Arc::new(Registry::new());
    let tool = create_test_tool_record("test-tool", "test-server");
    let id = tool.id.clone();
    registry.register(tool).unwrap();

    let upstream_manager = Arc::new(UpstreamManager::new(registry.clone()));
    let runtime = Runtime::new(registry, upstream_manager);

    let err = runtime
        .validate_callable_arguments(&id, &serde_json::json!(null))
        .unwrap_err();
    assert!(err.to_string().contains("Arguments must be an object"));
}

#[tokio::test]
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
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

#[test]
fn upstream_results_preserve_all_blocks_and_structured_payload() -> Result<(), anyhow::Error> {
    let raw = serde_json::json!({
        "content": [
            {"type":"text","text":"hello","annotations":{"audience":["user"]}},
            {"type":"image","data":"AA==","mimeType":"image/png"},
            {"type":"audio","data":"AA==","mimeType":"audio/wav"},
            {"type":"resource","resource":{"uri":"file:///sample","blob":"AA==","mimeType":"application/octet-stream"}},
            {"type":"resource_link","uri":"file:///other","name":"other","size":1},
            {"type":"future_kind","opaque":{"preserve":true}}
        ],
        "structuredContent": [1, {"trace":"upstream-owned"}],
        "_meta": {"opaque":"retained"}
    });
    let content = raw
        .get("content")
        .ok_or_else(|| anyhow::anyhow!("Upstream fixture has no content"))?;
    let structured_content = raw
        .get("structuredContent")
        .ok_or_else(|| anyhow::anyhow!("Upstream fixture has no structured content"))?;
    let decoded = serde_json::from_value::<Vec<ToolResultContent>>(content.clone());
    anyhow::ensure!(
        decoded
            .ok()
            .and_then(|blocks| serde_json::to_value(blocks).ok())
            == Some(content.clone()),
        "Decoded upstream blocks differ from the fixture"
    );
    let result = ToolResult::from_upstream(raw.clone());
    anyhow::ensure!(
        serde_json::to_value(&result.content).ok() == Some(content.clone()),
        "Upstream result content differs from the fixture"
    );
    anyhow::ensure!(
        result.structured_content == Some(structured_content.clone()),
        "Upstream structured content differs from the fixture"
    );
    anyhow::ensure!(
        result.raw_output == Some(raw),
        "Upstream raw output differs from the fixture"
    );
    anyhow::ensure!(
        result.content.first().and_then(ToolResultContent::as_text) == Some("hello"),
        "First upstream block is not the expected text"
    );
    anyhow::ensure!(
        !result.is_error,
        "Successful upstream result was marked as an error"
    );
    Ok(())
}

#[test]
fn empty_success_and_input_required_are_not_converted_to_errors_or_completion() {
    let empty = ToolResult::from_upstream(serde_json::json!({"content":[]}));
    assert!(empty.content.is_empty());
    assert!(!empty.is_error);
    let waiting = ToolResult::from_upstream(serde_json::json!({
        "status":"input_required", "inputRequests":[{"id":"approval"}], "requestState":{"opaque":1}
    }));
    assert_eq!(waiting.status, ExecutionStatus::InputRequired);
    assert!(!waiting.is_error);
    assert_eq!(
        waiting
            .raw_output
            .as_ref()
            .and_then(|raw| raw.get("requestState")),
        Some(&serde_json::json!({"opaque":1}))
    );
}

#[tokio::test]
async fn instruction_activation_never_executes_helpers_and_explicit_selection_does(
) -> Result<(), anyhow::Error> {
    let directory = tempfile::tempdir()?;
    let scripts = directory.path().join("scripts");
    std::fs::create_dir(&scripts)?;
    std::fs::write(directory.path().join("SKILL.md"), "---\nname: instruction-skill\ndescription: A test skill\n---\nRead these instructions before running helpers.\n")?;
    let helper = scripts.join("helper.sh");
    std::fs::write(&helper, "printf ran > marker; printf helper-output")?;
    let mut record = create_test_tool_record("instruction-skill", "unused");
    record.kind = CallableKind::Skill;
    record.id = CallableId::skill("instruction-skill", "1.0.0");
    record.skill_directory = Some(directory.path().to_path_buf());
    record.bundled_tools = vec![BundledTool {
        name: "helper".to_string(),
        description: "Explicit helper".to_string(),
        command: vec!["sh".to_string(), helper.to_string_lossy().into_owned()],
        schema: serde_json::json!({"type":"object", "required":["value"]}),
    }];
    let ctx = ExecContext {
        callable_id: record.id.clone(),
        arguments: serde_json::json!({}),
        timeout_ms: None,
        trace_enabled: true,
    };
    let registry = Arc::new(Registry::new());
    registry.register(record)?;
    let manager = Arc::new(UpstreamManager::new(registry.clone()));
    let runtime = Runtime::with_sandbox_config(registry, manager, SandboxConfig::development());
    let activated = runtime.execute(ctx.clone()).await?;
    anyhow::ensure!(
        activated
            .content
            .first()
            .and_then(ToolResultContent::as_text)
            .is_some_and(|text| text.contains("Read these instructions")),
        "Activation did not return the skill instructions"
    );
    anyhow::ensure!(
        !directory.path().join("marker").exists(),
        "Activation executed the helper"
    );
    anyhow::ensure!(
        activated
            .structured_content
            .as_ref()
            .and_then(|data| data.get("resources_root"))
            == Some(&serde_json::json!(directory.path())),
        "Activation returned an unexpected resources root"
    );
    anyhow::ensure!(
        runtime
            .validate_script_arguments(&ctx.callable_id, "missing", &ctx.arguments)
            .is_err(),
        "Missing helper passed argument validation"
    );
    anyhow::ensure!(
        runtime
            .validate_script_arguments(&ctx.callable_id, "helper", &ctx.arguments)
            .is_err(),
        "Helper accepted arguments without the required value"
    );
    let mut helper_context = ctx;
    helper_context.arguments = serde_json::json!({"value":"approved"});
    runtime.validate_script_arguments(
        &helper_context.callable_id,
        "helper",
        &helper_context.arguments,
    )?;
    #[cfg(unix)]
    {
        let output = runtime
            .execute_script(helper_context, "scripts/helper.sh")
            .await?;
        anyhow::ensure!(
            !output.is_error,
            "Explicit helper execution returned an error"
        );
        anyhow::ensure!(
            output.content.first().and_then(ToolResultContent::as_text) == Some("helper-output"),
            "Explicit helper returned unexpected output"
        );
        anyhow::ensure!(
            directory.path().join("marker").exists(),
            "Explicit helper did not create its marker"
        );
    }
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn standard_native_hardening_does_not_inherit_host_environment() -> Result<(), anyhow::Error>
{
    let sandbox = Sandbox::new(SandboxConfig::development());
    let output = sandbox
        .execute(
            "env",
            &[],
            std::path::Path::new("."),
            &[("EXPLICIT_SECRET".to_string(), "intended".to_string())],
        )
        .await?;
    for line in output.stdout.lines() {
        let key = line.split('=').next().unwrap_or_default();
        anyhow::ensure!(
            [
                "PATH",
                "LANG",
                "HOME",
                "TMPDIR",
                "TMP",
                "TEMP",
                "EXPLICIT_SECRET"
            ]
            .contains(&key),
            "Unexpected inherited variable: {key}"
        );
    }
    anyhow::ensure!(
        output.stdout.contains("EXPLICIT_SECRET=intended"),
        "Explicit environment variable is missing"
    );
    anyhow::ensure!(
        !output
            .stdout
            .contains(&format!("HOME={}\n", std::env::var("HOME")?)),
        "Native sandbox inherited the host HOME"
    );
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn timeout_and_cancellation_kill_subprocess_descendants() -> Result<(), anyhow::Error> {
    let directory = tempfile::tempdir()?;
    for cancel in [false, true] {
        let marker = if cancel {
            "cancelled-child"
        } else {
            "timed-out-child"
        };
        let working_dir = directory.path().to_path_buf();
        let sandbox = Sandbox::new(SandboxConfig {
            timeout_ms: if cancel { 5000 } else { 50 },
            ..SandboxConfig::development()
        });
        let command = format!("(sleep 0.3; printf survived > {marker}) & wait");
        let task = tokio::spawn(async move {
            sandbox
                .execute("sh", &["-c".to_string(), command], &working_dir, &[])
                .await
        });
        if cancel {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            task.abort();
            anyhow::ensure!(
                task.await.is_err(),
                "Aborted subprocess task completed successfully"
            );
        } else {
            anyhow::ensure!(
                task.await??.timed_out,
                "Subprocess execution did not time out"
            );
        }
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        anyhow::ensure!(
            !directory.path().join(marker).exists(),
            "A descendant outlived its invocation"
        );
    }
    Ok(())
}

#[tokio::test]
async fn hardening_backends_reject_unenforced_containment_requests() {
    for backend in [SandboxBackend::Timeout, SandboxBackend::Restricted] {
        let sandbox = Sandbox::new(SandboxConfig {
            backend,
            allow_network: false,
            ..SandboxConfig::development()
        });
        assert!(sandbox
            .execute("true", &[], std::path::Path::new("."), &[])
            .await
            .is_err());
    }
}

#[cfg(unix)]
#[tokio::test]
async fn explicit_helpers_use_configured_deadline_without_request_override(
) -> Result<(), anyhow::Error> {
    let directory = tempfile::tempdir()?;
    let mut record = create_test_tool_record("deadline-skill", "unused");
    record.id = CallableId::skill("deadline-skill", "1.0.0");
    record.kind = CallableKind::Skill;
    record.skill_directory = Some(directory.path().to_path_buf());
    record.bundled_tools = vec![BundledTool {
        name: "wait".to_string(),
        description: "A slow helper".to_string(),
        command: vec!["sh".to_string(), "-c".to_string(), "sleep 1".to_string()],
        schema: serde_json::json!({"type":"object"}),
    }];
    let ctx = ExecContext {
        callable_id: record.id.clone(),
        arguments: serde_json::json!({}),
        timeout_ms: None,
        trace_enabled: false,
    };
    let registry = Arc::new(Registry::new());
    registry.register(record)?;
    let manager = Arc::new(UpstreamManager::new(registry.clone()));
    let runtime = Runtime::with_sandbox_config(
        registry,
        manager,
        SandboxConfig {
            timeout_ms: 25,
            ..SandboxConfig::development()
        },
    );
    let result = runtime.execute_script(ctx, "wait").await?;
    anyhow::ensure!(
        result.is_error,
        "Configured helper deadline did not produce an error"
    );
    anyhow::ensure!(
        result
            .content
            .iter()
            .filter_map(ToolResultContent::as_text)
            .any(|text| text.contains("timed out")),
        "Helper deadline error did not report a timeout"
    );
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn invocation_temporary_directories_are_private_and_removed() -> Result<(), anyhow::Error> {
    let sandbox = Sandbox::new(SandboxConfig::development());
    let first = sandbox
        .execute(
            "sh",
            &["-c".to_string(), "printf '%s' \"$TMPDIR\"".to_string()],
            std::path::Path::new("."),
            &[],
        )
        .await?;
    let second = sandbox
        .execute(
            "sh",
            &["-c".to_string(), "printf '%s' \"$TMPDIR\"".to_string()],
            std::path::Path::new("."),
            &[],
        )
        .await?;
    anyhow::ensure!(
        first.stdout != second.stdout,
        "Invocations shared the same temporary directory"
    );
    anyhow::ensure!(
        std::path::Path::new(&first.stdout) != std::env::temp_dir(),
        "Invocation used the shared system temporary directory"
    );
    anyhow::ensure!(
        !std::path::Path::new(&first.stdout).exists(),
        "First invocation temporary directory was not removed"
    );
    anyhow::ensure!(
        !std::path::Path::new(&second.stdout).exists(),
        "Second invocation temporary directory was not removed"
    );
    Ok(())
}
