//! Agent Skills Format Support
//!
//! Parses and converts skills in the Agent Skills format (Vercel skills.sh compatible)
//! to the internal skills.rs format.
//!
//! Agent Skills Format:
//! - SKILL.md with YAML frontmatter
//! - Optional scripts/, references/, assets/ directories
//! - Lowercase, hyphenated naming convention
//!
//! Spec: https://agentskills.io/specification

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum AgentSkillsError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("YAML parse error: {0}")]
    YamlParse(#[from] serde_yaml::Error),

    #[error("Invalid skill format: {0}")]
    InvalidFormat(String),

    #[error("Missing required field: {0}")]
    MissingField(String),

    #[error("Validation error: {0}")]
    ValidationError(String),
}

pub type Result<T> = std::result::Result<T, AgentSkillsError>;

/// Flexible allowed-tools field that accepts both string and array formats
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AllowedTools {
    /// Space-delimited string format: "Bash Read Write"
    String(String),
    /// Array format: ["Bash", "Read", "Write"]
    Array(Vec<String>),
}

impl AllowedTools {
    /// Convert to Vec<String> regardless of format
    pub fn to_vec(&self) -> Vec<String> {
        match self {
            AllowedTools::String(s) => s.split_whitespace().map(|t| t.to_string()).collect(),
            AllowedTools::Array(arr) => arr.clone(),
        }
    }
}

/// Agent Skills YAML frontmatter
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSkillsFrontmatter {
    /// Skill name (required, must match directory name)
    pub name: String,

    /// Description (optional for command-only skills, max 1024 chars)
    /// Some skills intentionally omit description to prevent auto-triggering
    #[serde(default)]
    pub description: Option<String>,

    /// License (optional)
    #[serde(default)]
    pub license: Option<String>,

    /// Compatibility notes (optional, max 500 chars)
    #[serde(default)]
    pub compatibility: Option<String>,

    /// Additional metadata (optional)
    #[serde(default)]
    pub metadata: HashMap<String, String>,

    /// Pre-approved tools (experimental)
    /// Accepts both string format ("Bash Read Write") and array format (["Bash", "Read", "Write"])
    #[serde(default, rename = "allowed-tools")]
    pub allowed_tools: Option<AllowedTools>,
}

/// Parsed Agent Skill
#[derive(Debug, Clone)]
pub struct AgentSkill {
    pub frontmatter: AgentSkillsFrontmatter,
    pub content: String,
    pub path: PathBuf,
    pub scripts: Vec<PathBuf>,
    pub references: Vec<PathBuf>,
    pub assets: Vec<PathBuf>,
}

impl AgentSkill {
    /// Parse an Agent Skill from a directory
    pub async fn from_directory(path: &Path) -> Result<Self> {
        let skill_md_path = path.join("SKILL.md");

        if !skill_md_path.exists() {
            return Err(AgentSkillsError::InvalidFormat(
                "SKILL.md not found".to_string(),
            ));
        }

        let content = tokio::fs::read_to_string(&skill_md_path).await?;
        let (frontmatter, body) = Self::parse_frontmatter(&content)?;

        // Validate name matches directory
        let dir_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| AgentSkillsError::InvalidFormat("Invalid directory name".to_string()))?;

        if frontmatter.name != dir_name {
            return Err(AgentSkillsError::ValidationError(format!(
                "Skill name '{}' does not match directory name '{}'",
                frontmatter.name, dir_name
            )));
        }

        // Validate name format
        Self::validate_name(&frontmatter.name)?;

        // Discover optional directories
        let scripts = Self::discover_files(&path.join("scripts")).await;
        let references = Self::discover_files(&path.join("references")).await;
        let assets = Self::discover_files(&path.join("assets")).await;

