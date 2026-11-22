//! Tests for core module: types, registry, policy, and persistence

use jiff::Timestamp;
use skillsrs::core::persistence::PersistenceLayer;
use skillsrs::core::policy::{ConsentLevel, PolicyConfig, PolicyEngine};
use skillsrs::core::registry::{Registry, ServerHealth, ServerInfo};
use skillsrs::core::{CallableId, CallableKind, CallableRecord, CostHints, RiskTier, SchemaDigest};
use skillsrs::execution::ExecutionTrace;
use tempfile::NamedTempFile;

#[test]
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
fn test_callable_id_parsing() {
    let tool_id = CallableId::tool("test-server", "test-tool", "abc123");
    assert_eq!(tool_id.kind().unwrap(), CallableKind::Tool);
    assert_eq!(tool_id.server_alias(), Some("test-server".to_string()));
    assert_eq!(tool_id.tool_name(), Some("test-tool".to_string()));

    let skill_id = CallableId::skill("my-skill", "1.0.0");
    assert_eq!(skill_id.kind().unwrap(), CallableKind::Skill);
    assert_eq!(skill_id.skill_name(), Some("my-skill".to_string()));
}

#[test]
fn test_risk_tier_ordering() {
    assert!(RiskTier::ReadOnly < RiskTier::Writes);
    assert!(RiskTier::Writes < RiskTier::Destructive);
    assert!(RiskTier::Destructive < RiskTier::Admin);
    assert!(RiskTier::Admin < RiskTier::Unknown);
}

#[test]
fn test_risk_tier_requires_consent() {
    assert!(!RiskTier::ReadOnly.requires_consent());
    assert!(RiskTier::Writes.requires_consent());
    assert!(RiskTier::Destructive.requires_consent());
    assert!(RiskTier::Admin.requires_consent());
    assert!(RiskTier::Unknown.requires_consent());
}

#[test]
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
fn test_risk_tier_from_str() {
    assert_eq!("read_only".parse::<RiskTier>().unwrap(), RiskTier::ReadOnly);
    assert_eq!("writes".parse::<RiskTier>().unwrap(), RiskTier::Writes);
    assert_eq!(
        "destructive".parse::<RiskTier>().unwrap(),
        RiskTier::Destructive
    );
    assert_eq!("admin".parse::<RiskTier>().unwrap(), RiskTier::Admin);
    assert_eq!("unknown".parse::<RiskTier>().unwrap(), RiskTier::Unknown);
    assert!("invalid".parse::<RiskTier>().is_err());
}

#[test]
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
fn test_schema_digest() {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" }
        }
    });
    let digest = SchemaDigest::from_schema(&schema).unwrap();
    assert_eq!(digest.short().len(), 8);
    assert!(digest.as_str().len() > 8);
}

#[test]
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
fn test_registry_basic_operations() {
    let registry = Registry::new();

    let record = create_test_tool_record("test-tool", "test-server");
    registry.register(record.clone()).unwrap();

    assert!(registry.contains(&record.id));
    assert_eq!(registry.len(), 1);

    let retrieved = registry.get(&record.id).unwrap();
    assert_eq!(retrieved.name, "test-tool");

    let by_fq = registry.get_by_fq_name("test-server.test-tool").unwrap();
    assert_eq!(by_fq.name, "test-tool");
}

#[test]
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
fn test_registry_remove() {
    let registry = Registry::new();
    let record = create_test_tool_record("test-tool", "test-server");

    registry.register(record.clone()).unwrap();
    assert_eq!(registry.len(), 1);

    registry.remove(&record.id);
    assert_eq!(registry.len(), 0);
    assert!(!registry.contains(&record.id));
}

#[test]
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
fn test_registry_by_kind() {
    let registry = Registry::new();

    let tool = create_test_tool_record("tool1", "server1");
    let skill = create_test_skill_record("skill1");

    registry.register(tool).unwrap();
    registry.register(skill).unwrap();

    let tools = registry.by_kind(CallableKind::Tool);
    let skills = registry.by_kind(CallableKind::Skill);

    assert_eq!(tools.len(), 1);
    assert_eq!(skills.len(), 1);
}

#[test]
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
fn test_registry_server_management() {
    let registry = Registry::new();

    let server_info = ServerInfo {
        alias: "test-server".to_string(),
        health: ServerHealth::Connected,
        tool_count: 5,
        last_refresh: Timestamp::now(),
        tags: vec!["test".to_string()],
    };

    registry.update_server(server_info);

    let info = registry.get_server("test-server").unwrap();
    assert_eq!(info.tool_count, 5);

    let stats = registry.stats();
    assert_eq!(stats.servers.len(), 1);
}

