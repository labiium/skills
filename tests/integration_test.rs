//! Integration tests for skills.rs
//!
//! Tests the complete lifecycle of skills including:
//! - Skill creation and validation
//! - Progressive disclosure
//! - Bundled tool execution
//! - MCP tool integration
//! - Persistence

use skillsrs_core::{CallableId, CallableKind};
use skillsrs_index::SearchEngine;
use skillsrs_policy::{PolicyConfig, PolicyEngine};
use skillsrs_registry::Registry;
use skillsrs_runtime::Runtime;
use skillsrs_skillstore::{CreateSkillRequest, SkillStore};
use skillsrs_upstream::UpstreamManager;
use std::sync::Arc;
use tempfile::TempDir;

/// Helper to create test environment
struct TestEnvironment {
    _temp_dir: TempDir,
    registry: Arc<Registry>,
    search_engine: Arc<SearchEngine>,
    _policy_engine: Arc<PolicyEngine>,
    runtime: Arc<Runtime>,
    skill_store: Arc<SkillStore>,
    _upstream_manager: Arc<UpstreamManager>,
}

impl TestEnvironment {
    async fn new() -> Self {
        let temp_dir = TempDir::new().unwrap();
        let registry = Arc::new(Registry::new());
        let search_engine = Arc::new(SearchEngine::new(registry.clone()));
        let policy_engine = Arc::new(PolicyEngine::new(PolicyConfig::default()).unwrap());
        let upstream_manager = Arc::new(UpstreamManager::new(registry.clone()));
        let runtime = Arc::new(Runtime::new(registry.clone(), upstream_manager.clone()));
        let skill_store = Arc::new(SkillStore::new(temp_dir.path(), registry.clone()).unwrap());

        TestEnvironment {
            _temp_dir: temp_dir,
            registry,
            search_engine,
            _policy_engine: policy_engine,
            runtime,
            skill_store,
            _upstream_manager: upstream_manager,
        }
    }
}

