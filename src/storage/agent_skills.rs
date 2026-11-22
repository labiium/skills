//! Agent Skills Format Support
//!
//! Parses Agent Skills documents and resources into the internal skills.rs format.
//!
//! Agent Skills Format:
//! - SKILL.md with YAML frontmatter
//! - Optional scripts/, references/, assets/ directories
//! - Lowercase, hyphenated naming convention
//!
//! Spec: <https://agentskills.io/specification>

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
    /// Space-delimited string format: `Bash Read Write`
    String(String),
    /// Array format: `["Bash", "Read", "Write"]`
    Array(Vec<String>),
}

impl AllowedTools {
    /// Convert to Vec<String> regardless of format
    #[must_use]
    pub fn to_vec(&self) -> Vec<String> {
        match self {
            Self::String(s) => s
                .split_whitespace()
                .map(std::string::ToString::to_string)
                .collect(),
            Self::Array(arr) => arr.clone(),
        }
    }
}

/// Agent Skills YAML frontmatter
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSkillsFrontmatter {
    /// Skill name (required, must match directory name)
    pub name: String,

    /// Required discovery description (max 1024 Unicode characters).
    #[serde(default)]
    pub description: Option<String>,

    /// Version (optional, defaults to "1.0.0")
    #[serde(default = "default_version")]
    pub version: String,

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
    /// Accepts both string format (`Bash Read Write`) and array format
    /// (`["Bash", "Read", "Write"]`).
    #[serde(default, rename = "allowed-tools")]
    pub allowed_tools: Option<AllowedTools>,

    /// Host-specific fields, retained without interpreting their values.
    #[serde(flatten)]
    pub extra: std::collections::BTreeMap<String, serde_yaml::Value>,
}

