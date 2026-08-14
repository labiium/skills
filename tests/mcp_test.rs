//! Tests for MCP module: `SkillsServer`

use rmcp::ServerHandler;
use skillsrs::core::policy::PolicyConfig;
use skillsrs::core::registry::Registry;
use skillsrs::execution::upstream::UpstreamManager;
use skillsrs::execution::Runtime;
use skillsrs::mcp::{ExecInput, ExecResponse, SkillsServer};
use skillsrs::storage::search::SearchEngine;
use skillsrs::storage::SkillStore;
use std::sync::Arc;
use tempfile::TempDir;

fn create_empty_server() -> (SkillsServer, Arc<Registry>, Arc<SearchEngine>) {
    create_empty_server_with_policy(PolicyConfig::default())
}

#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
fn create_empty_server_with_policy(
    policy: PolicyConfig,
) -> (SkillsServer, Arc<Registry>, Arc<SearchEngine>) {
    let registry = Arc::new(Registry::new());
    let search_engine = Arc::new(SearchEngine::new(registry.clone()));
    let open = policy.mode == skillsrs::core::policy::PolicyMode::Open;
    let policy_engine = Arc::new(skillsrs::core::policy::PolicyEngine::new(policy).unwrap());
    let upstream_manager = Arc::new(UpstreamManager::new(registry.clone()));
    let runtime = if open {
        let mut sandbox = skillsrs::execution::sandbox::SandboxConfig::default();
        sandbox.backend = skillsrs::execution::sandbox::SandboxBackend::None;
        Arc::new(Runtime::with_sandbox_config(
            registry.clone(),
            upstream_manager,
            sandbox,
        ))
    } else {
        Arc::new(Runtime::new(registry.clone(), upstream_manager))
    };
    let temp_dir = TempDir::new().unwrap();
    let skill_store = Arc::new(
        SkillStore::with_search_engine(temp_dir.path(), registry.clone(), search_engine.clone())
            .unwrap(),
    );

    (
        SkillsServer::new(
            registry.clone(),
            search_engine.clone(),
            policy_engine,
            runtime,
            skill_store,
        ),
        registry,
        search_engine,
    )
}

fn create_server_with_test_tool() -> (SkillsServer, String) {
    create_server_with_test_tool_policy(PolicyConfig::default())
}

#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
fn create_server_with_test_tool_policy(policy: PolicyConfig) -> (SkillsServer, String) {
    let open = policy.mode == skillsrs::core::policy::PolicyMode::Open;
    let (server, registry, search_engine) = create_empty_server_with_policy(policy);
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "path": { "type": "string" }
        },
        "required": ["path"]
    });
    let digest = skillsrs::core::SchemaDigest::from_schema(&schema).unwrap();
    let id = skillsrs::core::CallableId::tool("test", "test-tool", digest.as_str());
    let id_string = id.as_str().to_string();

    registry
        .register(skillsrs::core::CallableRecord {
            id,
            kind: skillsrs::core::CallableKind::Tool,
            fq_name: "test.test-tool".to_string(),
            name: "test-tool".to_string(),
            title: Some("Test Tool".to_string()),
            description: Some("A test tool".to_string()),
            tags: vec!["test".to_string()],
            input_schema: schema,
            output_schema: None,
            schema_digest: digest,
            server_alias: Some("test".to_string()),
            upstream_tool_name: Some("test-tool".to_string()),
            skill_version: None,
            uses: vec![],
            skill_directory: None,
            bundled_tools: vec![],
            additional_files: vec![],
            cost_hints: skillsrs::core::CostHints::default(),
            risk_tier: if open {
                skillsrs::core::RiskTier::Admin
            } else {
                skillsrs::core::RiskTier::ReadOnly
            },
            last_seen: chrono::Utc::now(),
            sandbox_config: None,
        })
        .unwrap();

    search_engine.rebuild();

    (server, id_string)
}

fn exec_input(id: String, arguments: serde_json::Value) -> ExecInput {
    ExecInput {
        id,
        arguments,
        describe: false,
        format: "both".to_string(),
        include_output_schema: true,
        max_bytes: 50_000,
        json_pointer: None,
        dry_run: false,
        timeout_ms: None,
        consent: None,
        trace: None,
    }
}

#[test]
fn test_server_exposes_exec_not_schema() {
    let (server, _) = create_server_with_test_tool();

    let mut tool_names = server.tool_names();
    tool_names.sort();

    assert_eq!(tool_names, vec!["exec", "manage", "search"]);
    assert!(!tool_names.contains(&"schema".to_string()));
}

#[test]
fn test_server_creation() {
    // This should not panic
    let server = create_empty_server().0;
    let info = server.get_info();
    assert_eq!(info.protocol_version.as_str(), "2026-07-28");
    assert!(server
        .supported_protocol_versions()
        .contains(&rmcp::model::ProtocolVersion::V_2026_07_28));
}

