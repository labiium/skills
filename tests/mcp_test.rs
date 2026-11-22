//! Tests for MCP module: `SkillsServer`

use jiff::Timestamp;
use rmcp::ServerHandler;
use skillsrs::core::policy::PolicyConfig;
use skillsrs::core::registry::Registry;
use skillsrs::execution::upstream::UpstreamManager;
use skillsrs::execution::Runtime;
use skillsrs::mcp::{ExecInput, ExecResponse, ManageInput, SkillsServer};
use skillsrs::storage::search::SearchEngine;
use skillsrs::storage::SkillStore;
use std::sync::Arc;
use tempfile::TempDir;

fn create_empty_server() -> (SkillsServer, Arc<Registry>, Arc<SearchEngine>, TempDir) {
    create_empty_server_with_policy(PolicyConfig::default())
}

#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
fn create_empty_server_with_policy(
    policy: PolicyConfig,
) -> (SkillsServer, Arc<Registry>, Arc<SearchEngine>, TempDir) {
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
        temp_dir,
    )
}

fn create_server_with_test_tool() -> (SkillsServer, String) {
    create_server_with_test_tool_policy(PolicyConfig {
        trusted_servers: vec!["test".to_string()],
        ..Default::default()
    })
}

fn create_server_with_test_tool_policy(policy: PolicyConfig) -> (SkillsServer, String) {
    let open = policy.mode == skillsrs::core::policy::PolicyMode::Open;
    let risk = if open {
        skillsrs::core::RiskTier::Admin
    } else {
        skillsrs::core::RiskTier::ReadOnly
    };
    create_server_with_test_tool_risk(policy, risk)
}

#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast"
)]
fn create_server_with_test_tool_risk(
    policy: PolicyConfig,
    risk_tier: skillsrs::core::RiskTier,
) -> (SkillsServer, String) {
    let (server, registry, search_engine, _root) = create_empty_server_with_policy(policy);
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
            risk_tier,
            last_seen: Timestamp::now(),
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
        script: None,
        input_responses: None,
        request_state: None,
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
    let (server, _, _, _root) = create_empty_server_with_policy(PolicyConfig::open());
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

#[tokio::test]
async fn standard_readiness_is_generic_and_does_not_advertise_a_call_quota() {
    let (server, _, _, _root) = create_empty_server();
    let profile = server.component_profile().await;
    assert!(profile.ready);
    assert_eq!(profile.component, "skills");
    assert_eq!(profile.schema, "skills.component-profile.v1");
    assert!(profile.gates.contains(&"management".to_string()));
    assert!(!profile.gates.contains(&"call".to_string()));
}

#[tokio::test]
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast"
)]
async fn standard_management_denies_every_mutation_before_side_effects() {
    let (server, _, _, root) = create_empty_server();
    let skill_dir = root.path().join("protected");
    std::fs::create_dir_all(&skill_dir).unwrap();
    let original = "---\nname: protected\ndescription: Keep this skill\n---\nKeep this body.";
    std::fs::write(skill_dir.join("SKILL.md"), original).unwrap();
    for operation in [
        "create",
        "update",
        "delete",
        "add_upstream",
        "remove_upstream",
        "update_upstream",
    ] {
        let request: ManageInput = serde_json::from_value(serde_json::json!({
            "operation": operation,
            "skill_id": "protected",
            "name": "created",
            "description": "Do not create",
            "skill_md": "Do not create",
            "upstream_alias": "untrusted",
            "transport": "stdio",
            "command": ["/bin/sh", "-c", "exit 0"],
            "consent": {"level": "admin_confirmed"}
        }))
        .unwrap();
        let error = server.manage_request(request).await.err().unwrap();
        assert!(
            error.contains("management is disabled"),
            "{operation}: {error}"
        );
        assert_eq!(
            std::fs::read_to_string(skill_dir.join("SKILL.md")).unwrap(),
            original
        );
        assert!(!root.path().join("created").exists());
    }
    let profile = server
        .manage_request(serde_json::from_value(serde_json::json!({"operation":"profile"})).unwrap())
        .await
        .unwrap();
    assert_eq!(profile.0.operation, "profile");
}

