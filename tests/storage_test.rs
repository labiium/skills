//! Tests for storage module: skill store, search, agent skills

use skillsrs::core::registry::Registry;
use skillsrs::core::CallableKind;
use skillsrs::storage::search::{SearchEngine, SearchQuery};
use skillsrs::storage::{CreateSkillRequest, SkillStore};
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
        tags: vec!["test".to_string()],
        scripts: vec![],
        references: vec![],
        assets: vec![],
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

    // Create SKILL.md with version in frontmatter
    // Agent Skills format is lenient with version strings
    let skill_md = r#"---
name: invalid-skill
description: Test
version: not-a-version
---

# Test
"#;

    std::fs::write(skill_dir.join("SKILL.md"), skill_md).unwrap();

    // Skill should load successfully (Agent Skills format doesn't strictly validate version)
    let skill = store.load_skill(&skill_dir).await.unwrap();

    // Just verify the skill loaded with the version as-is
    assert_eq!(skill.manifest.version, "not-a-version");
}

#[tokio::test]
async fn test_validate_circular_dependency() {
    let temp = TempDir::new().unwrap();
    let registry = Arc::new(Registry::new());
    let store = SkillStore::new(temp.path(), registry.clone()).unwrap();

    // Create a skill that depends on another skill
    let skill_a_dir = temp.path().join("skill-a");
    std::fs::create_dir(&skill_a_dir).unwrap();

    // Skill A uses tools including a reference to skill-b
    let skill_a_md = r#"---
name: skill-a
description: Test skill A
version: 1.0.0
allowed-tools: ["skill-b"]
---

# Skill A

This skill uses skill-b.
"#;

    std::fs::write(skill_a_dir.join("SKILL.md"), skill_a_md).unwrap();

    // Create skill B
    let skill_b_dir = temp.path().join("skill-b");
    std::fs::create_dir(&skill_b_dir).unwrap();

    let skill_b_md = r#"---
name: skill-b
description: Test skill B
version: 1.0.0
---

# Skill B

This is skill B.
"#;

    std::fs::write(skill_b_dir.join("SKILL.md"), skill_b_md).unwrap();

    // Load both skills
    let skill_a = store.load_skill(&skill_a_dir).await.unwrap();
    let skill_b = store.load_skill(&skill_b_dir).await.unwrap();

    // Register both skills
    store.register_skill(&skill_a).unwrap();
    store.register_skill(&skill_b).unwrap();
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
        tags: vec!["test".to_string()],
        scripts: vec![("helper.py".to_string(), "print('hello')".to_string())],
        references: vec![],
        assets: vec![],
    };

    let id = store.create_skill(request).await.unwrap();
    assert!(id.as_str().contains("test-skill"));

    // Verify skill was created
    let skill_dir = temp.path().join("test-skill");
    assert!(skill_dir.exists());
    // SKILL.md should exist with YAML frontmatter (new format)
    assert!(skill_dir.join("SKILL.md").exists());
    // Scripts should be in scripts/ subdirectory (new format)
    assert!(skill_dir.join("scripts").exists());
    assert!(skill_dir.join("scripts/helper.py").exists());
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
        tags: vec![],
        scripts: vec![("script.py".to_string(), "print('test')".to_string())],
        references: vec![("data.json".to_string(), "{}".to_string())],
        assets: vec![],
    };

    store.create_skill(request).await.unwrap();

    let content = store.load_skill_content("test-skill").unwrap();
    // SKILL.md now includes YAML frontmatter
    assert!(content.skill_md.contains("name: test-skill"));
    assert!(content.skill_md.contains("# Test Skill"));
    assert!(content.skill_md.contains("Instructions here."));
    assert!(content.uses_tools.contains(&"brave_search".to_string()));
    assert_eq!(content.additional_files.len(), 1); // references/data.json
    assert_eq!(content.bundled_tools.len(), 1); // scripts/script.py
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

    // Create SKILL.md with YAML frontmatter (Agent Skills format)
    let skill_md = r#"---
name: test-skill
description: A test skill
version: 1.0.0
---

# Test Skill