#[tokio::test]
async fn test_open_register_component_profile_is_versioned_and_gate_free() {
    let (server, _, _) = create_empty_server_with_policy(PolicyConfig::open());
    let profile = server.component_profile().await;
    assert_eq!(profile.schema, "mesurii.component-profile.v1");
    assert_eq!(profile.component, "register");
    assert_eq!(profile.profile, "open");
    assert!(!profile.ready);
    assert!(profile.gates.is_empty());
    assert_eq!(profile.sandbox, "none");
    assert!(profile.source_revision.is_none());
}

#[tokio::test]
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
async fn test_open_profile_reaches_registered_off_panel_instrument() {
    let (server, id) = create_server_with_test_tool_policy(PolicyConfig::open());
    let response = server
        .exec_callable(ExecInput {
            dry_run: true,
            ..exec_input(id, serde_json::json!({"path": "workspace"}))
        })
        .await
        .unwrap();
    assert_eq!(response.mode, "dry_run");
}

#[tokio::test]
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
async fn test_exec_describe_returns_schema() {
    let (server, id) = create_server_with_test_tool();

    let response = server
        .exec_callable(ExecInput {
            describe: true,
            format: "json_schema".to_string(),
            ..exec_input(id, serde_json::json!({}))
        })
        .await
        .unwrap();

    assert_eq!(response.mode, "describe");
    assert_eq!(response.callable.name, "test-tool");
    let description = response.description.unwrap();
    assert_eq!(
        description.descriptor.as_ref().unwrap().schema,
        "skills.callable-descriptor.v1"
    );
    assert!(description.input_schema.is_some());
    assert!(description.signature.is_none());
    assert!(response.dry_run.is_none());
    assert!(response.execution.is_none());
}

#[tokio::test]
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
async fn test_exec_describe_takes_precedence_over_dry_run_and_invalid_arguments() {
    let (server, id) = create_server_with_test_tool();

    let response = server
        .exec_callable(ExecInput {
            describe: true,
            dry_run: true,
            ..exec_input(id, serde_json::json!(null))
        })
        .await
        .unwrap();

    assert_eq!(response.mode, "describe");
    assert!(response.description.is_some());
    assert!(response.dry_run.is_none());
    assert!(response.execution.is_none());
}

#[tokio::test]
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
async fn test_exec_describe_rejects_invalid_json_pointer() {
    let (server, id) = create_server_with_test_tool();

    let err = server
        .exec_callable(ExecInput {
            describe: true,
            format: "json_schema".to_string(),
            json_pointer: Some("/does-not-exist".to_string()),
            ..exec_input(id, serde_json::json!({}))
        })
        .await
        .unwrap_err();

    assert!(err.contains("Invalid JSON pointer"));
}

#[tokio::test]
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
async fn test_exec_describe_enforces_max_bytes() {
    let (server, id) = create_server_with_test_tool();

    let err = server
        .exec_callable(ExecInput {
            describe: true,
            max_bytes: 1,
            ..exec_input(id, serde_json::json!({}))
        })
        .await
        .unwrap_err();

    assert!(err.contains("Response exceeds max_bytes limit"));
}

#[tokio::test]
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
async fn test_exec_dry_run_validates_arguments() {
    let (server, id) = create_server_with_test_tool();

    let err = server
        .exec_callable(ExecInput {
            dry_run: true,
            ..exec_input(id, serde_json::json!({}))
        })
        .await
        .unwrap_err();

    assert!(err.contains("Missing required argument: path"));
}

#[tokio::test]
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
async fn test_exec_dry_run_accepts_valid_arguments() {
    let (server, id) = create_server_with_test_tool();

    let response = server
        .exec_callable(ExecInput {
            dry_run: true,
            ..exec_input(id, serde_json::json!({ "path": "/tmp/file.txt" }))
        })
        .await
        .unwrap();

    assert_eq!(response.mode, "dry_run");
    assert!(response.dry_run.unwrap().valid);
    assert!(response.execution.is_none());
}

#[test]
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
fn test_exec_response_schema_matches_optional_payload_shape() {
    let schema = serde_json::to_value(schemars::schema_for!(ExecResponse)).unwrap();
    let root_required = schema
        .get("required")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();

    assert!(root_required.contains(&serde_json::json!("mode")));
    assert!(root_required.contains(&serde_json::json!("callable")));
    assert!(!root_required.contains(&serde_json::json!("description")));
    assert!(!root_required.contains(&serde_json::json!("dry_run")));
    assert!(!root_required.contains(&serde_json::json!("execution")));

    let exec_output_required = schema
        .pointer("/$defs/ExecOutput/required")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();

    assert!(exec_output_required.contains(&serde_json::json!("content")));
    assert!(exec_output_required.contains(&serde_json::json!("is_error")));
    assert!(!exec_output_required.contains(&serde_json::json!("structured_content")));
    assert!(!exec_output_required.contains(&serde_json::json!("timing")));
    assert!(!exec_output_required.contains(&serde_json::json!("steps")));
}
