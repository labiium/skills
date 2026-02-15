//! Tests for storage module: skill store, search, agent skills

use skillsrs::core::registry::Registry;
use skillsrs::core::{CallableKind, RiskTier};
use skillsrs::storage::search::{SearchEngine, SearchQuery};
use skillsrs::storage::{
    CreateSkillRequest, EntrypointType, SkillHints, SkillManifest, SkillStore, ToolPolicy,
};
use std::sync::Arc;
use tempfile::TempDir;

#[tokio::test]
async fn test_validate_skill() {
    let temp = TempDir::new().unwrap();
    let registry = Arc::new(Registry::new());
    let store = SkillStore::new(temp.path(), registry.clone()).unwrap();

    let request = CreateSkillRequest {
        name: "valid-skill".to_string(),
        version: "1.0.0".to_string(),
        description: "A valid test skill with proper description".to_string(),
        skill_md_content: "# Valid Skill\n\nThis has proper documentation.".to_string(),
        uses_tools: vec![],
        bundled_files: vec![],
        tags: vec!["test".to_string()],
    };

    store.create_skill(request).await.unwrap();
    let skill = store
        .load_skill(&temp.path().join("valid-skill"))
        .await
        .unwrap();

    let result = store.validate_skill(&skill);
    assert!(result.valid, "Skill should be valid: {:?}", result.errors);
}

#[tokio::test]
async fn test_validate_invalid_version() {
    let temp = TempDir::new().unwrap();
    let registry = Arc::new(Registry::new());
    let store = SkillStore::new(temp.path(), registry.clone()).unwrap();

    let skill_dir = temp.path().join("invalid-skill");
    std::fs::create_dir(&skill_dir).unwrap();

    let manifest = SkillManifest {
        id: "invalid-skill".to_string(),
        title: "Invalid Skill".to_string(),
        version: "not-a-version".to_string(),
        description: "Test".to_string(),
        inputs: serde_json::json!({"type": "object"}),
        outputs: None,
        entrypoint: EntrypointType::Prompted,
        tool_policy: ToolPolicy {
            allow: vec![],
            deny: vec![],
            required: vec![],
        },
        hints: SkillHints::default(),
        risk_tier: None,
    };

    std::fs::write(
        skill_dir.join("skill.json"),
        serde_json::to_string_pretty(&manifest).unwrap(),
    )
    .unwrap();

    std::fs::write(skill_dir.join("SKILL.md"), "# Test").unwrap();

    let skill = store.load_skill(&skill_dir).await.unwrap();
    let result = store.validate_skill(&skill);

    assert!(!result.valid);
    assert!(result.errors.iter().any(|e| e.contains("Invalid version")));
}

#[tokio::test]
async fn test_validate_circular_dependency() {
    let temp = TempDir::new().unwrap();
    let registry = Arc::new(Registry::new());
    let store = SkillStore::new(temp.path(), registry.clone()).unwrap();

    let skill_dir = temp.path().join("circular-skill");
    std::fs::create_dir(&skill_dir).unwrap();

    let manifest = SkillManifest {
        id: "circular-skill".to_string(),
        title: "Circular".to_string(),
        version: "1.0.0".to_string(),
        description: "Test".to_string(),
        inputs: serde_json::json!({"type": "object"}),
        outputs: None,
        entrypoint: EntrypointType::Prompted,
        tool_policy: ToolPolicy {
            allow: vec!["circular-skill".to_string()], // References itself!
            deny: vec![],
            required: vec![],
        },
        hints: SkillHints::default(),
        risk_tier: None,
    };

    std::fs::write(
        skill_dir.join("skill.json"),
        serde_json::to_string_pretty(&manifest).unwrap(),
    )
    .unwrap();

    std::fs::write(skill_dir.join("SKILL.md"), "# Test").unwrap();

    let skill = store.load_skill(&skill_dir).await.unwrap();
    let result = store.validate_skill(&skill);

    assert!(!result.valid);
    assert!(result
        .errors
        .iter()
        .any(|e| e.contains("Circular dependency")));
}