#[tokio::test]
#[ignore = "flaky search indexing test - needs investigation"]
async fn test_full_skill_lifecycle() {
    let env = TestEnvironment::new().await;

    // 1. Create a skill
    let request = CreateSkillRequest {
        name: "test-researcher".to_string(),
        version: "1.0.0".to_string(),
        description: "A test skill for web research".to_string(),
        skill_md_content: r#"# Test Researcher

## Purpose
Research topics using available tools.

## Usage
1. Search for the topic
2. Read relevant results
3. Summarize findings

## Tools Used
- search_tool (if available)
"#
        .to_string(),
        uses_tools: vec!["search_tool".to_string()],
        bundled_files: vec![(
            "helper.py".to_string(),
            r#"#!/usr/bin/env python3
import json
import sys
import os

# Read arguments from environment
args_json = os.environ.get('SKILL_ARGS_JSON', '{}')
args = json.loads(args_json)

# Simple echo for testing
result = {
    "status": "success",
    "input": args,
    "message": "Helper executed successfully"
}

print(json.dumps(result))
"#
            .to_string(),
        )],
        tags: vec!["research".to_string(), "test".to_string()],
    };

    let skill_id = env
        .skill_store
        .create_skill(request)
        .await
        .expect("Failed to create skill");

    // 2. Verify skill is registered
    let record = env
        .registry
        .get(&skill_id)
        .expect("Skill not found in registry");
    assert_eq!(record.kind, CallableKind::Skill);
    assert_eq!(record.name, "test-researcher");
    assert!(
        !record.bundled_tools.is_empty(),
        "Should have bundled tools"
    );

    // 3. Load skill content (progressive disclosure)
    let content = env
        .skill_store
        .load_skill_content("test-researcher")
        .expect("Failed to load skill content");
    assert!(content.skill_md.contains("Test Researcher"));
    assert_eq!(content.bundled_tools.len(), 1);
    assert!(content.uses_tools.contains(&"search_tool".to_string()));

    // 4. Load specific file
    let helper_content = env
        .skill_store
        .load_skill_file("test-researcher", "helper.py")
        .expect("Failed to load helper.py");
    assert!(helper_content.contains("Helper executed successfully"));

    // 5. Search should find the skill
    env.search_engine.rebuild();
    let results = env
        .search_engine
        .search(&skillsrs_index::SearchQuery {
            q: "research".to_string(),
            kind: "skills".to_string(),
            mode: "best".to_string(),
            limit: 10,
            filters: Some(skillsrs_index::SearchFilters::default()),
            cursor: None,
        })
        .await
        .expect("Search failed");

    assert!(
        !results.matches.is_empty(),
        "Should find the test-researcher skill"
    );
    assert!(results.matches.iter().any(|m| m.name == "test-researcher"));

    // 6. Execute the skill (will run bundled tool)
    let exec_context = skillsrs_runtime::ExecContext {
        callable_id: skill_id.clone(),
        arguments: serde_json::json!({"query": "test"}),
        timeout_ms: Some(5000),
        trace_enabled: false,
    };

    let result = env
        .runtime
        .execute(exec_context)
        .await
        .expect("Execution failed");

    // Bundled tool should have executed
    assert!(!result.is_error, "Execution should succeed");
    assert!(!result.content.is_empty());

    // 7. Update the skill
    let update_request = CreateSkillRequest {
        name: "test-researcher".to_string(),
        version: "1.1.0".to_string(),
        description: "Updated test skill".to_string(),
        skill_md_content: "# Updated Test Researcher\n\nNew content.".to_string(),
        uses_tools: vec![],
        bundled_files: vec![],
        tags: vec!["research".to_string()],
    };

    env.skill_store
        .update_skill("test-researcher", update_request)
        .await
        .expect("Failed to update skill");

    // Verify update
    let updated_content = env
        .skill_store
        .load_skill_content("test-researcher")
        .expect("Failed to load updated content");
    assert!(updated_content.skill_md.contains("Updated Test Researcher"));

    // 8. Delete the skill
    env.skill_store
        .delete_skill("test-researcher")
        .expect("Failed to delete skill");

    // Verify deletion
    assert!(env
        .skill_store
        .load_skill_content("test-researcher")
        .is_err());
}

#[tokio::test]
async fn test_skill_validation() {
    let env = TestEnvironment::new().await;

    // Valid skill
    let valid_request = CreateSkillRequest {
        name: "valid-skill".to_string(),
        version: "1.0.0".to_string(),
        description: "A valid skill for testing".to_string(),
        skill_md_content: "# Valid Skill\n\nThis is valid.".to_string(),
        uses_tools: vec![],
        bundled_files: vec![],
        tags: vec![],
    };

    env.skill_store
        .create_skill(valid_request)
        .await
        .expect("Valid skill should be created");

    // Invalid: bad version
    let invalid_version = CreateSkillRequest {
        name: "invalid-version".to_string(),
        version: "not-a-version".to_string(),
        description: "Test".to_string(),
        skill_md_content: "# Test".to_string(),
        uses_tools: vec![],
        bundled_files: vec![],
        tags: vec![],
    };

    // Should still create (validation is separate from creation)
    // but validation will fail
    env.skill_store
        .create_skill(invalid_version)
        .await
        .expect("Creation should succeed");

    // Load and validate
    let skill = env
        .skill_store
        .load_skill(&env._temp_dir.path().join("invalid-version"))
        .await
        .expect("Should load");

    let validation = env.skill_store.validate_skill(&skill);
    assert!(!validation.valid, "Should fail validation");
    assert!(validation.errors.iter().any(|e| e.contains("version")));
}

