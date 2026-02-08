//! Tests for paths module

use skillsrs::paths::{PathsConfig, SkillsPaths};
use tempfile::TempDir;

#[test]
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
fn test_path_overrides() {
    let temp_dir = TempDir::new().unwrap();
    let custom_skills = temp_dir.path().join("custom_skills");

    let config = PathsConfig {
        skills_root: Some(custom_skills.clone()),
        ..Default::default()
    };

    let paths = config.apply_to(SkillsPaths::new().unwrap());
    assert_eq!(paths.skills_root, custom_skills);
}

#[test]
fn test_display() {
    let temp_dir = TempDir::new().unwrap();
    let paths = SkillsPaths::with_root(temp_dir.path());

    let display = paths.display();
    assert!(display.contains("Data directory"));
    assert!(display.contains("Skills root"));
    assert!(display.contains(temp_dir.path().to_str().unwrap()));
}