        Ok(AgentSkill {
            frontmatter,
            content: body,
            path: path.to_path_buf(),
            scripts,
            references,
            assets,
        })
    }

    /// Parse YAML frontmatter from SKILL.md (private)
    fn parse_frontmatter(content: &str) -> Result<(AgentSkillsFrontmatter, String)> {
        // Normalize line endings to \n for consistent parsing
        let normalized = content.replace("\r\n", "\n");

        // Check for frontmatter delimiters
        if !normalized.starts_with("---\n") {
            return Err(AgentSkillsError::InvalidFormat(
                "SKILL.md must start with YAML frontmatter (---)".to_string(),
            ));
        }

        // Find the closing delimiter
        let after_first = &normalized[4..]; // Skip "---\n"
        let end_pos = after_first.find("\n---\n").ok_or_else(|| {
            AgentSkillsError::InvalidFormat(
                "SKILL.md frontmatter not properly closed with ---".to_string(),
            )
        })?;

        let yaml_content = &after_first[..end_pos];
        let body_start = 4 + end_pos + 5; // Skip "---\n" + frontmatter + "\n---\n"
        let body = normalized[body_start..].trim().to_string();

        let frontmatter: AgentSkillsFrontmatter = serde_yaml::from_str(yaml_content)?;

        // Validate required fields
        if frontmatter.name.is_empty() {
            return Err(AgentSkillsError::MissingField("name".to_string()));
        }

        // Validate field constraints
        if frontmatter.name.len() > 64 {
            return Err(AgentSkillsError::ValidationError(
                "name exceeds 64 characters".to_string(),
            ));
        }
        if let Some(ref desc) = frontmatter.description {
            if desc.len() > 1024 {
                return Err(AgentSkillsError::ValidationError(
                    "description exceeds 1024 characters".to_string(),
                ));
            }
        }
        if let Some(ref compat) = frontmatter.compatibility {
            if compat.len() > 500 {
                return Err(AgentSkillsError::ValidationError(
                    "compatibility exceeds 500 characters".to_string(),
                ));
            }
        }

        Ok((frontmatter, body))
    }

    /// Validate skill name format per Agent Skills spec
    fn validate_name(name: &str) -> Result<()> {
        if name.is_empty() || name.len() > 64 {
            return Err(AgentSkillsError::ValidationError(
                "name must be 1-64 characters".to_string(),
            ));
        }

        // Must be lowercase, numbers, hyphens only
        if !name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            return Err(AgentSkillsError::ValidationError(
                "name must contain only lowercase letters, numbers, and hyphens".to_string(),
            ));
        }

        // Cannot start or end with hyphen
        if name.starts_with('-') || name.ends_with('-') {
            return Err(AgentSkillsError::ValidationError(
                "name cannot start or end with hyphen".to_string(),
            ));
        }

        // No consecutive hyphens
        if name.contains("--") {
            return Err(AgentSkillsError::ValidationError(
                "name cannot contain consecutive hyphens".to_string(),
            ));
        }

        Ok(())
    }

    /// Discover files in a directory (non-recursive)
    async fn discover_files(dir: &Path) -> Vec<PathBuf> {
        if !dir.exists() {
            return Vec::new();
        }

        let mut files = Vec::new();
        if let Ok(mut entries) = tokio::fs::read_dir(dir).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                if let Ok(metadata) = entry.metadata().await {
                    if metadata.is_file() {
                        files.push(entry.path());
                    }
                }
            }
        }
        files
    }

    /// Parse allowed-tools into a Vec (supports both string and array formats)
    pub fn parse_allowed_tools(&self) -> Vec<String> {
        self.frontmatter
            .allowed_tools
            .as_ref()
            .map(|tools| tools.to_vec())
            .unwrap_or_default()
    }

    /// Extract version from metadata or default to "1.0.0"
    pub fn version(&self) -> String {
        self.frontmatter
            .metadata
            .get("version")
            .cloned()
            .unwrap_or_else(|| "1.0.0".to_string())
    }

    /// Extract author from metadata
    pub fn author(&self) -> Option<String> {
        self.frontmatter.metadata.get("author").cloned()
    }

    /// Check if skill has scripts
    pub fn has_scripts(&self) -> bool {
        !self.scripts.is_empty()
    }

    /// Check if skill has references
    pub fn has_references(&self) -> bool {
        !self.references.is_empty()
    }

    /// Check if skill has assets
    pub fn has_assets(&self) -> bool {
        !self.assets.is_empty()
    }

    /// Convert Agent Skill to skills.rs SkillManifest format
    pub fn to_skill_manifest(&self) -> crate::SkillManifest {
        use crate::{EntrypointType, SkillHints, SkillManifest, ToolPolicy};

        // Determine entrypoint type
        let entrypoint = if self.has_scripts() {
            EntrypointType::Script
        } else {
            EntrypointType::Prompted
        };

        // Parse allowed tools for tool policy
        let allowed_tools = self.parse_allowed_tools();
        let tool_policy = ToolPolicy {
            allow: if allowed_tools.is_empty() {
                vec!["*".to_string()]
            } else {
                allowed_tools
            },
            deny: vec![],
            required: vec![],
        };

        // Get description (use empty string if not provided for command-only skills)
        let description = self
            .frontmatter
            .description
            .clone()
            .unwrap_or_else(|| format!("Command-only skill: {}", self.frontmatter.name));

        // Extract hints from description (keywords for search)
        let description_words: Vec<String> = description
            .split_whitespace()
            .filter(|w| w.len() > 3)
            .take(10)
            .map(|s| s.to_lowercase().trim_end_matches('.').to_string())
            .collect();

        let hints = SkillHints {
            intent: description_words.clone(),
            domain: vec![],
            outcomes: vec![],
            expected_calls: None,
        };

        // Create input schema (generic object for Agent Skills)
        let inputs = serde_json::json!({
            "type": "object",
            "properties": {},
            "additionalProperties": true,
            "description": description.clone()
        });

        SkillManifest {
            id: self.frontmatter.name.clone(),
            title: self
                .frontmatter
                .name
                .split('-')
                .map(|w| {
                    let mut chars = w.chars();
                    match chars.next() {
                        None => String::new(),
                        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                    }
                })
                .collect::<Vec<_>>()
                .join(" "),
            version: self.version(),
            description,
            inputs,
            outputs: None,
            entrypoint,
            tool_policy,
            hints,
            risk_tier: None, // Will be inferred
        }
    }

    /// Convert Agent Skill to SkillContent for progressive disclosure
    pub async fn to_skill_content(&self) -> crate::SkillContent {
        use crate::core::BundledTool;
        use crate::SkillContent;

        let mut bundled_tools = Vec::new();
        let mut additional_files = Vec::new();

        // Add scripts as bundled tools
        for script_path in &self.scripts {
            if let Some(filename) = script_path.file_name().and_then(|n| n.to_str()) {
                let interpreter = Self::infer_interpreter(filename);
                let tool = BundledTool {
                    name: filename.to_string(),
                    description: format!("Bundled script: {}", filename),
                    command: vec![interpreter, filename.to_string()],
                    schema: serde_json::json!({
                        "type": "object",
                        "properties": {},
                        "additionalProperties": true
                    }),
                };
                bundled_tools.push(tool);
            }
        }

        // Add references as additional files
        for ref_path in &self.references {
            if let Some(filename) = ref_path.file_name().and_then(|n| n.to_str()) {
                additional_files.push(format!("references/{}", filename));
            }
        }

        // Add assets as additional files
        for asset_path in &self.assets {
            if let Some(filename) = asset_path.file_name().and_then(|n| n.to_str()) {
                additional_files.push(format!("assets/{}", filename));
            }
        }

        SkillContent {
            skill_md: self.content.clone(),
            additional_files,
            bundled_tools,
            uses_tools: self.parse_allowed_tools(),
        }
    }

    /// Infer interpreter from file extension
    fn infer_interpreter(filename: &str) -> String {
        if filename.ends_with(".py") {
            "python3".to_string()
        } else if filename.ends_with(".sh") {
            "bash".to_string()
        } else if filename.ends_with(".js") {
            "node".to_string()
        } else {
            "bash".to_string() // default
        }
    }
}

/// Public wrapper to parse frontmatter for backward compatibility with old-format skills
pub fn parse_frontmatter_public(content: &str) -> Result<(AgentSkillsFrontmatter, String)> {
    AgentSkill::parse_frontmatter(content)
}
