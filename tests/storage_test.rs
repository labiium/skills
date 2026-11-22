//! Tests for storage module: skill store, search, agent skills

use jiff::Timestamp;
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

    store.create_skill(request).unwrap();
    let skill = store
        .load_skill(&temp.path().join("valid-skill"))
        .await
        .unwrap();

    let result = store.validate_skill(&skill);
    assert!(result.valid, "Skill should be valid: {:?}", result.errors);
}

#[test]
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
fn test_create_skill() {
    let temp = TempDir::new().unwrap();
    let registry = Arc::new(Registry::new());
    let store = SkillStore::new(temp.path(), registry).unwrap();

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

    let id = store.create_skill(request).unwrap();
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

#[test]
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
fn test_load_skill_content() {
    let temp = TempDir::new().unwrap();
    let registry = Arc::new(Registry::new());
    let store = SkillStore::new(temp.path(), registry).unwrap();

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

    store.create_skill(request).unwrap();

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

#[test]
#[expect(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
fn test_search_engine() {
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
        last_seen: Timestamp::now(),
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

    let results = search_engine.search(&query).unwrap();
    assert_eq!(results.total_matches, 1);
    assert_eq!(results.matches[0].name, "test-tool");
}

#[test]
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
fn test_exhaustive_search_enumerates_every_record_once_in_stable_pages() {
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
                last_seen: Timestamp::now(),
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
        .unwrap();
    let second = search_engine
        .search(&SearchQuery {
            q: "*".to_string(),
            kind: "tools".to_string(),
            limit: 50,
            filters: None,
            cursor: first.next_cursor.clone(),
        })
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

#[test]
#[expect(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
fn test_search_matches_skill_names_across_separators() {
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
            last_seen: Timestamp::now(),
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

    let results = search_engine.search(&single_token_query).unwrap();
    assert_eq!(results.total_matches, 1);
    assert_eq!(results.matches[0].name, "pytestlab-core");

    let spaced_query = SearchQuery {
        q: "pytestlab core".to_string(),
        kind: "skills".to_string(),
        limit: 10,
        filters: None,
        cursor: None,
    };

    let results = search_engine.search(&spaced_query).unwrap();
    assert_eq!(results.total_matches, 1);
    assert_eq!(results.matches[0].name, "pytestlab-core");
    assert!(
        results.matches[0].score >= 50.0,
        "name token matches should score strongly, got {}",
        results.matches[0].score
    );
}

#[test]
#[expect(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
fn test_search_ranks_name_matches_before_description_only_matches() {
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
            last_seen: Timestamp::now(),
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
            last_seen: Timestamp::now(),
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

    let results = search_engine.search(&query).unwrap();
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

    let results = search_engine.search(&query).unwrap();
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
            last_seen: Timestamp::now(),
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

    let results = search_engine.search(&query).unwrap();
    assert_eq!(results.total_matches, 2);
    assert_eq!(results.matches[0].name, "calibration-helper");
}

#[test]
#[expect(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
fn test_search_with_filters() {
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
            last_seen: Timestamp::now(),
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
            last_seen: Timestamp::now(),
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

    let results = search_engine.search(&query).unwrap();
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

    let results = search_engine.search(&query).unwrap();
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

    let results = search_engine.search(&query).unwrap();
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

    let results = search_engine.search(&query).unwrap();
    assert_eq!(results.total_matches, 1, "Should find skill by description");
    assert_eq!(results.matches[0].name, "searchable-skill");
}

#[test]
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
fn test_find_skill_directory_uses_frontmatter_name() {
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

    store.create_skill(request).unwrap();
    let skill_dir = temp.path().join("yaml-escaped-skill");
    let skill = store.load_skill(&skill_dir).await.unwrap();

    assert_eq!(
        skill.manifest.description,
        "Handles: colon values\nand newlines safely"
    );
}

#[test]
#[expect(clippy::unwrap_used, reason = "regression assertions")]
fn metadata_survives_creation_and_instruction_edits() {
    use skillsrs::storage::UpdateSkillRequest;
    let temp = TempDir::new().unwrap();
    let store = SkillStore::new(temp.path(), Arc::new(Registry::new())).unwrap();
    let document = "---\nname: retained\ndescription: Original\nversion: 2.3.4\nlicense: MIT\ncompatibility: Requires a shell\nmetadata:\n  author: Ada\nallowed-tools: Read Bash\ntags: [original]\ndisable-model-invocation: true\nuser-invocable: false\nhost-options:\n  nested: [one, 2, false]\n---\n\nOriginal body";
    store
        .create_skill(CreateSkillRequest {
            name: "retained".to_string(),
            version: "2.3.4".to_string(),
            description: "Original".to_string(),
            skill_md_content: document.to_string(),
            uses_tools: vec![],
            tags: vec![],
            scripts: vec![],
            references: vec![],
            assets: vec![],
        })
        .unwrap();
    let before = store.load_skill_content("retained").unwrap();
    store
        .update_skill(
            "retained",
            UpdateSkillRequest {
                skill_md_content: Some("Edited instructions".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
    let after = store.load_skill_content("retained").unwrap();
    assert_eq!(before.frontmatter, after.frontmatter);
    assert!(after.skill_md.ends_with("Edited instructions"));
    assert_eq!(after.name, "retained");
    assert_eq!(after.version, "2.3.4");
    assert_eq!(after.uses_tools, ["Read", "Bash"]);
    assert_eq!(
        after
            .frontmatter
            .get("license")
            .and_then(serde_yaml::Value::as_str),
        Some("MIT")
    );
    assert_eq!(
        after
            .frontmatter
            .get("disable-model-invocation")
            .and_then(serde_yaml::Value::as_bool),
        Some(true)
    );
    store
        .update_skill(
            "retained",
            UpdateSkillRequest {
                description: Some("Updated".to_string()),
                uses_tools: Some(vec![]),
                tags: Some(vec![]),
                ..Default::default()
            },
        )
        .unwrap();
    let cleared = store.load_skill_content("retained").unwrap();
    assert_eq!(cleared.description, "Updated");
    assert!(cleared.uses_tools.is_empty());
    assert!(cleared
        .frontmatter
        .get("tags")
        .and_then(serde_yaml::Value::as_sequence)
        .is_some_and(Vec::is_empty));
    assert_eq!(
        cleared.frontmatter.get("host-options"),
        Some(before.frontmatter.get("host-options").unwrap())
    );
}

#[test]
fn frontmatter_requires_descriptions_and_counts_unicode_characters() {
    use skillsrs::storage::agent_skills::parse_frontmatter_public;
    for fields in [
        "name: valid",
        "description: text",
        "name: valid\ndescription: ''",
        "name: valid\ndescription: text\ncompatibility: ''",
        "name: bad--name\ndescription: text",
        "name: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\ndescription: text",
    ] {
        assert!(parse_frontmatter_public(&format!("---\n{fields}\n---\n\nBody")).is_err());
    }
    let description = "界".repeat(1024);
    let compatibility = "é".repeat(500);
    let valid = format!(
        "---\nname: valid\ndescription: {description}\ncompatibility: {compatibility}\n---\n\nBody"
    );
    assert!(parse_frontmatter_public(&valid).is_ok());
    let invalid = valid.replace(&description, &format!("{description}界"));
    assert!(parse_frontmatter_public(&invalid).is_err());
    let invalid = valid.replace(&compatibility, &format!("{compatibility}é"));
    assert!(parse_frontmatter_public(&invalid).is_err());
}

#[tokio::test]
#[expect(clippy::unwrap_used, reason = "regression assertions")]
async fn recursive_inventory_includes_resources_and_never_promotes_scripts_to_entrypoints() {
    let temp = TempDir::new().unwrap();
    let registry = Arc::new(Registry::new());
    let store = SkillStore::new(temp.path(), registry.clone()).unwrap();
    let root = temp.path().join("inventory");
    for directory in [
        "scripts/nested",
        "custom/deep",
        "references/nested",
        ".git",
        "node_modules/pkg",
    ] {
        std::fs::create_dir_all(root.join(directory)).unwrap();
    }
    std::fs::write(
        root.join("SKILL.md"),
        "---\nname: inventory\ndescription: Inventory resources\n---\n\nInstructions",
    )
    .unwrap();
    for file in [
        "scripts/nested/run.py",
        "custom/deep/data.bin",
        "references/nested/guide.md",
        "package.json",
        ".git/private",
        "node_modules/pkg/index.js",
    ] {
        std::fs::write(root.join(file), [0, 255, 42]).unwrap();
    }
    let content = store.load_skill_content("inventory").unwrap();
    assert_eq!(
        content.additional_files,
        [
            "custom/deep/data.bin",
            "package.json",
            "references/nested/guide.md"
        ]
    );
    assert_eq!(content.bundled_tools.len(), 1);
    let bundled = content.bundled_tools.first().unwrap();
    assert_eq!(bundled.name, "scripts/nested/run.py");
    assert_eq!(
        bundled.command.get(1).unwrap(),
        root.join("scripts/nested/run.py")
            .to_string_lossy()
            .as_ref()
    );
    assert_eq!(
        store
            .load_skill_file_bytes("inventory", "custom/deep/data.bin")
            .unwrap(),
        [0, 255, 42]
    );
    assert!(store
        .load_skill_file("inventory", "custom/deep/data.bin")
        .is_err());
    let skill = store.load_skill(&root).await.unwrap();
    assert!(matches!(
        skill.manifest.entrypoint,
        skillsrs::storage::EntrypointType::Prompted
    ));
    store.register_skill(&skill).unwrap();
    let record = registry.by_kind(CallableKind::Skill).remove(0);
    assert_eq!(record.additional_files, content.additional_files);
    assert!(store
        .load_skill_file_bytes("../inventory", "SKILL.md")
        .is_err());
    assert!(store
        .load_skill_file_bytes("inventory", "../SKILL.md")
        .is_err());
}

#[tokio::test]
#[expect(clippy::unwrap_used, reason = "regression assertions")]
async fn overlays_are_ordered_read_only_and_support_root_packages() {
    use skillsrs::storage::UpdateSkillRequest;
    let managed = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    let user = TempDir::new().unwrap();
    let mut store = SkillStore::new(managed.path(), Arc::new(Registry::new())).unwrap();
    for (root, marker) in [(project.path(), "project"), (user.path(), "user")] {
        std::fs::write(
            root.join("SKILL.md"),
            format!("---\nname: overlay\ndescription: {marker}\n---\n\n{marker}"),
        )
        .unwrap();
    }
    store.add_read_root(project.path().to_path_buf());
    store.add_read_root(user.path().to_path_buf());
    store.add_read_root(project.path().to_path_buf());
    assert_eq!(store.load_all().await.unwrap().len(), 1);
    assert_eq!(
        store.load_skill_content("overlay").unwrap().description,
        "project"
    );
    assert!(store
        .update_skill(
            "overlay",
            UpdateSkillRequest {
                skill_md_content: Some("forbidden".to_string()),
                ..Default::default()
            }
        )
        .is_err());
    assert!(store.delete_skill("overlay").is_err());
    let primary = managed.path().join("overlay");
    std::fs::create_dir(&primary).unwrap();
    std::fs::write(
        primary.join("SKILL.md"),
        "---\nname: overlay\ndescription: primary\n---\n\nPrimary",
    )
    .unwrap();
    assert_eq!(
        store.load_skill_content("overlay").unwrap().description,
        "primary"
    );
    assert_eq!(store.load_all().await.unwrap().len(), 1);
    store.delete_skill("overlay").unwrap();
    assert_eq!(
        store.load_skill_content("overlay").unwrap().description,
        "project"
    );
}

#[tokio::test]
#[expect(clippy::unwrap_used, reason = "regression assertions")]
async fn validation_reports_malformed_missing_and_mismatched_packages() {
    let temp = TempDir::new().unwrap();
    let store = SkillStore::new(temp.path(), Arc::new(Registry::new())).unwrap();
    for name in ["malformed", "missing", "mismatch", "bad-version"] {
        std::fs::create_dir(temp.path().join(name)).unwrap();
    }
    std::fs::write(
        temp.path().join("malformed/SKILL.md"),
        "---\ninvalid: [\n---\n\nBody",
    )
    .unwrap();
    std::fs::write(
        temp.path().join("mismatch/SKILL.md"),
        "---\nname: different\ndescription: Text\n---\n\nBody",
    )
    .unwrap();
    std::fs::write(
        temp.path().join("bad-version/SKILL.md"),
        "---\nname: bad-version\ndescription: Text\nversion: not-semver\n---\n\nBody",
    )
    .unwrap();
    let validation = store.validate_all().await;
    assert!(!validation.valid);
    for name in ["malformed", "missing", "mismatch", "bad-version"] {
        assert!(
            validation.errors.iter().any(|error| error.contains(name)),
            "{:?}",
            validation.errors
        );
    }
}

#[cfg(unix)]
#[tokio::test]
#[expect(clippy::unwrap_used, reason = "regression assertions")]
async fn symlinks_cannot_escape_resources_or_mutations_and_roots_are_deduplicated() {
    use skillsrs::storage::UpdateSkillRequest;
    use std::os::unix::fs::symlink;
    let temp = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    let mut store = SkillStore::new(temp.path(), Arc::new(Registry::new())).unwrap();
    let root = temp.path().join("safe");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(
        root.join("SKILL.md"),
        "---\nname: safe\ndescription: Safe\n---\n\nInstructions",
    )
    .unwrap();
    std::fs::write(outside.path().join("secret"), "private").unwrap();
    symlink(outside.path(), root.join("escape")).unwrap();
    symlink(&root, root.join("loop")).unwrap();
    symlink(outside.path(), root.join("scripts")).unwrap();
    symlink(temp.path(), outside.path().join("alias")).unwrap();
    store.add_read_root(outside.path().join("alias"));
    assert_eq!(store.load_all().await.unwrap().len(), 1);
    assert!(store
        .load_skill_content("safe")
        .unwrap()
        .additional_files
        .is_empty());
    assert!(store.load_skill_file("safe", "escape/secret").is_err());
    assert!(store
        .update_skill(
            "safe",
            UpdateSkillRequest {
                scripts: vec![("secret".to_string(), "overwrite".to_string())],
                ..Default::default()
            }
        )
        .is_err());
    assert_eq!(
        std::fs::read_to_string(outside.path().join("secret")).unwrap(),
        "private"
    );
}

#[tokio::test]
#[expect(clippy::unwrap_used, reason = "isolated discovery regression")]
async fn visibility_precedes_ranked_pagination_and_cursors_are_checked() {
    let temp = TempDir::new().unwrap();
    let registry = Arc::new(Registry::new());
    let store = SkillStore::new(temp.path(), registry.clone()).unwrap();
    for name in ["a-hidden", "b-visible", "c-hidden", "d-visible"] {
        let directory = temp.path().join(name);
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(
            directory.join("SKILL.md"),
            format!(
                "---\nname: {name}\ndescription: Shared discovery fixture\n---\n\nInstructions"
            ),
        )
        .unwrap();
    }
    store.load_and_register_all().await.unwrap();
    let engine = SearchEngine::new(registry);
    let mut query = SearchQuery {
        q: "shared".into(),
        kind: "skills".into(),
        limit: 1,
        filters: None,
        cursor: None,
    };
    let first = engine
        .search_visible(&query, |record| record.name.ends_with("visible"))
        .unwrap();
    assert_eq!(
        first.matches.first().map(|record| record.name.as_str()),
        Some("b-visible")
    );
    assert_eq!(first.total_matches, 2);
    query.cursor = first.next_cursor;
    let second = engine
        .search_visible(&query, |record| record.name.ends_with("visible"))
        .unwrap();
    assert_eq!(
        second.matches.first().map(|record| record.name.as_str()),
        Some("d-visible")
    );
    assert!(second.next_cursor.is_none());
    query.cursor = Some("not-a-cursor".into());
    assert!(engine.search(&query).is_err());
    query.cursor = None;
    query.limit = 0;
    assert!(engine.search(&query).is_err());
}

#[expect(clippy::unwrap_used, reason = "regression fixture")]
fn write_overlay_package(directory: &std::path::Path, name: &str, version: &str, marker: &str) {
    std::fs::create_dir_all(directory.join("scripts")).unwrap();
    std::fs::write(
        directory.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {marker}\nversion: {version}\n---\n\n{marker}"),
    )
    .unwrap();
    std::fs::write(directory.join("scripts/marker.sh"), marker).unwrap();
}

#[expect(clippy::unwrap_used, reason = "regression assertions")]
fn assert_overlay_winner(
    store: &SkillStore,
    registry: &Registry,
    search: &SearchEngine,
    name: &str,
    directory: &std::path::Path,
    version: &str,
    marker: &str,
) {
    let record = registry.get_by_fq_name(&format!("skill.{name}")).unwrap();
    assert_eq!(
        record.skill_directory,
        Some(directory.canonicalize().unwrap())
    );
    assert_eq!(record.skill_version.as_deref(), Some(version));
    assert_eq!(record.id, skillsrs::core::CallableId::skill(name, version));
    assert_eq!(
        registry
            .by_kind(CallableKind::Skill)
            .iter()
            .filter(|record| record.name == name)
            .count(),
        1
    );
    let content = store.load_skill_content(name).unwrap();
    assert_eq!(content.directory, record.skill_directory.unwrap());
    assert_eq!(content.version, version);
    assert_eq!(content.description, marker);
    assert!(content.skill_md.ends_with(marker));
    assert_eq!(
        store.load_skill_file(name, "scripts/marker.sh").unwrap(),
        marker
    );
    let tool = record
        .bundled_tools
        .iter()
        .find(|tool| tool.name == "scripts/marker.sh")
        .unwrap();
    let script = directory.canonicalize().unwrap().join("scripts/marker.sh");
    assert_eq!(tool.command.last().map(String::as_str), script.to_str());
    let results = search
        .search(&SearchQuery {
            q: name.to_string(),
            kind: "skills".to_string(),
            limit: 50,
            filters: None,
            cursor: None,
        })
        .unwrap();
    let mut matches = results.matches.iter().filter(|result| result.name == name);
    assert_eq!(matches.next().unwrap().id, record.id.as_str());
    assert!(matches.next().is_none());
}

#[tokio::test]
#[expect(clippy::unwrap_used, reason = "overlay deletion regression")]
async fn managed_override_delete_restores_winning_overlay() {
    let managed = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    let user = TempDir::new().unwrap();
    let primary = managed.path().join("overlay");
    write_overlay_package(&primary, "overlay", "3.0.0", "Managed body");
    write_overlay_package(project.path(), "overlay", "2.0.0", "Project body");
    write_overlay_package(user.path(), "overlay", "1.0.0", "User body");
    let registry = Arc::new(Registry::new());
    let search = Arc::new(SearchEngine::new(registry.clone()));
    let mut store =
        SkillStore::with_search_engine(managed.path(), registry.clone(), search.clone()).unwrap();
    store.add_read_root(project.path().to_path_buf());
    store.add_read_root(user.path().to_path_buf());
    store.load_and_register_all().await.unwrap();
    assert_overlay_winner(
        &store,
        &registry,
        &search,
        "overlay",
        &primary,
        "3.0.0",
        "Managed body",
    );

    store.delete_skill("overlay").unwrap();
    assert!(registry
        .get(&skillsrs::core::CallableId::skill("overlay", "3.0.0"))
        .is_none());
    assert_overlay_winner(
        &store,
        &registry,
        &search,
        "overlay",
        project.path(),
        "2.0.0",
        "Project body",
    );
    assert!(store.delete_skill("overlay").is_err());

    store
        .create_skill(CreateSkillRequest {
            name: "overlay".to_string(),
            version: "4.0.0".to_string(),
            description: "Recreated managed body".to_string(),
            skill_md_content: "Recreated managed body".to_string(),
            uses_tools: vec![],
            tags: vec![],
            scripts: vec![(
                "marker.sh".to_string(),
                "Recreated managed body".to_string(),
            )],
            references: vec![],
            assets: vec![],
        })
        .unwrap();
    assert_overlay_winner(
        &store,
        &registry,
        &search,
        "overlay",
        &primary,
        "4.0.0",
        "Recreated managed body",
    );
    assert!(registry
        .get(&skillsrs::core::CallableId::skill("overlay", "2.0.0"))
        .is_none());
}

#[tokio::test]
#[expect(clippy::unwrap_used, reason = "incremental overlay refresh regression")]
async fn directory_refresh_reconciles_removal_recreation_names_versions_and_renames() {
    let managed = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    let primary = managed.path().join("directory-name");
    let unrelated = managed.path().join("unrelated");
    write_overlay_package(&primary, "overlay", "3.0.0", "Managed body");
    write_overlay_package(project.path(), "overlay", "2.0.0", "Project body");
    write_overlay_package(&unrelated, "unrelated", "1.0.0", "Unrelated body");
    let registry = Arc::new(Registry::new());
    let search = Arc::new(SearchEngine::new(registry.clone()));
    let mut store =
        SkillStore::with_search_engine(managed.path(), registry.clone(), search.clone()).unwrap();
    store.add_read_root(project.path().to_path_buf());
    store.load_and_register_all().await.unwrap();
    let untouched = registry.get_by_fq_name("skill.unrelated").unwrap();

    std::fs::remove_dir_all(&primary).unwrap();
    store.refresh_directory(&primary).unwrap();
    assert_overlay_winner(
        &store,
        &registry,
        &search,
        "overlay",
        project.path(),
        "2.0.0",
        "Project body",
    );
    assert!(registry
        .get(&skillsrs::core::CallableId::skill("overlay", "3.0.0"))
        .is_none());

    write_overlay_package(&primary, "overlay", "4.0.0", "Recreated body");
    store.refresh_directory(&primary).unwrap();
    assert_overlay_winner(
        &store,
        &registry,
        &search,
        "overlay",
        &primary,
        "4.0.0",
        "Recreated body",
    );
    write_overlay_package(&primary, "overlay", "5.0.0", "Version edited body");
    store.refresh_directory(&primary).unwrap();
    assert_overlay_winner(
        &store,
        &registry,
        &search,
        "overlay",
        &primary,
        "5.0.0",
        "Version edited body",
    );
    assert!(registry
        .get(&skillsrs::core::CallableId::skill("overlay", "4.0.0"))
        .is_none());

    write_overlay_package(&primary, "renamed", "6.0.0", "Name edited body");
    store.refresh_directory(&primary).unwrap();
    assert_overlay_winner(
        &store,
        &registry,
        &search,
        "overlay",
        project.path(),
        "2.0.0",
        "Project body",
    );
    assert_overlay_winner(
        &store,
        &registry,
        &search,
        "renamed",
        &primary,
        "6.0.0",
        "Name edited body",
    );
    assert!(registry
        .get(&skillsrs::core::CallableId::skill("overlay", "5.0.0"))
        .is_none());

    let moved = managed.path().join("moved-directory");
    std::fs::rename(&primary, &moved).unwrap();
    store.refresh_directory(&primary).unwrap();
    store.refresh_directory(&moved).unwrap();
    assert_overlay_winner(
        &store,
        &registry,
        &search,
        "renamed",
        &moved,
        "6.0.0",
        "Name edited body",
    );
    assert_eq!(
        registry
            .get_by_fq_name("skill.unrelated")
            .unwrap()
            .last_seen,
        untouched.last_seen
    );
    assert!(store
        .refresh_directory(project.path().parent().unwrap())
        .is_err());
}

#[tokio::test]
#[expect(clippy::unwrap_used, reason = "managed rename overlay regression")]
async fn managed_override_rename_restores_previous_name_from_overlay() {
    use skillsrs::storage::UpdateSkillRequest;
    let managed = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    let primary = managed.path().join("overlay");
    write_overlay_package(&primary, "overlay", "3.0.0", "Managed body");
    write_overlay_package(project.path(), "overlay", "2.0.0", "Project body");
    let registry = Arc::new(Registry::new());
    let search = Arc::new(SearchEngine::new(registry.clone()));
    let mut store =
        SkillStore::with_search_engine(managed.path(), registry.clone(), search.clone()).unwrap();
    store.add_read_root(project.path().to_path_buf());
    store.load_and_register_all().await.unwrap();
    let id = store
        .update_skill(
            "overlay",
            UpdateSkillRequest {
                name: Some("renamed".to_string()),
                version: Some("4.0.0".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(id, skillsrs::core::CallableId::skill("renamed", "4.0.0"));
    assert_overlay_winner(
        &store,
        &registry,
        &search,
        "overlay",
        project.path(),
        "2.0.0",
        "Project body",
    );
    assert_overlay_winner(
        &store,
        &registry,
        &search,
        "renamed",
        &managed.path().join("renamed"),
        "4.0.0",
        "Managed body",
    );
    assert!(registry
        .get(&skillsrs::core::CallableId::skill("overlay", "3.0.0"))
        .is_none());
    assert!(store
        .update_skill("overlay", UpdateSkillRequest::default())
        .is_err());
}

#[tokio::test]
#[expect(
    clippy::unwrap_used,
    reason = "root-package overlay refresh regression"
)]
async fn root_package_refresh_restores_lower_overlay_after_frontmatter_rename() {
    let managed = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    let user = TempDir::new().unwrap();
    write_overlay_package(project.path(), "overlay", "2.0.0", "Project body");
    write_overlay_package(user.path(), "overlay", "1.0.0", "User body");
    let registry = Arc::new(Registry::new());
    let search = Arc::new(SearchEngine::new(registry.clone()));
    let mut store =
        SkillStore::with_search_engine(managed.path(), registry.clone(), search.clone()).unwrap();
    store.add_read_root(project.path().to_path_buf());
    store.add_read_root(user.path().to_path_buf());
    store.load_and_register_all().await.unwrap();

    write_overlay_package(project.path(), "renamed", "3.0.0", "Renamed project body");
    store.refresh_directory(project.path()).unwrap();
    assert_overlay_winner(
        &store,
        &registry,
        &search,
        "overlay",
        user.path(),
        "1.0.0",
        "User body",
    );
    assert_overlay_winner(
        &store,
        &registry,
        &search,
        "renamed",
        project.path(),
        "3.0.0",
        "Renamed project body",
    );
    assert!(registry
        .get(&skillsrs::core::CallableId::skill("overlay", "2.0.0"))
        .is_none());

    std::fs::remove_file(project.path().join("SKILL.md")).unwrap();
    store.refresh_directory(project.path()).unwrap();
    assert!(registry.get_by_fq_name("skill.renamed").is_none());
    assert!(registry
        .get(&skillsrs::core::CallableId::skill("renamed", "3.0.0"))
        .is_none());
    write_overlay_package(project.path(), "overlay", "4.0.0", "Recreated project body");
    store.refresh_directory(project.path()).unwrap();
    assert_overlay_winner(
        &store,
        &registry,
        &search,
        "overlay",
        project.path(),
        "4.0.0",
        "Recreated project body",
    );
}