#[test]
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
fn test_policy_engine_allow_readonly() {
    let config = PolicyConfig {
        trusted_servers: vec!["server1".to_string()],
        ..Default::default()
    };
    let engine = PolicyEngine::new(config).unwrap();

    let record = create_test_tool_record("readonly-tool", "server1");
    let result = engine
        .authorize(&record, &serde_json::json!({}), ConsentLevel::None)
        .unwrap();

    assert!(result.allowed);
}

#[test]
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
fn test_policy_engine_deny_by_tag() {
    let config = PolicyConfig {
        deny_tags: vec!["dangerous".to_string()],
        ..Default::default()
    };
    let engine = PolicyEngine::new(config).unwrap();

    let mut record = create_test_tool_record("dangerous-tool", "server1");
    record.tags = vec!["dangerous".to_string()];

    let result = engine
        .authorize(&record, &serde_json::json!({}), ConsentLevel::None)
        .unwrap();

    assert!(!result.allowed);
    assert!(result.reason.contains("denied tag"));
}

#[test]
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
fn test_policy_engine_require_consent_for_writes() {
    let config = PolicyConfig {
        trusted_servers: vec!["server1".to_string()],
        ..Default::default()
    };
    let engine = PolicyEngine::new(config).unwrap();

    let mut record = create_test_tool_record("write-tool", "server1");
    record.risk_tier = RiskTier::Writes;

    // Without consent, should be denied
    let result = engine
        .authorize(&record, &serde_json::json!({}), ConsentLevel::None)
        .unwrap();
    assert!(!result.allowed);
    assert!(result.required_consent.is_some());

    // With user consent, should be allowed
    let result = engine
        .authorize(&record, &serde_json::json!({}), ConsentLevel::UserConfirmed)
        .unwrap();
    assert!(result.allowed);
}

#[test]
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
fn test_open_policy_is_machine_readable_and_unconditionally_invokes_off_panel_tools() {
    let engine = PolicyEngine::new(PolicyConfig::open()).unwrap();
    let mut record = create_test_tool_record("off-panel-write", "untrusted-provider");
    record.risk_tier = RiskTier::Admin;
    record.tags = vec!["blocked".to_string()];
    record.fq_name = "unlisted.off-panel-write".to_string();

    let result = engine
        .authorize(&record, &serde_json::json!({}), ConsentLevel::None)
        .unwrap();
    assert!(result.allowed);

    let state = engine.state();
    assert_eq!(state.mode, "open");
    assert!(state.unconditional);
    assert!(state.management_enabled);
}

#[test]
fn test_skills_policy_open_selector_overlays_standard_config() {
    let config = PolicyConfig::from_selector(PolicyConfig::default(), Some("OPEN"));
    assert_eq!(config.mode.to_string(), "open");
    assert!(config.require_consent_for.is_empty());
    assert!(config.deny_tags.is_empty());
    assert!(config.management_enabled);
}

#[test]
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast"
)]
fn partial_policy_configuration_keeps_security_defaults() {
    let config: PolicyConfig =
        serde_json::from_value(serde_json::json!({"max_exec_ms": 500})).unwrap();
    assert!(!config.management_enabled);
    assert!(config.enforce_server_trust);
    assert!(config.require_consent_for.contains(&"unknown".to_string()));
    let engine = PolicyEngine::new(config).unwrap();
    assert!(engine.authorize_management().is_err());
    assert_eq!(engine.check_timeout(None).unwrap(), 500);
    assert_eq!(engine.check_timeout(Some(900)).unwrap(), 500);
    assert_eq!(engine.check_timeout(Some(100)).unwrap(), 100);
    assert!(engine.check_timeout(Some(0)).is_err());
    assert!(
        serde_json::from_value::<PolicyConfig>(serde_json::json!({"max_calls_per_skill": 2}))
            .is_err()
    );
}

#[test]
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast"
)]
fn unknown_execution_requires_trusted_consent_but_metadata_does_not() {
    let engine = PolicyEngine::new(PolicyConfig {
        trusted_servers: vec!["server1".to_string()],
        ..Default::default()
    })
    .unwrap();
    let mut record = create_test_tool_record("unknown-tool", "server1");
    record.risk_tier = RiskTier::Unknown;
    assert!(engine.authorize_read(&record).allowed);
    assert!(
        !engine
            .authorize(&record, &serde_json::json!({}), ConsentLevel::None)
            .unwrap()
            .allowed
    );
    assert!(
        engine
            .authorize(&record, &serde_json::json!({}), ConsentLevel::UserConfirmed)
            .unwrap()
            .allowed
    );
}