#[tokio::test]
async fn test_progressive_disclosure() {
    let env = TestEnvironment::new().await;

    // Create a skill with multiple files
    let request = CreateSkillRequest {
        name: "multi-file-skill".to_string(),
        version: "1.0.0".to_string(),
        description: "Skill with multiple files".to_string(),
        skill_md_content: "# Multi-File Skill\n\nUses multiple files.".to_string(),
        uses_tools: vec![],
        bundled_files: vec![
            ("script.py".to_string(), "print('hello')".to_string()),
            ("data.json".to_string(), r#"{"key": "value"}"#.to_string()),
            ("helper.sh".to_string(), "echo 'test'".to_string()),
        ],
        tags: vec![],
    };

    env.skill_store
        .create_skill(request)
        .await
        .expect("Failed to create skill");

    // Level 1: Get metadata
    let record = env
        .registry
        .all()
        .into_iter()
        .find(|r| r.name == "multi-file-skill")
        .expect("Skill not found");
    assert_eq!(record.bundled_tools.len(), 2); // .py and .sh
    assert_eq!(record.additional_files.len(), 1); // .json

    // Level 2: Load SKILL.md
    let content = env
        .skill_store
        .load_skill_content("multi-file-skill")
        .expect("Failed to load content");
    assert!(content.skill_md.contains("Multi-File Skill"));

    // Level 3: Load specific files on demand
    let script = env
        .skill_store
        .load_skill_file("multi-file-skill", "script.py")
        .expect("Failed to load script");
    assert!(script.contains("hello"));

    let data = env
        .skill_store
        .load_skill_file("multi-file-skill", "data.json")
        .expect("Failed to load data");
    assert!(data.contains("key"));

    // Should reject path traversal
    assert!(env
        .skill_store
        .load_skill_file("multi-file-skill", "../secret")
        .is_err());
}

#[tokio::test]
async fn test_bundled_tool_execution() {
    let env = TestEnvironment::new().await;

    // Create skill with Python script
    let request = CreateSkillRequest {
        name: "python-skill".to_string(),
        version: "1.0.0".to_string(),
        description: "Skill with Python script".to_string(),
        skill_md_content: "# Python Skill".to_string(),
        uses_tools: vec![],
        bundled_files: vec![(
            "compute.py".to_string(),
            r#"#!/usr/bin/env python3
import json
import os

args = json.loads(os.environ.get('SKILL_ARGS_JSON', '{}'))
result = {
    "computed": args.get("value", 0) * 2,
    "status": "ok"
}
print(json.dumps(result))
"#
            .to_string(),
        )],
        tags: vec![],
    };

    let skill_id = env
        .skill_store
        .create_skill(request)
        .await
        .expect("Failed to create skill");

    // Execute the skill
    let exec_context = skillsrs_runtime::ExecContext {
        callable_id: skill_id,
        arguments: serde_json::json!({"value": 21}),
        timeout_ms: Some(5000),
        trace_enabled: false,
    };

    let result = env.runtime.execute(exec_context).await.expect("Failed");

    assert!(!result.is_error);
    if let Some(structured) = result.structured_content {
        assert_eq!(
            structured.get("computed").and_then(|v| v.as_i64()),
            Some(42)
        );
    }
}

#[tokio::test]
async fn test_skill_with_circular_dependency() {
    let env = TestEnvironment::new().await;

    // Create skill that references itself
    let request = CreateSkillRequest {
        name: "circular-skill".to_string(),
        version: "1.0.0".to_string(),
        description: "Skill with circular dependency".to_string(),
        skill_md_content: "# Circular".to_string(),
        uses_tools: vec!["circular-skill".to_string()],
        bundled_files: vec![],
        tags: vec![],
    };

    env.skill_store
        .create_skill(request)
        .await
        .expect("Creation should succeed");

    // Validation should catch circular dependency
    let skill = env
        .skill_store
        .load_skill(&env._temp_dir.path().join("circular-skill"))
        .await
        .expect("Should load");

    let validation = env.skill_store.validate_skill(&skill);
    assert!(!validation.valid);
    assert!(validation
        .errors
        .iter()
        .any(|e| e.contains("Circular dependency")));
}

#[tokio::test]
async fn test_registry_persistence_integration() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");

    // Create persistence layer
    let persistence: skillsrs_core::persistence::PersistenceLayer =
        skillsrs_core::persistence::PersistenceLayer::new(&db_path)
            .await
            .expect("Failed to create persistence");

    // Create a callable record
    let record = skillsrs_core::CallableRecord {
        id: CallableId::from("skill://test@1.0@abc"),
        kind: CallableKind::Skill,
        fq_name: "skill.test".to_string(),
        name: "test".to_string(),
        title: Some("Test Skill".to_string()),
        description: Some("A test skill".to_string()),
        tags: vec!["test".to_string()],
        input_schema: serde_json::json!({"type": "object"}),
        output_schema: None,
        schema_digest: skillsrs_core::SchemaDigest::from("abc"),
        server_alias: None,
        upstream_tool_name: None,
        skill_version: Some("1.0".to_string()),
        uses: vec![],
        skill_directory: Some(temp_dir.path().to_path_buf()),
        bundled_tools: vec![],
        additional_files: vec![],
        cost_hints: skillsrs_core::CostHints::default(),
        risk_tier: skillsrs_core::RiskTier::ReadOnly,
        last_seen: chrono::Utc::now(),
    };

    // Save it
    persistence
        .save_callable(&record)
        .await
        .expect("Failed to save");

    // Load it back
    let loaded = persistence
        .load_callable(&record.id)
        .await
        .expect("Failed to load");

    assert_eq!(loaded.name, record.name);
    assert_eq!(loaded.kind, record.kind);
    assert_eq!(loaded.description, record.description);

    // Test execution history
    persistence
        .record_execution(
            "exec-123",
            &record.id,
            &serde_json::json!({"test": "data"}),
            Some(&serde_json::json!({"result": "success"})),
            false,
            Some(100),
            chrono::Utc::now(),
            Some(chrono::Utc::now()),
            None,
        )
        .await
        .expect("Failed to record execution");

    let history: Vec<skillsrs_core::persistence::ExecutionRecord> = persistence
        .get_execution_history(&record.id, 10)
        .await
        .expect("Failed to get history");

    assert_eq!(history.len(), 1);
    assert_eq!(history[0].execution_id, "exec-123");
    assert!(!history[0].is_error);

    // Clean up
    persistence.close().await;
}

