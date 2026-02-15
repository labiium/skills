//! Agent Skills Synchronization
//!
//! Automatically syncs Agent Skills from repositories declared in config.yaml.
//! Manages the lifecycle of skills: fetching, updating, and cleanup.

use crate::storage::agent_skills::AgentSkill;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use tokio::process::Command;
use tracing::{debug, info, warn};

/// Agent Skills repository configuration (from config.yaml)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSkillsRepoConfig {
    /// Repository URL or GitHub shorthand (owner/repo)
    pub repo: String,

    /// Git ref (branch, tag, or commit) - defaults to main/master
    #[serde(default)]
    pub git_ref: Option<String>,

    /// Specific skill names to import (if omitted, imports all)
    #[serde(default)]
    pub skills: Option<Vec<String>>,

    /// Alias for tracking (auto-generated from repo URL if not provided)
    #[serde(default)]
    pub alias: Option<String>,
}

/// Metadata about synced skills for tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncMetadata {
    pub repo: String,
    pub git_ref: Option<String>,
    pub commit_sha: String,
    pub synced_skills: Vec<String>,
    pub last_sync: chrono::DateTime<chrono::Utc>,
}

/// Agent Skills synchronizer
pub struct AgentSkillsSync {
    skills_root: PathBuf,
    metadata_path: PathBuf,
    metadata: HashMap<String, SyncMetadata>,
}

impl AgentSkillsSync {
    /// Create a new synchronizer
    pub async fn new(skills_root: impl AsRef<Path>) -> Result<Self> {
        let skills_root = skills_root.as_ref().to_path_buf();
        let metadata_path = skills_root.join(".agent-skills-sync.json");

        // Load existing metadata
        let metadata = if metadata_path.exists() {
            let content = tokio::fs::read_to_string(&metadata_path).await?;
            serde_json::from_str(&content).unwrap_or_default()
        } else {
            HashMap::new()
        };

        Ok(Self {
            skills_root,
            metadata_path,
            metadata,
        })
    }

    /// Sync all Agent Skills repositories from config
    pub async fn sync_all(&mut self, repos: &[AgentSkillsRepoConfig]) -> Result<SyncReport> {
        let mut report = SyncReport::default();

        // Get current repo aliases from config
        let config_aliases: HashSet<String> = repos.iter().map(|r| self.repo_alias(r)).collect();

        // Find repos that were removed from config
        let removed_aliases: Vec<String> = self
            .metadata
            .keys()
            .filter(|alias| !config_aliases.contains(*alias))
            .cloned()
            .collect();

        // Clean up removed repos
        for alias in removed_aliases {
            info!("Removing skills from deleted repo: {}", alias);
            if let Some(meta) = self.metadata.get(&alias) {
                for skill_name in &meta.synced_skills {
                    let skill_path = self.skills_root.join(skill_name);
                    if skill_path.exists() {
                        match tokio::fs::remove_dir_all(&skill_path).await {
                            Ok(_) => {
                                report.removed.push(skill_name.clone());
                                info!("Removed skill: {}", skill_name);
                            }
                            Err(e) => {
                                warn!("Failed to remove skill {}: {}", skill_name, e);
                                report.errors.push(format!("Remove {}: {}", skill_name, e));
                            }
                        }
                    }
                }
                self.metadata.remove(&alias);
            }
        }

        // Sync each configured repo
        for repo_config in repos {
            let alias = self.repo_alias(repo_config);
            info!(
                "Syncing Agent Skills from: {} ({})",
                repo_config.repo, alias
            );

            match self.sync_repo(repo_config, &alias).await {
                Ok(repo_report) => {
                    report.merge(repo_report);
                }
                Err(e) => {
                    warn!("Failed to sync repo {}: {}", repo_config.repo, e);
                    report.errors.push(format!("{}: {}", repo_config.repo, e));
                }
            }
        }

        // Save metadata
        self.save_metadata().await?;

        Ok(report)
    }