#[tokio::test]
async fn test_create_skill() {
    let temp = TempDir::new().unwrap();
    let registry = Arc::new(Registry::new());
    let store = SkillStore::new(temp.path(), registry.clone()).unwrap();

    let request = CreateSkillRequest {
        name: "test-skill".to_string(),
        version: "1.0.0".to_string(),
        description: "A test skill".to_string(),
        skill_md_content: "# Test Skill\n\nThis is a test skill.".to_string(),
        uses_tools: vec!["brave_search".to_string()],
        bundled_files: vec![("helper.py".to_string(), "print('hello')".to_string())],
        tags: vec!["test".to_string()],
    };

    let id = store.create_skill(request).await.unwrap();
    assert!(id.as_str().contains("test-skill"));

    // Verify skill was created
    let skill_dir = temp.path().join("test-skill");
    assert!(skill_dir.exists());
    assert!(skill_dir.join("skill.json").exists());
    assert!(skill_dir.join("SKILL.md").exists());
    assert!(skill_dir.join("helper.py").exists());
}

#[tokio::test]
async fn test_load_skill_content() {
    let temp = TempDir::new().unwrap();
    let registry = Arc::new(Registry::new());
    let store = SkillStore::new(temp.path(), registry.clone()).unwrap();

    let request = CreateSkillRequest {
        name: "test-skill".to_string(),
        version: "1.0.0".to_string(),
        description: "A test skill".to_string(),
        skill_md_content: "# Test Skill\n\nInstructions here.".to_string(),
        uses_tools: vec!["brave_search".to_string()],
        bundled_files: vec![
            ("script.py".to_string(), "print('test')".to_string()),
            ("data.json".to_string(), "{}".to_string()),
        ],
        tags: vec![],
    };

    store.create_skill(request).await.unwrap();

    let content = store.load_skill_content("test-skill").unwrap();
    assert_eq!(content.skill_md, "# Test Skill\n\nInstructions here.");
    assert!(content.uses_tools.contains(&"brave_search".to_string()));
    assert_eq!(content.additional_files.len(), 1); // data.json
    assert_eq!(content.bundled_tools.len(), 1); // script.py
}

#[tokio::test]
async fn test_load_empty_store() {
    let temp = TempDir::new().unwrap();
    let registry = Arc::new(Registry::new());
    let store = SkillStore::new(temp.path(), registry).unwrap();

    let skills = store.load_all().await.unwrap();
    assert_eq!(skills.len(), 0);
}

#[tokio::test]
async fn test_register_skill() {
    let temp = TempDir::new().unwrap();
    let skill_dir = temp.path().join("test-skill");
    std::fs::create_dir(&skill_dir).unwrap();

    let manifest = SkillManifest {
        id: "test-skill".to_string(),
        title: "Test Skill".to_string(),
        version: "1.0.0".to_string(),
        description: "A test skill".to_string(),
        inputs: serde_json::json!({
            "type": "object",
            "properties": {
                "input": { "type": "string" }
            }
        }),
        outputs: None,
        entrypoint: EntrypointType::Workflow,
        tool_policy: ToolPolicy {
            allow: vec!["*".to_string()],
            deny: vec![],
            required: vec![],
        },
        hints: SkillHints::default(),
        risk_tier: Some("read_only".to_string()),
    };

    std::fs::write(
        skill_dir.join("skill.json"),
        serde_json::to_string_pretty(&manifest).unwrap(),
    )
    .unwrap();

    let registry = Arc::new(Registry::new());
    let store = SkillStore::new(temp.path(), registry.clone()).unwrap();

    let ids = store.load_and_register_all().await.unwrap();
    assert_eq!(ids.len(), 1);

    // Check that skill is in registry
    let record = registry.get(&ids[0]).unwrap();
    assert_eq!(record.name, "test-skill");
    assert_eq!(record.kind, CallableKind::Skill);
    assert_eq!(record.risk_tier, RiskTier::ReadOnly);
}

