//! Integration tests for Agent Skills support (Vercel skills.sh compatible)

use skillsrs_registry::Registry;
use skillsrs_skillstore::{agent_skills::AgentSkill, sync::*, SkillStore};
use std::sync::Arc;
use tempfile::TempDir;

/// Helper to create a minimal Agent Skill directory
fn create_test_agent_skill(dir: &std::path::Path, name: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;

    let skill_md = format!(
        r#"---
name: {}
description: Test skill for integration testing
license: MIT
metadata:
  author: test-suite
  version: "1.0.0"
allowed-tools: Bash Read Write
---

# {}

This is a test Agent Skill.

## Purpose
Used for integration testing of Agent Skills support.

## Usage
This skill demonstrates the Agent Skills format.
"#,
        name,
        name.replace('-', " ")
            .split(' ')
            .map(|w| {
                let mut chars = w.chars();
                match chars.next() {
                    None => String::new(),
                    Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    );

    std::fs::write(dir.join("SKILL.md"), skill_md)?;

    // Create optional directories
    std::fs::create_dir_all(dir.join("scripts"))?;
    std::fs::write(
        dir.join("scripts").join("helper.py"),
        "#!/usr/bin/env python3\nprint('Hello from Agent Skill')\n",
    )?;

    std::fs::create_dir_all(dir.join("references"))?;
    std::fs::write(
        dir.join("references").join("docs.md"),
        "# Reference Documentation\n\nThis is a reference doc.\n",
    )?;

    Ok(())
}

#[tokio::test]
async fn test_agent_skill_parsing() {
    let temp = TempDir::new().unwrap();
    let skill_dir = temp.path().join("test-skill");

    create_test_agent_skill(&skill_dir, "test-skill").unwrap();

    let agent_skill = AgentSkill::from_directory(&skill_dir).await.unwrap();

    assert_eq!(agent_skill.frontmatter.name, "test-skill");
    assert_eq!(
        agent_skill.frontmatter.description,
        "Test skill for integration testing"
    );
    assert_eq!(agent_skill.frontmatter.license, Some("MIT".to_string()));
    assert_eq!(
        agent_skill.frontmatter.metadata.get("author"),
        Some(&"test-suite".to_string())
    );
    assert_eq!(
        agent_skill.frontmatter.allowed_tools,
        Some("Bash Read Write".to_string())
    );

    assert!(agent_skill.content.contains("# Test Skill"));
    assert!(agent_skill.has_scripts());
    assert!(agent_skill.has_references());
    assert!(!agent_skill.has_assets());
}

#[tokio::test]
async fn test_agent_skill_name_validation() {
    let temp = TempDir::new().unwrap();

    // Valid names
    let valid_names = vec![
        "valid-name",
        "test-123",
        "a",
        "long-skill-name-with-hyphens",
    ];
    for name in valid_names {
        let skill_dir = temp.path().join(name);
        create_test_agent_skill(&skill_dir, name).unwrap();
        assert!(
            AgentSkill::from_directory(&skill_dir).await.is_ok(),
            "Valid name '{}' should be accepted",
            name
        );
    }

    // Invalid names
    let invalid_names = vec![
        ("Invalid-Name", "uppercase"),
        ("-invalid", "starts with hyphen"),
        ("invalid-", "ends with hyphen"),
        ("invalid--name", "consecutive hyphens"),
    ];

    for (name, reason) in invalid_names {
        let skill_dir = temp.path().join(name);
        std::fs::create_dir_all(&skill_dir).unwrap();

        // Create SKILL.md with invalid name
        let skill_md = format!(
            r#"---
name: {}
description: Test
---
Content
"#,
            name
        );
        std::fs::write(skill_dir.join("SKILL.md"), skill_md).unwrap();

        assert!(
            AgentSkill::from_directory(&skill_dir).await.is_err(),
            "Invalid name '{}' ({}) should be rejected",
            name,
            reason
        );
    }
}

#[tokio::test]
async fn test_agent_skill_name_directory_mismatch() {
    let temp = TempDir::new().unwrap();
    let skill_dir = temp.path().join("directory-name");
    std::fs::create_dir_all(&skill_dir).unwrap();

    // Name in frontmatter doesn't match directory name
    let skill_md = r#"---
name: different-name
description: Test
---
Content
"#;
    std::fs::write(skill_dir.join("SKILL.md"), skill_md).unwrap();

    let result = AgentSkill::from_directory(&skill_dir).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("does not match"));
}

#[tokio::test]
async fn test_agent_skill_to_skill_manifest() {
    let temp = TempDir::new().unwrap();
    let skill_dir = temp.path().join("test-conversion");
    create_test_agent_skill(&skill_dir, "test-conversion").unwrap();

    let agent_skill = AgentSkill::from_directory(&skill_dir).await.unwrap();
    let manifest = agent_skill.to_skill_manifest();

    assert_eq!(manifest.id, "test-conversion");
    assert_eq!(manifest.title, "Test Conversion");
    assert_eq!(manifest.version, "1.0.0");
    assert_eq!(manifest.description, "Test skill for integration testing");

    // Check tool policy conversion
    let allowed_tools = manifest.tool_policy.allow;
    assert!(allowed_tools.contains(&"Bash".to_string()));
    assert!(allowed_tools.contains(&"Read".to_string()));
    assert!(allowed_tools.contains(&"Write".to_string()));
}

#[tokio::test]
async fn test_agent_skill_progressive_disclosure() {
    let temp = TempDir::new().unwrap();
    let skill_dir = temp.path().join("disclosure-test");
    create_test_agent_skill(&skill_dir, "disclosure-test").unwrap();

    let agent_skill = AgentSkill::from_directory(&skill_dir).await.unwrap();
    let content = agent_skill.to_skill_content().await;

    assert!(content.skill_md.contains("# Disclosure Test"));
    assert_eq!(content.bundled_tools.len(), 1); // helper.py
    assert_eq!(content.bundled_tools[0].name, "helper.py");
    assert!(content.uses_tools.contains(&"Bash".to_string()));
}

#[tokio::test]
async fn test_skill_store_loads_agent_skills() {
    let temp = TempDir::new().unwrap();
    let skills_root = temp.path().join("skills");
    std::fs::create_dir_all(&skills_root).unwrap();

    // Create multiple Agent Skills
    create_test_agent_skill(&skills_root.join("skill-one"), "skill-one").unwrap();
    create_test_agent_skill(&skills_root.join("skill-two"), "skill-two").unwrap();

    let registry = Arc::new(Registry::new());
    let store = SkillStore::new(&skills_root, registry.clone()).unwrap();

    let skills = store.load_all().await.unwrap();
    assert_eq!(skills.len(), 2);

    let skill_names: Vec<_> = skills.iter().map(|s| s.manifest.id.as_str()).collect();
    assert!(skill_names.contains(&"skill-one"));
    assert!(skill_names.contains(&"skill-two"));
}

#[tokio::test]
async fn test_skill_store_mixed_formats() {
    let temp = TempDir::new().unwrap();
    let skills_root = temp.path().join("skills");
    std::fs::create_dir_all(&skills_root).unwrap();

    // Create Agent Skill (SKILL.md only, no skill.json)
    create_test_agent_skill(&skills_root.join("agent-skill"), "agent-skill").unwrap();

    // Create traditional skills.rs skill (skill.json + SKILL.md)
    let traditional_dir = skills_root.join("traditional-skill");
    std::fs::create_dir_all(&traditional_dir).unwrap();

    let skill_json = r#"{
  "id": "traditional-skill",
  "title": "Traditional Skill",
  "version": "1.0.0",
  "description": "Traditional skills.rs format",
  "inputs": {
    "type": "object",
    "properties": {}
  },
  "entrypoint": "prompted",
  "tool_policy": {
    "allow": ["*"],
    "deny": [],
    "required": []
  },
  "hints": {
    "intent": [],
    "domain": [],
    "outcomes": [],
    "expected_calls": null
  },
  "risk_tier": "read_only"
}"#;
    std::fs::write(traditional_dir.join("skill.json"), skill_json).unwrap();
    std::fs::write(
        traditional_dir.join("SKILL.md"),
        "# Traditional Skill\n\nTraditional format.",
    )
    .unwrap();

    let registry = Arc::new(Registry::new());
    let store = SkillStore::new(&skills_root, registry.clone()).unwrap();

    let skills = store.load_all().await.unwrap();
    assert_eq!(skills.len(), 2);

    let skill_names: Vec<_> = skills.iter().map(|s| s.manifest.id.as_str()).collect();
    assert!(skill_names.contains(&"agent-skill"));
    assert!(skill_names.contains(&"traditional-skill"));
}

#[tokio::test]
async fn test_agent_skills_sync_basic() {
    let temp = TempDir::new().unwrap();
    let skills_root = temp.path().join("skills");
    std::fs::create_dir_all(&skills_root).unwrap();

    // Create a mock "remote" repo directory
    let mock_repo = temp.path().join("mock-repo");
    std::fs::create_dir_all(&mock_repo).unwrap();

    // Initialize git repo
    std::process::Command::new("git")
        .args(["init"])
        .current_dir(&mock_repo)
        .output()
        .unwrap();

    std::process::Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(&mock_repo)
        .output()
        .unwrap();

    std::process::Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(&mock_repo)
        .output()
        .unwrap();

    // Create skills in mock repo
    create_test_agent_skill(&mock_repo.join("skill-alpha"), "skill-alpha").unwrap();
    create_test_agent_skill(&mock_repo.join("skill-beta"), "skill-beta").unwrap();

    std::process::Command::new("git")
        .args(["add", "."])
        .current_dir(&mock_repo)
        .output()
        .unwrap();

    std::process::Command::new("git")
        .args(["commit", "-m", "Initial commit"])
        .current_dir(&mock_repo)
        .output()
        .unwrap();

    // Configure sync with file:// URL
    let repo_url = format!("file://{}", mock_repo.display());
    let config = AgentSkillsRepoConfig {
        repo: repo_url,
        git_ref: None,
        skills: Some(vec!["skill-alpha".to_string()]),
        alias: None,
    };

    let mut sync = AgentSkillsSync::new(&skills_root).await.unwrap();
    let report = sync.sync_all(&[config]).await.unwrap();

    eprintln!(
        "Report: added={:?}, errors={:?}",
        report.added, report.errors
    );

    assert_eq!(
        report.added.len(),
        1,
        "Expected 1 skill to be added, but got {}. Errors: {:?}",
        report.added.len(),
        report.errors
    );
    assert!(report.added.contains(&"skill-alpha".to_string()));
    assert!(!report.added.contains(&"skill-beta".to_string())); // Not in filter

    // Verify skill was copied
    assert!(skills_root.join("skill-alpha").join("SKILL.md").exists());
    assert!(!skills_root.join("skill-beta").exists());
}

#[tokio::test]
async fn test_agent_skills_sync_all_skills() {
    let temp = TempDir::new().unwrap();
    let skills_root = temp.path().join("skills");
    std::fs::create_dir_all(&skills_root).unwrap();

    let mock_repo = temp.path().join("mock-repo");
    std::fs::create_dir_all(&mock_repo).unwrap();

    std::process::Command::new("git")
        .args(["init"])
        .current_dir(&mock_repo)
        .output()
        .unwrap();

    std::process::Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(&mock_repo)
        .output()
        .unwrap();

    std::process::Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(&mock_repo)
        .output()
        .unwrap();

    create_test_agent_skill(&mock_repo.join("skill-one"), "skill-one").unwrap();
    create_test_agent_skill(&mock_repo.join("skill-two"), "skill-two").unwrap();

    std::process::Command::new("git")
        .args(["add", "."])
        .current_dir(&mock_repo)
        .output()
        .unwrap();

    std::process::Command::new("git")
        .args(["commit", "-m", "Initial commit"])
        .current_dir(&mock_repo)
        .output()
        .unwrap();

    let repo_url = format!("file://{}", mock_repo.display());
    let config = AgentSkillsRepoConfig {
        repo: repo_url,
        git_ref: None,
        skills: None, // Import all skills
        alias: None,
    };

    let mut sync = AgentSkillsSync::new(&skills_root).await.unwrap();
    let report = sync.sync_all(&[config]).await.unwrap();

    assert_eq!(report.added.len(), 2);
    assert!(report.added.contains(&"skill-one".to_string()));
    assert!(report.added.contains(&"skill-two".to_string()));
}

#[tokio::test]
async fn test_agent_skills_sync_removal() {
    let temp = TempDir::new().unwrap();
    let skills_root = temp.path().join("skills");
    std::fs::create_dir_all(&skills_root).unwrap();

    let mock_repo = temp.path().join("mock-repo");
    std::fs::create_dir_all(&mock_repo).unwrap();

    std::process::Command::new("git")
        .args(["init"])
        .current_dir(&mock_repo)
        .output()
        .unwrap();

    std::process::Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(&mock_repo)
        .output()
        .unwrap();

    std::process::Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(&mock_repo)
        .output()
        .unwrap();

    create_test_agent_skill(&mock_repo.join("skill-temp"), "skill-temp").unwrap();

    std::process::Command::new("git")
        .args(["add", "."])
        .current_dir(&mock_repo)
        .output()
        .unwrap();

    std::process::Command::new("git")
        .args(["commit", "-m", "Initial commit"])
        .current_dir(&mock_repo)
        .output()
        .unwrap();

    let repo_url = format!("file://{}", mock_repo.display());
    let config = AgentSkillsRepoConfig {
        repo: repo_url.clone(),
        git_ref: None,
        skills: None,
        alias: Some("test-repo".to_string()),
    };

    let mut sync = AgentSkillsSync::new(&skills_root).await.unwrap();

    // First sync: add skills
    let report = sync.sync_all(std::slice::from_ref(&config)).await.unwrap();
    assert_eq!(report.added.len(), 1);

    // Second sync: remove repo from config
    let report = sync.sync_all(&[]).await.unwrap();
    assert_eq!(report.removed.len(), 1);
    assert!(report.removed.contains(&"skill-temp".to_string()));
    assert!(!skills_root.join("skill-temp").exists());
}

#[tokio::test]
async fn test_windows_line_endings() {
    let temp = TempDir::new().unwrap();
    let skill_dir = temp.path().join("windows-skill");
    std::fs::create_dir_all(&skill_dir).unwrap();

    // Create SKILL.md with Windows line endings (\r\n)
    let skill_md = "---\r\nname: windows-skill\r\ndescription: Test Windows line endings\r\n---\r\n\r\n# Windows Skill\r\n\r\nThis has Windows line endings.\r\n";
    std::fs::write(skill_dir.join("SKILL.md"), skill_md).unwrap();

    let agent_skill = AgentSkill::from_directory(&skill_dir).await.unwrap();
    assert_eq!(agent_skill.frontmatter.name, "windows-skill");
    assert!(agent_skill.content.contains("# Windows Skill"));
}

#[tokio::test]
async fn test_mixed_line_endings() {
    let temp = TempDir::new().unwrap();
    let skill_dir = temp.path().join("mixed-skill");
    std::fs::create_dir_all(&skill_dir).unwrap();

    // Mix of \r\n and \n
    let skill_md = "---\r\nname: mixed-skill\r\ndescription: Mixed line endings\n---\n\n# Mixed Skill\r\n\nMixed content.\n";
    std::fs::write(skill_dir.join("SKILL.md"), skill_md).unwrap();

    let agent_skill = AgentSkill::from_directory(&skill_dir).await.unwrap();
    assert_eq!(agent_skill.frontmatter.name, "mixed-skill");
    assert!(agent_skill.content.contains("# Mixed Skill"));
}
