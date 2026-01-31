//! System paths and directory management for skills.rs
//!
//! This module handles platform-appropriate directory locations for:
//! - Skills storage
//! - Database persistence
//! - Configuration files
//! - Cache and logs
//!
//! It follows XDG Base Directory specification on Linux and platform conventions
//! on macOS and Windows.

use anyhow::{Context, Result};
use directories::ProjectDirs;
use std::path::{Path, PathBuf};

/// Application identifier for directory structures
const APP_QUALIFIER: &str = "rs";
const APP_ORGANIZATION: &str = "labiium";
const APP_NAME: &str = "skills";

/// System paths for skills.rs
#[derive(Debug, Clone)]
pub struct SkillsPaths {
    /// Root data directory (skills storage)
    pub data_dir: PathBuf,

    /// Configuration directory
    pub config_dir: PathBuf,

    /// Cache directory
    pub cache_dir: PathBuf,

    /// Database file path
    pub database_path: PathBuf,

    /// Skills root directory
    pub skills_root: PathBuf,

    /// Logs directory
    pub logs_dir: PathBuf,
}

impl SkillsPaths {
    /// Create paths using system defaults
    ///
    /// Uses platform-specific directories:
    /// - Linux: ~/.local/share/skills, ~/.config/skills, ~/.cache/skills
    /// - macOS: ~/Library/Application Support/skills, ~/Library/Preferences/skills, ~/Library/Caches/skills
    /// - Windows: %APPDATA%\labiium\skills, %LOCALAPPDATA%\labiium\skills\cache
    pub fn new() -> Result<Self> {
        let project_dirs = ProjectDirs::from(APP_QUALIFIER, APP_ORGANIZATION, APP_NAME)
            .context("Failed to determine system directories")?;

        let data_dir = project_dirs.data_dir().to_path_buf();
        let config_dir = project_dirs.config_dir().to_path_buf();
        let cache_dir = project_dirs.cache_dir().to_path_buf();

        let database_path = data_dir.join("skills.db");
        let skills_root = data_dir.join("skills");
        let logs_dir = data_dir.join("logs");

        Ok(Self {
            data_dir,
            config_dir,
            cache_dir,
            database_path,
            skills_root,
            logs_dir,
        })
    }

    /// Create paths with custom root directory
    ///
    /// Useful for testing or custom installations.
    /// All paths will be placed under the root directory.
    #[allow(dead_code)]
    pub fn with_root<P: AsRef<Path>>(root: P) -> Self {
        let root = root.as_ref().to_path_buf();

        Self {
            data_dir: root.clone(),
            config_dir: root.join("config"),
            cache_dir: root.join("cache"),
            database_path: root.join("skills.db"),
            skills_root: root.join("skills"),
            logs_dir: root.join("logs"),
        }
    }

    /// Create all necessary directories
    ///
    /// Creates directories with appropriate permissions.
    /// Safe to call multiple times.
    pub fn ensure_directories(&self) -> Result<()> {
        let dirs = [
            &self.data_dir,
            &self.config_dir,
            &self.cache_dir,
            &self.skills_root,
            &self.logs_dir,
        ];

        for dir in &dirs {
            if !dir.exists() {
                std::fs::create_dir_all(dir)
                    .with_context(|| format!("Failed to create directory: {}", dir.display()))?;
            }
        }

        Ok(())
    }

    /// Get default config file path
    pub fn default_config_file(&self) -> PathBuf {
        self.config_dir.join("config.yaml")
    }

    /// Get database directory (parent of database file)
    #[allow(dead_code)]
    pub fn database_dir(&self) -> PathBuf {
        self.database_path.parent().unwrap().to_path_buf()
    }

    /// Display paths for informational purposes
    pub fn display(&self) -> String {
        format!(
            "Skills paths:
  Data directory:    {}
  Config directory:  {}
  Cache directory:   {}
  Database:          {}
  Skills root:       {}
  Logs directory:    {}",
            self.data_dir.display(),
            self.config_dir.display(),
            self.cache_dir.display(),
            self.database_path.display(),
            self.skills_root.display(),
            self.logs_dir.display()
        )
    }
}

impl Default for SkillsPaths {
    fn default() -> Self {
        Self::new().expect("Failed to determine system directories")
    }
}

/// Configuration overrides for paths
#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
pub struct PathsConfig {
    /// Override data directory
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_dir: Option<PathBuf>,

    /// Override config directory
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_dir: Option<PathBuf>,

    /// Override cache directory
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_dir: Option<PathBuf>,

    /// Override database path
    #[serde(skip_serializing_if = "Option::is_none")]
    pub database_path: Option<PathBuf>,

    /// Override skills root
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skills_root: Option<PathBuf>,

    /// Override logs directory
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logs_dir: Option<PathBuf>,
}

impl PathsConfig {
    /// Apply overrides to default paths
    pub fn apply_to(&self, mut paths: SkillsPaths) -> SkillsPaths {
        if let Some(ref data_dir) = self.data_dir {
            paths.data_dir = data_dir.clone();
        }
        if let Some(ref config_dir) = self.config_dir {
            paths.config_dir = config_dir.clone();
        }
        if let Some(ref cache_dir) = self.cache_dir {
            paths.cache_dir = cache_dir.clone();
        }
        if let Some(ref database_path) = self.database_path {
            paths.database_path = database_path.clone();
        }
        if let Some(ref skills_root) = self.skills_root {
            paths.skills_root = skills_root.clone();
        }
        if let Some(ref logs_dir) = self.logs_dir {
            paths.logs_dir = logs_dir.clone();
        }
        paths
    }
}

/// Get paths from environment variables or config
///
/// Environment variables take precedence:
/// - SKILLS_DATA_DIR
/// - SKILLS_CONFIG_DIR
/// - SKILLS_CACHE_DIR
/// - SKILLS_DATABASE_PATH
/// - SKILLS_ROOT
/// - SKILLS_LOGS_DIR
pub fn paths_from_env() -> PathsConfig {
    PathsConfig {
        data_dir: std::env::var("SKILLS_DATA_DIR").ok().map(PathBuf::from),
        config_dir: std::env::var("SKILLS_CONFIG_DIR").ok().map(PathBuf::from),
        cache_dir: std::env::var("SKILLS_CACHE_DIR").ok().map(PathBuf::from),
        database_path: std::env::var("SKILLS_DATABASE_PATH")
            .ok()
            .map(PathBuf::from),
        skills_root: std::env::var("SKILLS_ROOT").ok().map(PathBuf::from),
        logs_dir: std::env::var("SKILLS_LOGS_DIR").ok().map(PathBuf::from),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
