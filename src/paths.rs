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
    /// # Errors
    /// Returns an error if the operation cannot be completed.
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
    /// # Errors
    /// Returns an error if the operation cannot be completed.
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
    #[must_use]
    pub fn default_config_file(&self) -> PathBuf {
        self.config_dir.join("config.yaml")
    }

    /// Get database directory (parent of database file)
    #[allow(dead_code)]
    #[must_use]
    pub fn database_dir(&self) -> PathBuf {
        self.database_path
            .parent()
            .map_or_else(PathBuf::new, Path::to_path_buf)
    }

    /// Display paths for informational purposes
    #[must_use]
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
        Self::new().unwrap_or_else(|_| Self::with_root("."))
    }
}

/// Configuration overrides for paths
#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
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

    /// Override logs directory
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logs_dir: Option<PathBuf>,
}

impl PathsConfig {
    /// Apply overrides to default paths
    ///
    /// Note: `skills_root` is always derived from `data_dir/skills`/ to enforce
    /// the nested directory structure regardless of configuration.
    #[must_use]
    pub fn apply_to(&self, mut paths: SkillsPaths) -> SkillsPaths {
        if let Some(ref data_dir) = self.data_dir {
            paths.data_dir.clone_from(data_dir);
            if self.database_path.is_none() {
                paths.database_path = data_dir.join("skills.db");
            }
            if self.cache_dir.is_none() {
                paths.cache_dir = data_dir.join("cache");
            }
            if self.logs_dir.is_none() {
                paths.logs_dir = data_dir.join("logs");
            }
        }
        if let Some(ref config_dir) = self.config_dir {
            paths.config_dir.clone_from(config_dir);
        }
        if let Some(ref cache_dir) = self.cache_dir {
            paths.cache_dir.clone_from(cache_dir);
        }
        if let Some(ref database_path) = self.database_path {
            paths.database_path.clone_from(database_path);
        }
        // Always derive skills_root from data_dir/skills/
        // This enforces the nested structure: .skills/skills/
        paths.skills_root = paths.data_dir.join("skills");
        if let Some(ref logs_dir) = self.logs_dir {
            paths.logs_dir.clone_from(logs_dir);
        }
        paths
    }
}

/// Get paths from environment variable
///
/// Only `SKILLS_PATH` is supported. All other paths are derived from it:
/// - Skills: {`SKILLS_PATH}/skills`/
/// - Database: {`SKILLS_PATH}/skills.db`
/// - Cache: {`SKILLS_PATH}/cache`/
/// - Logs: {`SKILLS_PATH}/logs`/
#[must_use]
pub fn paths_from_env() -> PathsConfig {
    std::env::var("SKILLS_PATH").map_or_else(
        |_| PathsConfig::default(),
        |path| {
            let base = PathBuf::from(path);
            PathsConfig {
                data_dir: Some(base.clone()),
                config_dir: Some(base.join("config")),
                cache_dir: Some(base.join("cache")),
                database_path: Some(base.join("skills.db")),
                logs_dir: Some(base.join("logs")),
            }
        },
    )
}

/// Ordered native skill locations: trusted project scopes, then user scopes.
#[must_use]
pub fn shared_skill_roots(project: Option<&Path>, home: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(project) = project {
        let git_root = project.ancestors().find(|path| path.join(".git").exists());
        for directory in project.ancestors() {
            roots.push(directory.join(".agents/skills"));
            roots.push(directory.join(".claude/skills"));
            if git_root.is_none() || git_root == Some(directory) {
                break;
            }
        }
    }
    roots.push(home.join(".agents/skills"));
    roots.push(home.join(".claude/skills"));
    roots
}