    /// Sync a single repository
    async fn sync_repo(
        &mut self,
        config: &AgentSkillsRepoConfig,
        alias: &str,
    ) -> Result<SyncReport> {
        let mut report = SyncReport::default();

        // Parse repo URL
        let repo_url = if config.repo.starts_with("http://")
            || config.repo.starts_with("https://")
            || config.repo.starts_with("file://")
        // Allow file:// for testing
        {
            config.repo.clone()
        } else if config.repo.contains('/') && !config.repo.contains(':') {
            format!("https://github.com/{}", config.repo)
        } else {
            return Err(anyhow::anyhow!(format!(
                "Invalid repo format: {}",
                config.repo
            )));
        };

        // Check if already synced with same commit
        let needs_sync = if let Some(meta) = self.metadata.get(alias) {
            // Check if config changed (different git_ref or skill filter)
            meta.git_ref != config.git_ref || {
                match (&config.skills, !meta.synced_skills.is_empty()) {
                    (Some(config_skills), true) => {
                        // If config specifies skills, check if they match
                        let meta_set: HashSet<_> = meta.synced_skills.iter().collect();
                        let config_set: HashSet<_> = config_skills.iter().collect();
                        meta_set != config_set
                    }
                    _ => false,
                }
            }
        } else {
            true
        };

        if !needs_sync {
            debug!("Repo {} already up-to-date", alias);
            return Ok(report);
        }

        // Clone to temp directory
        let temp_dir = tempfile::tempdir()?;
        let clone_path = temp_dir.path();

        debug!("Cloning {} to {:?}", repo_url, clone_path);

        let mut cmd = Command::new("git");
        cmd.arg("clone");
        if let Some(ref git_ref) = config.git_ref {
            cmd.arg("--branch").arg(git_ref);
        }
        cmd.arg("--depth").arg("1");
        cmd.arg(&repo_url);
        cmd.arg(clone_path);

        let output = cmd.output().await?;
        if !output.status.success() {
            return Err(anyhow::anyhow!(std::io::Error::other(format!(
                "Git clone failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ))));
        }

        // Get commit SHA
        let commit_sha = self.get_commit_sha(clone_path).await?;

        // Discover all SKILL.md files
        let mut discovered_skills = Vec::new();
        Self::find_skill_md(clone_path, &mut discovered_skills);

        if discovered_skills.is_empty() {
            warn!("No Agent Skills found in repository: {}", repo_url);
            return Ok(report);
        }

        // Filter by requested skills if specified
        let skills_to_import: Vec<PathBuf> = if let Some(ref skill_filter) = config.skills {
            discovered_skills
                .into_iter()
                .filter(|path| {
                    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                        skill_filter.contains(&name.to_string())
                    } else {
                        false
                    }
                })
                .collect()
        } else {
            discovered_skills
        };

        if skills_to_import.is_empty() {
            warn!("None of the requested skills found in: {}", repo_url);
            return Ok(report);
        }

        // Remove old skills from this repo if they exist
        if let Some(meta) = self.metadata.get(alias) {
            for old_skill in &meta.synced_skills {
                if !skills_to_import.iter().any(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| n == old_skill)
                        .unwrap_or(false)
                }) {
                    let skill_path = self.skills_root.join(old_skill);
                    if skill_path.exists() {
                        tokio::fs::remove_dir_all(&skill_path).await?;
                        report.removed.push(old_skill.clone());
                    }
                }
            }
        }

        // Import skills
        let mut synced_skills = Vec::new();
        for skill_path in &skills_to_import {
            let skill_name = skill_path
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(|| anyhow::anyhow!("Invalid skill name".to_string()))?;

            // Parse and validate the skill
            match AgentSkill::from_directory(skill_path).await {
                Ok(_agent_skill) => {
                    // Copy skill directory
                    let dest_path = self.skills_root.join(skill_name);

                    // Remove if exists
                    if dest_path.exists() {
                        tokio::fs::remove_dir_all(&dest_path).await?;
                        report.updated.push(skill_name.to_string());
                    } else {
                        report.added.push(skill_name.to_string());
                    }

                    Self::copy_dir_all(skill_path, &dest_path).await?;
                    synced_skills.push(skill_name.to_string());
                }
                Err(e) => {
                    warn!("Failed to parse skill {}: {}", skill_name, e);
                    report.errors.push(format!("{}: {}", skill_name, e));
                }
            }
        }

        // Update metadata
        let metadata = SyncMetadata {
            repo: config.repo.clone(),
            git_ref: config.git_ref.clone(),
            commit_sha,
            synced_skills,
            last_sync: chrono::Utc::now(),
        };
        self.metadata.insert(alias.to_string(), metadata);

        Ok(report)
    }

    /// Generate a unique alias for a repo
    fn repo_alias(&self, config: &AgentSkillsRepoConfig) -> String {
        if let Some(ref alias) = config.alias {
            return alias.clone();
        }

        // Generate from repo URL
        let repo = &config.repo;
        if let Some(last) = repo.split('/').next_back() {
            last.trim_end_matches(".git").to_string()
        } else {
            repo.replace(['/', ':'], "-")
        }
    }

    /// Get current commit SHA from a cloned repo
    async fn get_commit_sha(&self, repo_path: &Path) -> Result<String> {
        let output = Command::new("git")
            .arg("rev-parse")
            .arg("HEAD")
            .current_dir(repo_path)
            .output()
            .await?;

        if !output.status.success() {
            return Err(anyhow::anyhow!(std::io::Error::other(
                "Failed to get commit SHA",
            )));
        }

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// Find all directories containing SKILL.md
    fn find_skill_md(dir: &Path, found: &mut Vec<PathBuf>) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if path.join("SKILL.md").exists() {
                        found.push(path);
                    } else {
                        // Recurse into subdirectories
                        Self::find_skill_md(&path, found);
                    }
                }
            }
        }
    }

    /// Copy directory recursively (boxed to avoid recursion issues)
    fn copy_dir_all<'a>(
        src: &'a Path,
        dst: &'a Path,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = std::io::Result<()>> + 'a + Send>> {
        Box::pin(async move {
            tokio::fs::create_dir_all(dst).await?;
            let mut entries = tokio::fs::read_dir(src).await?;

            while let Some(entry) = entries.next_entry().await? {
                let path = entry.path();
                let dest_path = dst.join(entry.file_name());

                if path.is_dir() {
                    Self::copy_dir_all(&path, &dest_path).await?;
                } else {
                    tokio::fs::copy(&path, &dest_path).await?;
                }
            }
            Ok(())
        })
    }

    /// Save metadata to disk
    async fn save_metadata(&self) -> Result<()> {
        let json = serde_json::to_string_pretty(&self.metadata)
            .map_err(|e| crate::SkillStoreError::Parse(e.to_string()))?;
        tokio::fs::write(&self.metadata_path, json).await?;
        Ok(())
    }
}

/// Report of sync operations
#[derive(Debug, Default, Clone)]
pub struct SyncReport {
    pub added: Vec<String>,
    pub updated: Vec<String>,
    pub removed: Vec<String>,
    pub errors: Vec<String>,
}

impl SyncReport {
    pub fn merge(&mut self, other: SyncReport) {
        self.added.extend(other.added);
        self.updated.extend(other.updated);
        self.removed.extend(other.removed);
        self.errors.extend(other.errors);
    }

    pub fn is_empty(&self) -> bool {
        self.added.is_empty()
            && self.updated.is_empty()
            && self.removed.is_empty()
            && self.errors.is_empty()
    }

    pub fn total_changes(&self) -> usize {
        self.added.len() + self.updated.len() + self.removed.len()
    }
}
