//! Tests for core module: types, registry, policy, and persistence

use skillsrs::core::persistence::PersistenceLayer;
use skillsrs::core::policy::{ConsentLevel, PolicyConfig, PolicyEngine};
use skillsrs::core::registry::{Registry, ServerHealth, ServerInfo};
use skillsrs::core::{CallableId, CallableKind, CallableRecord, CostHints, RiskTier, SchemaDigest};
use tempfile::NamedTempFile;

#[test]
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
    assert!(!RiskTier::Unknown.requires_consent());
}

#[test]
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
fn test_registry_server_management() {
    let registry = Registry::new();

    let server_info = ServerInfo {
        alias: "test-server".to_string(),
        health: ServerHealth::Connected,
        tool_count: 5,
        last_refresh: chrono::Utc::now(),
        tags: vec!["test".to_string()],
    };

    registry.update_server(server_info);

    let info = registry.get_server("test-server").unwrap();
    assert_eq!(info.tool_count, 5);

    let stats = registry.stats();
    assert_eq!(stats.servers.len(), 1);
}

#[tokio::test]
async fn test_policy_engine_allow_readonly() {
    let config = PolicyConfig::default();
    let engine = PolicyEngine::new(config).unwrap();

    let record = create_test_tool_record("readonly-tool", "server1");
    let result = engine
        .authorize(&record, &serde_json::json!({}), ConsentLevel::None)
        .await
        .unwrap();

    assert!(result.allowed);
}

#[tokio::test]
async fn test_policy_engine_deny_by_tag() {
    let config = PolicyConfig {
        deny_tags: vec!["dangerous".to_string()],
        ..Default::default()
    };
    let engine = PolicyEngine::new(config).unwrap();

    let mut record = create_test_tool_record("dangerous-tool", "server1");
    record.tags = vec!["dangerous".to_string()];

    let result = engine
        .authorize(&record, &serde_json::json!({}), ConsentLevel::None)
        .await
        .unwrap();

    assert!(!result.allowed);
    assert!(result.reason.contains("denied tag"));
}

#[tokio::test]
async fn test_policy_engine_require_consent_for_writes() {
    let config = PolicyConfig::default();
    let engine = PolicyEngine::new(config).unwrap();

    let mut record = create_test_tool_record("write-tool", "server1");
    record.risk_tier = RiskTier::Writes;

    // Without consent, should be denied
    let result = engine
        .authorize(&record, &serde_json::json!({}), ConsentLevel::None)
        .await
        .unwrap();
    assert!(!result.allowed);
    assert!(result.required_consent.is_some());

    // With user consent, should be allowed
    let result = engine
        .authorize(&record, &serde_json::json!({}), ConsentLevel::UserConfirmed)
        .await
        .unwrap();
    assert!(result.allowed);
}

#[tokio::test]
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
        last_seen: chrono::Utc::now(),
    };

    // Save it
    persistence.save_callable(&record).await.unwrap();

    // Load it back
    let loaded = persistence.load_callable(&record.id).await.unwrap();
    assert_eq!(loaded.name, record.name);
    assert_eq!(loaded.kind, record.kind);

    // Delete it
    persistence.delete_callable(&record.id).await.unwrap();

    // Should not be found
    assert!(persistence.load_callable(&record.id).await.is_err());

    persistence.close().await;
}

// Helper functions

fn create_test_tool_record(name: &str, server: &str) -> CallableRecord {
    let schema = serde_json::json!({"type": "object"});
    let digest = SchemaDigest::from_schema(&schema).unwrap();
    let id = CallableId::tool(server, name, digest.as_str());

    CallableRecord {
        id,
        kind: CallableKind::Tool,
        fq_name: format!("{}.{}", server, name),
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
        last_seen: chrono::Utc::now(),
    }
}

fn create_test_skill_record(name: &str) -> CallableRecord {
    let schema = serde_json::json!({"type": "object"});
    let digest = SchemaDigest::from_schema(&schema).unwrap();
    let id = CallableId::skill(name, "1.0.0");

    CallableRecord {
        id,
        kind: CallableKind::Skill,
        fq_name: format!("skill.{}", name),
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
        last_seen: chrono::Utc::now(),
    }
}
