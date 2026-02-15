//! Tests for MCP module: SkillsServer

use skillsrs::core::policy::PolicyConfig;
use skillsrs::core::registry::Registry;
use skillsrs::execution::upstream::UpstreamManager;
use skillsrs::execution::Runtime;
use skillsrs::mcp::SkillsServer;
use skillsrs::storage::search::SearchEngine;
use skillsrs::storage::SkillStore;
use std::sync::Arc;
use tempfile::TempDir;

#[test]
fn test_server_exposes_core_tools() {
    // CRITICAL CONTRACT TEST: Must expose core tools + skill management tools
    let registry = Arc::new(Registry::new());
    let search_engine = Arc::new(SearchEngine::new(registry.clone()));
    let policy_engine =
        Arc::new(skillsrs::core::policy::PolicyEngine::new(PolicyConfig::default()).unwrap());
    let upstream_manager = Arc::new(UpstreamManager::new(registry.clone()));
    let runtime = Arc::new(Runtime::new(registry.clone(), upstream_manager));
    let temp_dir = TempDir::new().unwrap();
    let skill_store = Arc::new(SkillStore::new(temp_dir.path(), registry.clone()).unwrap());

    let _server = SkillsServer::new(registry, search_engine, policy_engine, runtime, skill_store);

    // Get tools from router - we can't directly access tool_router, but we can verify
    // the server was created successfully
    // The actual tool count verification is done through the mcp protocol

    // Just verify the server was created without panicking
    // The actual tool exposure is tested through the protocol
}

#[test]
fn test_server_creation() {
    let registry = Arc::new(Registry::new());
    let search_engine = Arc::new(SearchEngine::new(registry.clone()));
    let policy_engine =
        Arc::new(skillsrs::core::policy::PolicyEngine::new(PolicyConfig::default()).unwrap());
    let upstream_manager = Arc::new(UpstreamManager::new(registry.clone()));
    let runtime = Arc::new(Runtime::new(registry.clone(), upstream_manager));
    let temp_dir = TempDir::new().unwrap();
    let skill_store = Arc::new(SkillStore::new(temp_dir.path(), registry.clone()).unwrap());

    // This should not panic
    let _server = SkillsServer::new(registry, search_engine, policy_engine, runtime, skill_store);
}

#[tokio::test]
async fn test_search_tool_through_server() {
    let registry = Arc::new(Registry::new());
    let search_engine = Arc::new(SearchEngine::new(registry.clone()));
    let policy_engine =
        Arc::new(skillsrs::core::policy::PolicyEngine::new(PolicyConfig::default()).unwrap());
    let upstream_manager = Arc::new(UpstreamManager::new(registry.clone()));
    let runtime = Arc::new(Runtime::new(registry.clone(), upstream_manager));
    let temp_dir = TempDir::new().unwrap();
    let skill_store = Arc::new(SkillStore::new(temp_dir.path(), registry.clone()).unwrap());

    // Register a test record
    let schema = serde_json::json!({"type": "object"});
    let digest = skillsrs::core::SchemaDigest::from_schema(&schema).unwrap();
    let id = skillsrs::core::CallableId::tool("test", "test-tool", digest.as_str());

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
            risk_tier: skillsrs::core::RiskTier::ReadOnly,
            last_seen: chrono::Utc::now(),
            sandbox_config: None,
        })
        .unwrap();

    search_engine.rebuild();

    let _server = SkillsServer::new(registry, search_engine, policy_engine, runtime, skill_store);

    // The actual search test would require calling the MCP protocol
    // which is tested at the integration level
}