#[tokio::test]
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast"
)]
async fn explicit_management_opt_in_and_open_allow_real_crud() {
    for policy in [
        PolicyConfig {
            management_enabled: true,
            ..Default::default()
        },
        PolicyConfig::open(),
    ] {
        let (server, _, _, root) = create_empty_server_with_policy(policy);
        let create: ManageInput = serde_json::from_value(serde_json::json!({
            "operation": "create", "name": "managed", "description": "Managed instructions",
            "skill_md": "# Managed\nOriginal instructions.", "tags": ["keep"],
            "references": [["guide.txt", "Read me"]]
        }))
        .unwrap();
        server.manage_request(create).await.unwrap();
        server.manage_request(serde_json::from_value(serde_json::json!({
            "operation": "update", "skill_id": "managed", "description": "Updated description"
        })).unwrap()).await.unwrap();
        let get = server
            .manage_request(
                serde_json::from_value(serde_json::json!({
                    "operation": "get", "skill_id": "managed", "filename": "references/guide.txt"
                }))
                .unwrap(),
            )
            .await
            .unwrap()
            .0;
        assert_eq!(get.message, "Read me");
        assert!(get.data.is_none());
        let instructions = server
            .manage_request(
                serde_json::from_value(serde_json::json!({
                    "operation": "get", "skill_id": "managed"
                }))
                .unwrap(),
            )
            .await
            .unwrap()
            .0;
        assert!(instructions.message.contains("Original instructions."));
        std::fs::create_dir_all(root.path().join("managed/assets")).unwrap();
        std::fs::write(root.path().join("managed/assets/data.bin"), [0xff, 0, 1]).unwrap();
        let binary = server
            .manage_request(
                serde_json::from_value(serde_json::json!({
                    "operation": "get", "skill_id": "managed", "filename": "assets/data.bin"
                }))
                .unwrap(),
            )
            .await
            .unwrap()
            .0
            .data
            .unwrap();
        assert_eq!(
            binary.get("encoding").and_then(serde_json::Value::as_str),
            Some("base64")
        );
        assert_eq!(
            binary.get("content").and_then(serde_json::Value::as_str),
            Some("/wAB")
        );
        server
            .manage_request(
                serde_json::from_value(serde_json::json!({
                    "operation": "delete", "skill_id": "managed"
                }))
                .unwrap(),
            )
            .await
            .unwrap();
        assert!(!root.path().join("managed").exists());
    }
}

#[tokio::test]
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast"
)]
async fn request_consent_cannot_authorize_unknown_code() {
    let (server, id) = create_server_with_test_tool_risk(
        PolicyConfig {
            trusted_servers: vec!["test".to_string()],
            ..Default::default()
        },
        skillsrs::core::RiskTier::Unknown,
    );
    let input: ExecInput = serde_json::from_value(serde_json::json!({
        "id": id,
        "arguments": {"path": "/tmp/test"},
        "consent": {"level": "admin_confirmed", "token": "self-issued"},
        "dry_run": true
    }))
    .unwrap();
    assert!(server
        .exec_callable(input.clone())
        .await
        .unwrap_err()
        .contains("trusted consent"));
    let trusted = server.with_trusted_consent(skillsrs::core::policy::ConsentLevel::UserConfirmed);
    assert_eq!(trusted.exec_callable(input).await.unwrap().mode, "dry_run");
}

