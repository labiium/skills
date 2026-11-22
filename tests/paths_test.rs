//! Tests for paths module

use skillsrs::paths::{PathsConfig, SkillsPaths};
use tempfile::TempDir;

#[test]
#[expect(
    clippy::expect_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
fn test_system_paths() {
    let paths = SkillsPaths::new().expect("Should create system paths");

    // Verify all paths are set
    assert!(!paths.data_dir.as_os_str().is_empty());
    assert!(!paths.config_dir.as_os_str().is_empty());
    assert!(!paths.cache_dir.as_os_str().is_empty());
    assert!(!paths.database_path.as_os_str().is_empty());
    assert!(!paths.skills_root.as_os_str().is_empty());
    assert!(!paths.logs_dir.as_os_str().is_empty());
}

#[test]
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
fn test_custom_root() {
    let temp_dir = TempDir::new().unwrap();
    let paths = SkillsPaths::with_root(temp_dir.path());

    assert_eq!(paths.data_dir, temp_dir.path());
    assert_eq!(paths.config_dir, temp_dir.path().join("config"));
    assert_eq!(paths.cache_dir, temp_dir.path().join("cache"));
    assert_eq!(paths.database_path, temp_dir.path().join("skills.db"));
    assert_eq!(paths.skills_root, temp_dir.path().join("skills"));
    assert_eq!(paths.logs_dir, temp_dir.path().join("logs"));
}

#[test]
#[expect(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
fn test_ensure_directories() {
    let temp_dir = TempDir::new().unwrap();
    let paths = SkillsPaths::with_root(temp_dir.path());

    paths
        .ensure_directories()
        .expect("Should create directories");

    assert!(paths.data_dir.exists());
    assert!(paths.config_dir.exists());
    assert!(paths.cache_dir.exists());
    assert!(paths.skills_root.exists());
    assert!(paths.logs_dir.exists());
}

#[test]
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
fn data_root_isolates_persistence_unless_explicitly_overridden() {
    let temp_dir = TempDir::new().unwrap();
    let managed = temp_dir.path().join("managed");
    let explicit_database = temp_dir.path().join("shared.db");
    let mut config = PathsConfig {
        data_dir: Some(managed.clone()),
        ..Default::default()
    };
    let paths = config.apply_to(SkillsPaths::with_root(temp_dir.path().join("old")));
    assert_eq!(paths.database_path, managed.join("skills.db"));
    assert_eq!(paths.cache_dir, managed.join("cache"));
    assert_eq!(paths.logs_dir, managed.join("logs"));
    config.database_path = Some(explicit_database.clone());
    let paths = config.apply_to(paths);
    assert_eq!(paths.database_path, explicit_database);
}

#[test]
#[expect(
    clippy::unwrap_used,
    reason = "test assertions intentionally fail fast with source-local context"
)]
fn test_display() {
    let temp_dir = TempDir::new().unwrap();
    let paths = SkillsPaths::with_root(temp_dir.path());

    let display = paths.display();
    assert!(display.contains("Data directory"));
    assert!(display.contains("Skills root"));
    assert!(display.contains(temp_dir.path().to_str().unwrap()));
}

#[test]
#[expect(clippy::unwrap_used, reason = "isolated fixture setup and assertions")]
fn shared_roots_respect_project_trust_and_repository_boundary() {
    let temp = TempDir::new().unwrap();
    let repository = temp.path().join("repo");
    let nested = repository.join("packages/app");
    let home = temp.path().join("home");
    std::fs::create_dir_all(repository.join(".git")).unwrap();
    std::fs::create_dir_all(&nested).unwrap();
    let user_only = skillsrs::paths::shared_skill_roots(None, &home);
    assert_eq!(
        user_only,
        vec![home.join(".agents/skills"), home.join(".claude/skills")]
    );
    let trusted = skillsrs::paths::shared_skill_roots(Some(&nested), &home);
    assert_eq!(trusted.first(), Some(&nested.join(".agents/skills")));
    assert!(trusted.contains(&repository.join(".agents/skills")));
    assert!(!trusted.contains(&temp.path().join(".agents/skills")));
    assert_eq!(trusted.last(), Some(&home.join(".claude/skills")));
}