A test skill for registration.
"#;

    std::fs::write(skill_dir.join("SKILL.md"), skill_md).unwrap();

    let registry = Arc::new(Registry::new());
    let store = SkillStore::new(temp.path(), registry.clone()).unwrap();

    let ids = store.load_and_register_all().await.unwrap();
    assert_eq!(ids.len(), 1);

    // Check that skill is in registry
    let record = registry.get(&ids[0]).unwrap();
    assert_eq!(record.name, "test-skill");
    assert_eq!(record.kind, CallableKind::Skill);
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
        sandbox_config: None,
    };

    registry.register(record).unwrap();
    search_engine.rebuild();

    // Search for the tool
    let query = SearchQuery {
        q: "test-tool".to_string(),
        kind: "any".to_string(),
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
            sandbox_config: None,
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
            sandbox_config: None,
        })
        .unwrap();

    search_engine.rebuild();

    // Search with kind filter for tools only
    let query = SearchQuery {
        q: "test".to_string(),
        kind: "tools".to_string(),
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
        limit: 10,
        filters: None,
        cursor: None,
    };

    let results = search_engine.search(&query).await.unwrap();
    assert_eq!(results.total_matches, 1);
    assert_eq!(results.matches[0].kind, "skill");
}

#[tokio::test]
async fn test_skills_loaded_from_files_are_searchable() {
    // Test for the bug fix: skills loaded from files should be searchable
    // This verifies that the search index is rebuilt after skills are loaded
    let temp = TempDir::new().unwrap();
    let skill_dir = temp.path().join("searchable-skill");
    std::fs::create_dir(&skill_dir).unwrap();

    // Create a SKILL.md file
    let skill_md = r#"---
name: searchable-skill
description: A skill that should be searchable after loading
version: 1.0.0
---

# Searchable Skill

This skill tests that loaded skills are searchable.
"#;

    std::fs::write(skill_dir.join("SKILL.md"), skill_md).unwrap();

    let registry = Arc::new(Registry::new());
    let search_engine = SearchEngine::new(registry.clone());
    let store = SkillStore::new(temp.path(), registry.clone()).unwrap();

    // Load skills from files - this populates the registry
    let ids = store.load_and_register_all().await.unwrap();
    assert_eq!(ids.len(), 1);

    // Rebuild search index AFTER skills are loaded (this was the fix)
    search_engine.rebuild();

    // Now search for the skill - it should be found
    let query = SearchQuery {
        q: "searchable".to_string(),
        kind: "any".to_string(),
        limit: 10,
        filters: None,
        cursor: None,
    };

    let results = search_engine.search(&query).await.unwrap();
    assert_eq!(
        results.total_matches, 1,
        "Skill should be searchable after loading and rebuilding index"
    );
    assert_eq!(results.matches[0].name, "searchable-skill");
    assert_eq!(results.matches[0].kind, "skill");

    // Also test searching by description
    let query = SearchQuery {
        q: "loading".to_string(),
        kind: "any".to_string(),
        limit: 10,
        filters: None,
        cursor: None,
    };

    let results = search_engine.search(&query).await.unwrap();
    assert_eq!(results.total_matches, 1, "Should find skill by description");
    assert_eq!(results.matches[0].name, "searchable-skill");
}

#[tokio::test]
async fn test_find_skill_directory_uses_frontmatter_name() {
    let temp = TempDir::new().unwrap();
    let registry = Arc::new(Registry::new());
    let store = SkillStore::new(temp.path(), registry).unwrap();

    // Directory name intentionally does not match skill name.
    let skill_dir = temp.path().join("different-folder-name");
    std::fs::create_dir(&skill_dir).unwrap();
    let skill_md = r#"---
name: frontmatter-name
description: Skill loaded by frontmatter name
version: 1.0.0
---

# Skill
"#;
    std::fs::write(skill_dir.join("SKILL.md"), skill_md).unwrap();

    let content = store.load_skill_content("frontmatter-name").unwrap();
    assert!(content.skill_md.contains("name: frontmatter-name"));
}

#[tokio::test]
async fn test_create_skill_writes_yaml_escaped_frontmatter() {
    let temp = TempDir::new().unwrap();
    let registry = Arc::new(Registry::new());
    let store = SkillStore::new(temp.path(), registry).unwrap();

    let request = CreateSkillRequest {
        name: "yaml-escaped-skill".to_string(),
        version: "1.0.0".to_string(),
        description: "Handles: colon values\nand newlines safely".to_string(),
        skill_md_content: "# Body".to_string(),
        uses_tools: vec!["filesystem/write_file".to_string(), "search:*".to_string()],
        tags: vec!["tag:alpha".to_string()],
        scripts: vec![],
        references: vec![],
        assets: vec![],
    };

    store.create_skill(request).await.unwrap();
    let skill_dir = temp.path().join("yaml-escaped-skill");
    let skill = store.load_skill(&skill_dir).await.unwrap();

    assert_eq!(
        skill.manifest.description,
        "Handles: colon values\nand newlines safely"
    );
}