#[tokio::test]
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast"
)]
async fn skill_reads_and_activation_never_execute_bundled_code() {
    let (server, _, _, root) = create_empty_server_with_policy(PolicyConfig {
        management_enabled: true,
        ..Default::default()
    });
    let marker = root.path().join("executed");
    let script = format!(
        "from pathlib import Path\nPath({:?}).write_text('executed')\n",
        marker.to_str().unwrap()
    );
    let created = server.manage_request(serde_json::from_value(serde_json::json!({
        "operation": "create", "name": "instruction-only", "description": "Read instructions",
        "skill_md": "Use the instructions without running bundled code.",
        "scripts": [["write.py", script]]
    })).unwrap()).await.unwrap().0;
    let id = created.skill_id.unwrap();
    server
        .manage_request(
            serde_json::from_value(serde_json::json!({
                "operation": "get", "skill_id": id
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    let activated = server
        .exec_callable(exec_input(id.clone(), serde_json::json!({})))
        .await
        .unwrap();
    assert_eq!(activated.execution.unwrap().status, "completed");
    assert!(!marker.exists());
    let requested: ExecInput = serde_json::from_value(serde_json::json!({
        "id": id, "script": "scripts/write.py", "consent": {"level": "admin_confirmed"}
    }))
    .unwrap();
    assert!(server
        .exec_callable(requested)
        .await
        .unwrap_err()
        .contains("trusted consent"));
    assert!(!marker.exists());
}

#[tokio::test]
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast"
)]
async fn discovery_and_description_enforce_read_trust_and_denies() {
    for policy in [
        PolicyConfig::default(),
        PolicyConfig {
            trusted_servers: vec!["test".to_string()],
            deny_patterns: vec!["*".to_string()],
            ..Default::default()
        },
    ] {
        let (server, id) = create_server_with_test_tool_policy(policy);
        let input = serde_json::from_value(serde_json::json!({"q": "*"})).unwrap();
        let results = server
            .search(rmcp::handler::server::wrapper::Parameters(input))
            .await
            .unwrap()
            .0;
        assert!(results.matches.is_empty());
        assert_eq!(results.stats.total_callables, 0);
        assert!(server
            .exec_callable(ExecInput {
                describe: true,
                ..exec_input(id, serde_json::json!({}))
            })
            .await
            .unwrap_err()
            .contains("Read denied"));
    }
}

#[tokio::test]
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast"
)]
async fn management_disabled_still_allows_existing_instruction_reads() {
    let (writer, registry, search, root) = create_empty_server_with_policy(PolicyConfig {
        management_enabled: true,
        ..Default::default()
    });
    writer
        .manage_request(
            serde_json::from_value(serde_json::json!({
                "operation": "create", "name": "readable", "description": "Read-only management",
                "skill_md": "Existing instructions remain readable."
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    let upstreams = Arc::new(UpstreamManager::new(registry.clone()));
    let store = Arc::new(
        SkillStore::with_search_engine(root.path(), registry.clone(), search.clone()).unwrap(),
    );
    let reader = SkillsServer::new(
        registry.clone(),
        search,
        Arc::new(skillsrs::core::policy::PolicyEngine::new(PolicyConfig::default()).unwrap()),
        Arc::new(Runtime::new(registry, upstreams)),
        store,
    );
    let response = reader
        .manage_request(
            serde_json::from_value(serde_json::json!({
                "operation": "get", "skill_id": "readable"
            }))
            .unwrap(),
        )
        .await
        .unwrap()
        .0;
    assert!(response
        .message
        .contains("Existing instructions remain readable."));
    assert!(!reader
        .component_profile()
        .await
        .gates
        .contains(&"call".to_string()));
}

#[tokio::test]
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast"
)]
async fn script_dry_run_validates_selection_without_running_code() {
    let (server, _, _, root) = create_empty_server_with_policy(PolicyConfig {
        management_enabled: true,
        require_consent_for: vec![],
        ..Default::default()
    });
    let marker = root.path().join("executed");
    let created = server.manage_request(serde_json::from_value(serde_json::json!({
        "operation": "create", "name": "script-check", "description": "Explicit script",
        "skill_md": "Instructions do not require helper execution.",
        "scripts": [["helper.sh", format!("printf executed > {:?}\n", marker.to_str().unwrap())]]
    })).unwrap()).await.unwrap().0;
    let id = created.skill_id.unwrap();
    let valid: ExecInput = serde_json::from_value(serde_json::json!({
        "id": id, "script": "scripts/helper.sh", "dry_run": true
    }))
    .unwrap();
    assert_eq!(
        server.exec_callable(valid.clone()).await.unwrap().mode,
        "dry_run"
    );
    let invalid = ExecInput {
        script: Some("scripts/missing.sh".to_string()),
        ..valid
    };
    assert!(server.exec_callable(invalid).await.is_err());
    assert!(!marker.exists());
}