/// Default version for Agent Skills
fn default_version() -> String {
    "1.0.0".to_string()
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
    /// # Errors
    /// Returns an error if the document is missing, unreadable or invalid, its name
    /// differs from the directory, or a resource escapes the package.
    pub async fn from_directory(path: &Path) -> Result<Self> {
        Self::from_directory_with_name_check(path, true).await
    }

    pub(crate) async fn from_directory_with_name_check(
        path: &Path,
        check_name: bool,
    ) -> Result<Self> {
        let skill_md_path = path.join("SKILL.md");

        if !skill_md_path.exists() {
            return Err(AgentSkillsError::InvalidFormat(
                "SKILL.md not found".to_string(),
            ));
        }
        super::contained_file(path, "SKILL.md")?;

        let content = tokio::fs::read_to_string(&skill_md_path).await?;
        let (frontmatter, body) = Self::parse_frontmatter(&content)?;

        // Validate name matches directory
        let dir_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| AgentSkillsError::InvalidFormat("Invalid directory name".to_string()))?;

        if check_name && frontmatter.name != dir_name {
            return Err(AgentSkillsError::ValidationError(format!(
                "Skill name '{}' does not match directory name '{}'",
                frontmatter.name, dir_name
            )));
        }

        let files = super::inventory_files(path)?;
        let in_directory = |directory: &str| {
            files
                .iter()
                .filter(|file| file.starts_with(directory))
                .map(|file| path.join(file))
                .collect()
        };
        let scripts = in_directory("scripts");
        let references = in_directory("references");
        let assets = in_directory("assets");

        Ok(Self {
            frontmatter,
            content: body,
            path: path.to_path_buf(),
            scripts,
            references,
            assets,
        })
    }

    pub(crate) fn source_name(content: &str) -> Result<String> {
        #[derive(Deserialize)]
        struct Name {
            name: String,
        }
        let normalized = normalized_document(content);
        let (yaml, _) = frontmatter_parts(&normalized)?;
        let name: Name = serde_yaml::from_str(yaml)?;
        Self::validate_name(&name.name)?;
        Ok(name.name)
    }

    fn parse_frontmatter(content: &str) -> Result<(AgentSkillsFrontmatter, String)> {
        let normalized = normalized_document(content);
        let (yaml_content, body) = frontmatter_parts(&normalized)?;
        let body = body.trim().to_string();
        let frontmatter: AgentSkillsFrontmatter = serde_yaml::from_str(yaml_content)?;

        // Validate required fields
        if frontmatter.name.is_empty() {
            return Err(AgentSkillsError::MissingField("name".to_string()));
        }

        Self::validate_name(&frontmatter.name)?;
        let description = frontmatter
            .description
            .as_deref()
            .ok_or_else(|| AgentSkillsError::MissingField("description".to_string()))?;
        if description.trim().is_empty() {
            return Err(AgentSkillsError::MissingField("description".to_string()));
        }
        if description.chars().count() > 1024 {
            return Err(AgentSkillsError::ValidationError(
                "description exceeds 1024 characters".to_string(),
            ));
        }
        if let Some(ref compat) = frontmatter.compatibility {
            if compat.trim().is_empty() || compat.chars().count() > 500 {
                return Err(AgentSkillsError::ValidationError(
                    "compatibility must contain 1-500 characters".to_string(),
                ));
            }
        }

        Ok((frontmatter, body))
    }

    /// Validate skill name format per Agent Skills spec
    fn validate_name(name: &str) -> Result<()> {
        if name.is_empty() || name.chars().count() > 64 {
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

    /// Parse allowed-tools into a Vec (supports both string and array formats)
    #[must_use]
    pub fn parse_allowed_tools(&self) -> Vec<String> {
        self.frontmatter
            .allowed_tools
            .as_ref()
            .map(AllowedTools::to_vec)
            .unwrap_or_default()
    }

    /// Extract version from frontmatter
    #[must_use]
    pub fn version(&self) -> String {
        self.frontmatter.version.clone()
    }

    /// Extract author from metadata
    #[must_use]
    pub fn author(&self) -> Option<String> {
        self.frontmatter.metadata.get("author").cloned()
    }

    /// Check if skill has scripts
    #[must_use]
    pub const fn has_scripts(&self) -> bool {
        !self.scripts.is_empty()
    }

    /// Check if skill has references
    #[must_use]
    pub const fn has_references(&self) -> bool {
        !self.references.is_empty()
    }

    /// Check if skill has assets
    #[must_use]
    pub const fn has_assets(&self) -> bool {
        !self.assets.is_empty()
    }

    /// Convert Agent Skill to skills.rs `SkillManifest` format
    #[must_use]
    pub fn to_skill_manifest(&self) -> crate::SkillManifest {
        use crate::{EntrypointType, SkillHints, SkillManifest, ToolPolicy};

        let entrypoint = EntrypointType::Prompted;

        // Parse allowed tools for tool policy
        let allowed_tools = self.parse_allowed_tools();
        let tool_policy = ToolPolicy {
            allow: allowed_tools,
            deny: vec![],
            required: vec![],
        };

        let description = self.frontmatter.description.clone().unwrap_or_default();

        // Extract hints from description (keywords for search)
        let description_words: Vec<String> = description
            .split_whitespace()
            .filter(|w| w.len() > 3)
            .take(10)
            .map(|s| s.to_lowercase().trim_end_matches('.').to_string())
            .collect();

        let hints = SkillHints {
            intent: description_words,
            domain: self
                .frontmatter
                .extra
                .get("tags")
                .and_then(serde_yaml::Value::as_sequence)
                .into_iter()
                .flatten()
                .filter_map(serde_yaml::Value::as_str)
                .map(str::to_string)
                .collect(),
            outcomes: vec![],
            expected_calls: None,
        };

        // Create input schema (generic object for Agent Skills)
        let inputs = serde_json::json!({
            "type": "object",
            "properties": {},
            "additionalProperties": true,
            "description": description
        });

        SkillManifest {
            id: self.frontmatter.name.clone(),
            title: self
                .frontmatter
                .name
                .split('-')
                .map(|w| {
                    let mut chars = w.chars();
                    chars.next().map_or_else(String::new, |first| {
                        first.to_uppercase().collect::<String>() + chars.as_str()
                    })
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

    /// Convert Agent Skill to `SkillContent` for progressive disclosure
    /// # Errors
    /// Returns an error if package resources cannot be inventoried safely or the
    /// frontmatter cannot be serialized to YAML.
    pub fn to_skill_content(&self) -> Result<crate::SkillContent> {
        use crate::SkillContent;

        let files = super::inventory_files(&self.path)?;
        let bundled_tools = super::SkillStore::discover_bundled_tools(&self.path, &files);
        let additional_files = super::resource_paths(&files, &bundled_tools);
        Ok(SkillContent {
            name: self.frontmatter.name.clone(),
            version: self.version(),
            description: self.frontmatter.description.clone().unwrap_or_default(),
            directory: self.path.clone(),
            frontmatter: serde_yaml::to_value(&self.frontmatter)?,
            skill_md: self.content.clone(),
            additional_files,
            bundled_tools,
            uses_tools: self.parse_allowed_tools(),
        })
    }
}

fn normalized_document(content: &str) -> std::borrow::Cow<'_, str> {
    if content.contains("\r\n") {
        std::borrow::Cow::Owned(content.replace("\r\n", "\n"))
    } else {
        std::borrow::Cow::Borrowed(content)
    }
}

fn frontmatter_parts(normalized: &str) -> Result<(&str, &str)> {
    let after_first = normalized.strip_prefix("---\n").ok_or_else(|| {
        AgentSkillsError::InvalidFormat(
            "SKILL.md must start with YAML frontmatter (---)".to_string(),
        )
    })?;
    after_first.split_once("\n---\n").ok_or_else(|| {
        AgentSkillsError::InvalidFormat(
            "SKILL.md frontmatter not properly closed with ---".to_string(),
        )
    })
}

/// Public wrapper to parse frontmatter for backward compatibility with old-format skills
/// # Errors
/// Returns an error for missing or malformed YAML frontmatter, required fields,
/// or values that violate the Agent Skills field constraints.
pub fn parse_frontmatter_public(content: &str) -> Result<(AgentSkillsFrontmatter, String)> {
    AgentSkill::parse_frontmatter(content)
}