#[tokio::test]
async fn test_search_engine() {
    let registry = Arc::new(Registry::new());
    let search_engine = SearchEngine::new(registry.clone());

    // Create and register a test record
    let schema = serde_json::json!({"type": "object"});
    let digest = skillsrs::core::SchemaDigest::from_schema(&schema).unwrap();
    let id = skillsrs::core::CallableId::tool("test-server", "test-tool", digest.as_str());

    let record = skillsrs::core::CallableRecord {
        id,
        kind: skillsrs::core::CallableKind::Tool,
        fq_name: "test-server.test-tool".to_string(),
        name: "test-tool".to_string(),
        title: Some("Test Tool".to_string()),
        description: Some("A test tool for searching".to_string()),
        tags: vec!["test".to_string(), "search".to_string()],
        input_schema: schema,
        output_schema: None,
        schema_digest: digest,
        server_alias: Some("test-server".to_string()),
        upstream_tool_name: Some("test-tool".to_string()),
        skill_version: None,
        uses: vec![],
        skill_directory: None,
        bundled_tools: vec![],
        additional_files: vec![],
        cost_hints: skillsrs::core::CostHints::default(),
        risk_tier: skillsrs::core::RiskTier::ReadOnly,
        last_seen: chrono::Utc::now(),
    };

    registry.register(record).unwrap();
    search_engine.rebuild();

    // Search for the tool
    let query = SearchQuery {
        q: "test-tool".to_string(),
        kind: "any".to_string(),
        mode: "literal".to_string(),
        limit: 10,
        filters: None,
        cursor: None,
    };

    let results = search_engine.search(&query).await.unwrap();
    assert_eq!(results.total_matches, 1);
    assert_eq!(results.matches[0].name, "test-tool");
}

#[tokio::test]
async fn test_search_with_filters() {
    let registry = Arc::new(Registry::new());
    let search_engine = SearchEngine::new(registry.clone());

    // Create test records
    let schema = serde_json::json!({"type": "object"});
    let digest = skillsrs::core::SchemaDigest::from_schema(&schema).unwrap();

    let tool_id = skillsrs::core::CallableId::tool("server1", "tool1", digest.as_str());
    let skill_id = skillsrs::core::CallableId::skill("skill1", "1.0.0");

    registry
        .register(skillsrs::core::CallableRecord {
            id: tool_id,
            kind: skillsrs::core::CallableKind::Tool,
            fq_name: "server1.tool1".to_string(),
            name: "tool1".to_string(),
            title: Some("Tool One".to_string()),
            description: Some("A test tool".to_string()),
            tags: vec!["test".to_string()],
            input_schema: schema.clone(),
            output_schema: None,
            schema_digest: digest.clone(),
            server_alias: Some("server1".to_string()),
            upstream_tool_name: Some("tool1".to_string()),
            skill_version: None,
            uses: vec![],
            skill_directory: None,
            bundled_tools: vec![],
            additional_files: vec![],
            cost_hints: skillsrs::core::CostHints::default(),
            risk_tier: skillsrs::core::RiskTier::ReadOnly,
            last_seen: chrono::Utc::now(),
        })
        .unwrap();

    registry
        .register(skillsrs::core::CallableRecord {
            id: skill_id,
            kind: skillsrs::core::CallableKind::Skill,
            fq_name: "skill.skill1".to_string(),
            name: "skill1".to_string(),
            title: Some("Skill One".to_string()),
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
            cost_hints: skillsrs::core::CostHints::default(),
            risk_tier: skillsrs::core::RiskTier::ReadOnly,
            last_seen: chrono::Utc::now(),
        })
        .unwrap();

    search_engine.rebuild();

    // Search with kind filter for tools only
    let query = SearchQuery {
        q: "test".to_string(),
        kind: "tools".to_string(),
        mode: "literal".to_string(),
        limit: 10,
        filters: None,
        cursor: None,
    };

    let results = search_engine.search(&query).await.unwrap();
    assert_eq!(results.total_matches, 1);
    assert_eq!(results.matches[0].kind, "tool");

    // Search with kind filter for skills only
    let query = SearchQuery {
        q: "test".to_string(),
        kind: "skills".to_string(),
        mode: "literal".to_string(),
        limit: 10,
        filters: None,
        cursor: None,
    };

    let results = search_engine.search(&query).await.unwrap();
    assert_eq!(results.total_matches, 1);
    assert_eq!(results.matches[0].kind, "skill");
}