#[test]
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast"
)]
fn trust_and_deny_policies_apply_to_metadata_reads() {
    let record = create_test_tool_record("readonly-tool", "server1");
    let engine = PolicyEngine::new(PolicyConfig::default()).unwrap();
    assert!(!engine.authorize_read(&record).allowed);
    let engine = PolicyEngine::new(PolicyConfig {
        trusted_servers: vec!["server1".to_string()],
        deny_patterns: vec!["*readonly*".to_string()],
        ..Default::default()
    })
    .unwrap();
    assert!(!engine.authorize_read(&record).allowed);
    let engine = PolicyEngine::new(PolicyConfig {
        enforce_server_trust: false,
        ..Default::default()
    })
    .unwrap();
    assert!(engine.authorize_read(&record).allowed);
}

#[tokio::test]
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
async fn test_persistence_lifecycle() {
    let temp_file = NamedTempFile::new().unwrap();
    let db_path = temp_file.path();

    let persistence = PersistenceLayer::new(db_path).await.unwrap();

    // Create a test callable
    let record = CallableRecord {
        id: CallableId::from("test://tool@1.0"),
        kind: CallableKind::Tool,
        fq_name: "test.tool".to_string(),
        name: "tool".to_string(),
        title: Some("Test Tool".to_string()),
        description: Some("A test".to_string()),
        tags: vec!["test".to_string()],
        input_schema: serde_json::json!({"type": "object"}),
        output_schema: None,
        schema_digest: SchemaDigest::from("test123"),
        server_alias: Some("test".to_string()),
        upstream_tool_name: Some("tool".to_string()),
        skill_version: None,
        uses: vec![],
        skill_directory: None,
        bundled_tools: vec![],
        additional_files: vec![],
        cost_hints: CostHints::default(),
        risk_tier: RiskTier::ReadOnly,
        last_seen: Timestamp::new(0, -750_000_000).unwrap(),
        sandbox_config: None,
    };

    // Save it
    persistence.save_callable(&record).await.unwrap();

    // Load it back
    let loaded = persistence.load_callable(&record.id).await.unwrap();
    assert_eq!(loaded.name, record.name);
    assert_eq!(loaded.kind, record.kind);
    assert_eq!(loaded.last_seen, Timestamp::from_second(-1).unwrap());

    let legacy_trace = serde_json::json!({
        "execution_id": "legacy",
        "started_at": "1969-12-31T23:59:59.250Z",
        "completed_at": "1970-01-01T00:00:00Z",
        "total_duration_ms": 750,
        "steps": [],
    });
    persistence
        .record_execution(
            "legacy",
            &record.id,
            &serde_json::Value::Null,
            None,
            false,
            Some(750),
            record.last_seen,
            Some(Timestamp::UNIX_EPOCH),
            Some(&legacy_trace),
        )
        .await
        .unwrap();
    let history = persistence
        .get_execution_history(&record.id, 1)
        .await
        .unwrap();
    let execution = history.into_iter().next().unwrap();
    assert_eq!(execution.started_at, loaded.last_seen);
    assert_eq!(execution.completed_at, Some(Timestamp::UNIX_EPOCH));
    assert_eq!(execution.trace.as_ref(), Some(&legacy_trace));
    let trace: ExecutionTrace = serde_json::from_value(execution.trace.unwrap()).unwrap();
    assert_eq!(trace.started_at, record.last_seen);
    assert_eq!(trace.completed_at, Some(Timestamp::UNIX_EPOCH));

    // Delete it
    persistence.delete_callable(&record.id).await.unwrap();

    // Should not be found
    assert!(persistence.load_callable(&record.id).await.is_err());

    persistence.close().await;
}

// Helper functions

#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
fn create_test_tool_record(name: &str, server: &str) -> CallableRecord {
    let schema = serde_json::json!({"type": "object"});
    let digest = SchemaDigest::from_schema(&schema).unwrap();
    let id = CallableId::tool(server, name, digest.as_str());

    CallableRecord {
        id,
        kind: CallableKind::Tool,
        fq_name: format!("{server}.{name}"),
        name: name.to_string(),
        title: Some(name.to_string()),
        description: Some("A test tool".to_string()),
        tags: vec!["test".to_string()],
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

#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
fn create_test_skill_record(name: &str) -> CallableRecord {
    let schema = serde_json::json!({"type": "object"});
    let digest = SchemaDigest::from_schema(&schema).unwrap();
    let id = CallableId::skill(name, "1.0.0");

    CallableRecord {
        id,
        kind: CallableKind::Skill,
        fq_name: format!("skill.{name}"),
        name: name.to_string(),
        title: Some(name.to_string()),
        description: Some("A test skill".to_string()),
        tags: vec!["skill".to_string()],
        input_schema: schema,
        output_schema: None,
        schema_digest: digest,
        server_alias: None,
        upstream_tool_name: None,
        skill_version: Some("1.0.0".to_string()),
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