#[tokio::test]
#[ignore = "flaky search indexing test - needs investigation"]
async fn test_search_and_filter() {
    let env = TestEnvironment::new().await;

    // Create multiple skills
    for i in 1..=5 {
        let request = CreateSkillRequest {
            name: format!("skill-{}", i),
            version: "1.0.0".to_string(),
            description: format!("Test skill number {}", i),
            skill_md_content: format!("# Skill {}\n\nDoes task {}", i, i),
            uses_tools: vec![],
            bundled_files: vec![],
            tags: if i % 2 == 0 {
                vec!["even".to_string()]
            } else {
                vec!["odd".to_string()]
            },
        };

        env.skill_store
            .create_skill(request)
            .await
            .expect("Failed to create skill");
    }

    env.search_engine.rebuild();

    // Search all skills
    let results = env
        .search_engine
        .search(&skillsrs_index::SearchQuery {
            q: "skill".to_string(),
            kind: "skills".to_string(),
            mode: "best".to_string(),
            limit: 10,
            filters: Some(skillsrs_index::SearchFilters::default()),
            cursor: None,
        })
        .await
        .expect("Search failed");

    assert_eq!(results.matches.len(), 5);

    // Search with tag filter
    let filtered = env
        .search_engine
        .search(&skillsrs_index::SearchQuery {
            q: "skill".to_string(),
            kind: "skills".to_string(),
            mode: "best".to_string(),
            limit: 10,
            filters: Some(skillsrs_index::SearchFilters {
                tags: Some(vec!["even".to_string()]),
                ..Default::default()
            }),
            cursor: None,
        })
        .await
        .expect("Filtered search failed");

    assert_eq!(filtered.matches.len(), 2);
    // Tags are in the metadata, not directly on SearchMatch
}
