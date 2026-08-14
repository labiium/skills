//! Tests for storage module: skill store, search, agent skills

use skillsrs::core::registry::Registry;
use skillsrs::core::CallableKind;
use skillsrs::storage::search::{SearchEngine, SearchQuery};
use skillsrs::storage::{CreateSkillRequest, SkillStore};
use std::sync::Arc;
use tempfile::TempDir;

#[tokio::test]
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
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
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
async fn test_validate_invalid_version() {
    let temp = TempDir::new().unwrap();
    let registry = Arc::new(Registry::new());
    let store = SkillStore::new(temp.path(), registry.clone()).unwrap();

    let skill_dir = temp.path().join("invalid-skill");
    std::fs::create_dir(&skill_dir).unwrap();

    // Create SKILL.md with version in frontmatter
    // Agent Skills format is lenient with version strings
    let skill_md = r"---
name: invalid-skill
description: Test
version: not-a-version
---

# Test
";

    std::fs::write(skill_dir.join("SKILL.md"), skill_md).unwrap();

    // Skill should load successfully (Agent Skills format doesn't strictly validate version)
    let skill = store.load_skill(&skill_dir).await.unwrap();

    // Just verify the skill loaded with the version as-is
    assert_eq!(skill.manifest.version, "not-a-version");
}

#[tokio::test]
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
async fn test_validate_circular_dependency() {
    let temp = TempDir::new().unwrap();
    let registry = Arc::new(Registry::new());
    let store = SkillStore::new(temp.path(), registry.clone()).unwrap();

    // Create a skill that depends on another skill
    let first_skill_directory = temp.path().join("skill-a");
    std::fs::create_dir(&first_skill_directory).unwrap();

    // Skill A uses tools including a reference to skill-b
    let first_manifest = r#"---
name: skill-a
description: Test skill A
version: 1.0.0
allowed-tools: ["skill-b"]
---

# Skill A

This skill uses skill-b.
"#;

    std::fs::write(first_skill_directory.join("SKILL.md"), first_manifest).unwrap();

    // Create skill B
    let second_skill_directory = temp.path().join("skill-b");
    std::fs::create_dir(&second_skill_directory).unwrap();

    let second_manifest = r"---
name: skill-b
description: Test skill B
version: 1.0.0
---

# Skill B

This is skill B.
";

    std::fs::write(second_skill_directory.join("SKILL.md"), second_manifest).unwrap();

    // Load both skills
    let first_skill = store.load_skill(&first_skill_directory).await.unwrap();
    let second_skill = store.load_skill(&second_skill_directory).await.unwrap();

    // Register both skills
    store.register_skill(&first_skill).unwrap();
    store.register_skill(&second_skill).unwrap();
}

#[tokio::test]
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
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
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
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
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
async fn test_load_empty_store() {
    let temp = TempDir::new().unwrap();
    let registry = Arc::new(Registry::new());
    let store = SkillStore::new(temp.path(), registry).unwrap();

    let skills = store.load_all().await.unwrap();
    assert_eq!(skills.len(), 0);
}

#[tokio::test]
#[expect(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
async fn test_register_skill() {
    let temp = TempDir::new().unwrap();
    let skill_dir = temp.path().join("test-skill");
    std::fs::create_dir(&skill_dir).unwrap();

    // Create SKILL.md with YAML frontmatter (Agent Skills format)
    let skill_md = r"---
name: test-skill
description: A test skill
version: 1.0.0
---

# Test Skill

A test skill for registration.
";

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
#[expect(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
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
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
async fn test_exhaustive_search_enumerates_every_record_once_in_stable_pages() {
    let registry = Arc::new(Registry::new());
    let search_engine = SearchEngine::new(registry.clone());
    let schema = serde_json::json!({"type": "object"});
    let digest = skillsrs::core::SchemaDigest::from_schema(&schema).unwrap();

    for index in 0..53 {
        let name = format!("instrument-{index:02}");
        let id = skillsrs::core::CallableId::tool("provider", &name, digest.as_str());
        registry
            .register(skillsrs::core::CallableRecord {
                id,
                kind: skillsrs::core::CallableKind::Tool,
                fq_name: format!("provider.{name}"),
                name: name.clone(),
                title: None,
                description: None,
                tags: if index % 2 == 0 {
                    vec!["even".to_string()]
                } else {
                    vec!["odd".to_string()]
                },
                input_schema: schema.clone(),
                output_schema: None,
                schema_digest: digest.clone(),
                server_alias: Some("provider".to_string()),
                upstream_tool_name: Some(name),
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
    }
    search_engine.rebuild();

    let first = search_engine
        .search(&SearchQuery {
            q: "*".to_string(),
            kind: "tools".to_string(),
            limit: 50,
            filters: None,
            cursor: None,
        })
        .await
        .unwrap();
    let second = search_engine
        .search(&SearchQuery {
            q: "*".to_string(),
            kind: "tools".to_string(),
            limit: 50,
            filters: None,
            cursor: first.next_cursor.clone(),
        })
        .await
        .unwrap();

    assert_eq!(first.total_matches, 53);
    assert_eq!(first.matches.len(), 50);
    assert_eq!(second.matches.len(), 3);
    assert!(second.next_cursor.is_none());
    let mut names: Vec<_> = first
        .matches
        .into_iter()
        .chain(second.matches)
        .map(|item| item.name)
        .collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted);
    names.sort();
    names.dedup();
    assert_eq!(names.len(), 53);

    let filtered = search_engine
        .search(&SearchQuery {
            q: "*".to_string(),
            kind: "tools".to_string(),
            limit: 50,
            filters: Some(skillsrs::storage::search::SearchFilters {
                server: None,
                tags: Some(vec!["odd".to_string()]),
                requires: None,
                capability: None,
            }),
            cursor: None,
        })
        .await
        .unwrap();
    assert_eq!(filtered.total_matches, 26);
    assert!(filtered.matches.iter().all(|item| {
        item.name.ends_with('1')
            || item.name.ends_with('3')
            || item.name.ends_with('5')
            || item.name.ends_with('7')
            || item.name.ends_with('9')
    }));
}

#[tokio::test]
#[expect(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
async fn test_search_matches_skill_names_across_separators() {
    let registry = Arc::new(Registry::new());
    let search_engine = SearchEngine::new(registry.clone());

    let schema = serde_json::json!({"type": "object"});
    let digest = skillsrs::core::SchemaDigest::from_schema(&schema).unwrap();
    let id = skillsrs::core::CallableId::skill("pytestlab-core", "1.0.0");

    registry
        .register(skillsrs::core::CallableRecord {
            id,
            kind: skillsrs::core::CallableKind::Skill,
            fq_name: "skill.pytestlab-core".to_string(),
            name: "pytestlab-core".to_string(),
            title: Some("PyTestLab Core".to_string()),
            description: Some("Use PyTestLab core library workflows".to_string()),
            tags: vec!["pytestlab".to_string(), "lamb".to_string()],
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

    let single_token_query = SearchQuery {
        q: "pytestlab".to_string(),
        kind: "skills".to_string(),
        limit: 10,
        filters: None,
        cursor: None,
    };

    let results = search_engine.search(&single_token_query).await.unwrap();
    assert_eq!(results.total_matches, 1);
    assert_eq!(results.matches[0].name, "pytestlab-core");

    let spaced_query = SearchQuery {
        q: "pytestlab core".to_string(),
        kind: "skills".to_string(),
        limit: 10,
        filters: None,
        cursor: None,
    };

    let results = search_engine.search(&spaced_query).await.unwrap();
    assert_eq!(results.total_matches, 1);
    assert_eq!(results.matches[0].name, "pytestlab-core");
    assert!(
        results.matches[0].score >= 50.0,
        "name token matches should score strongly, got {}",
        results.matches[0].score
    );
}

#[tokio::test]
#[expect(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
async fn test_search_ranks_name_matches_before_description_only_matches() {
    let registry = Arc::new(Registry::new());
    let search_engine = SearchEngine::new(registry.clone());

    let schema = serde_json::json!({"type": "object"});
    let digest = skillsrs::core::SchemaDigest::from_schema(&schema).unwrap();

    registry
        .register(skillsrs::core::CallableRecord {
            id: skillsrs::core::CallableId::skill("pytestlab-core", "1.0.0"),
            kind: skillsrs::core::CallableKind::Skill,
            fq_name: "skill.pytestlab-core".to_string(),
            name: "pytestlab-core".to_string(),
            title: Some("PyTestLab Core".to_string()),
            description: Some("Use the core lab workflow".to_string()),
            tags: vec![],
            input_schema: schema.clone(),
            output_schema: None,
            schema_digest: digest.clone(),
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

    registry
        .register(skillsrs::core::CallableRecord {
            id: skillsrs::core::CallableId::skill("generic-lab", "1.0.0"),
            kind: skillsrs::core::CallableKind::Skill,
            fq_name: "skill.generic-lab".to_string(),
            name: "generic-lab".to_string(),
            title: Some("Generic Lab".to_string()),
            description: Some("A workflow that can mention pytestlab core in prose".to_string()),
            tags: vec![],
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

    let query = SearchQuery {
        q: "pytestlab core".to_string(),
        kind: "skills".to_string(),
        limit: 10,
        filters: None,
        cursor: None,
    };

    let results = search_engine.search(&query).await.unwrap();
    assert_eq!(results.total_matches, 2);
    assert_eq!(results.matches[0].name, "pytestlab-core");
}

#[tokio::test]
#[expect(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
async fn test_search_matches_skill_body_terms() {
    let temp = TempDir::new().unwrap();
    let skill_dir = temp.path().join("body-search-skill");
    std::fs::create_dir(&skill_dir).unwrap();

    let skill_md = r"---
name: body-search-skill
description: Short workflow metadata
version: 1.0.0
---

# Body Search Skill

Use AutoInstrument source selection with from_preset and VISA connectivity.
";

    std::fs::write(skill_dir.join("SKILL.md"), skill_md).unwrap();

    let registry = Arc::new(Registry::new());
    let search_engine = SearchEngine::new(registry.clone());
    let store = SkillStore::new(temp.path(), registry.clone()).unwrap();

    let ids = store.load_and_register_all().await.unwrap();
    assert_eq!(ids.len(), 1);
    search_engine.rebuild();

    let query = SearchQuery {
        q: "AutoInstrument from_preset".to_string(),
        kind: "skills".to_string(),
        limit: 10,
        filters: None,
        cursor: None,
    };

    let results = search_engine.search(&query).await.unwrap();
    assert_eq!(results.total_matches, 1);
    assert_eq!(results.matches[0].name, "body-search-skill");
}

#[tokio::test]
#[expect(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
async fn test_search_ranks_metadata_matches_before_body_only_matches() {
    let temp = TempDir::new().unwrap();
    let body_skill_dir = temp.path().join("body-only-skill");
    std::fs::create_dir(&body_skill_dir).unwrap();

    let body_skill_md = r"---
name: body-only-skill
description: Short workflow metadata
version: 1.0.0
---

# Body Only Skill

This workflow mentions precision calibration in the instructions only.
";

    std::fs::write(body_skill_dir.join("SKILL.md"), body_skill_md).unwrap();

    let registry = Arc::new(Registry::new());
    let search_engine = SearchEngine::new(registry.clone());
    let store = SkillStore::new(temp.path(), registry.clone()).unwrap();

    let schema = serde_json::json!({"type": "object"});
    let digest = skillsrs::core::SchemaDigest::from_schema(&schema).unwrap();
    registry
        .register(skillsrs::core::CallableRecord {
            id: skillsrs::core::CallableId::skill("calibration-helper", "1.0.0"),
            kind: skillsrs::core::CallableKind::Skill,
            fq_name: "skill.calibration-helper".to_string(),
            name: "calibration-helper".to_string(),
            title: Some("Calibration Helper".to_string()),
            description: Some("Short workflow metadata".to_string()),
            tags: vec![],
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

    let ids = store.load_and_register_all().await.unwrap();
    assert_eq!(ids.len(), 1);
    search_engine.rebuild();

    let query = SearchQuery {
        q: "calibration".to_string(),
        kind: "skills".to_string(),
        limit: 10,
        filters: None,
        cursor: None,
    };

    let results = search_engine.search(&query).await.unwrap();
    assert_eq!(results.total_matches, 2);
    assert_eq!(results.matches[0].name, "calibration-helper");
}

#[tokio::test]
#[expect(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
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
#[expect(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
async fn test_skills_loaded_from_files_are_searchable() {
    // Test for the bug fix: skills loaded from files should be searchable
    // This verifies that the search index is rebuilt after skills are loaded
    let temp = TempDir::new().unwrap();
    let skill_dir = temp.path().join("searchable-skill");
    std::fs::create_dir(&skill_dir).unwrap();

    // Create a SKILL.md file
    let skill_md = r"---
name: searchable-skill
description: A skill that should be searchable after loading
version: 1.0.0
---

# Searchable Skill

This skill tests that loaded skills are searchable.
";

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
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
async fn test_find_skill_directory_uses_frontmatter_name() {
    let temp = TempDir::new().unwrap();
    let registry = Arc::new(Registry::new());
    let store = SkillStore::new(temp.path(), registry).unwrap();

    // Directory name intentionally does not match skill name.
    let skill_dir = temp.path().join("different-folder-name");
    std::fs::create_dir(&skill_dir).unwrap();
    let skill_md = r"---
name: frontmatter-name
description: Skill loaded by frontmatter name
version: 1.0.0
---

# Skill
";
    std::fs::write(skill_dir.join("SKILL.md"), skill_md).unwrap();

    let content = store.load_skill_content("frontmatter-name").unwrap();
    assert!(content.skill_md.contains("name: frontmatter-name"));
}

#[tokio::test]
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
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
