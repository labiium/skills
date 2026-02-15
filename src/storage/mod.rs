//! Skill Store
//!
//! Loads and manages Skills from the filesystem.
//! Skills are folder-based packages with:
//! - skill.json (manifest)
//! - SKILL.md (documentation)
//! - workflow.yaml (optional entrypoint)
//!
//! Also supports Agent Skills format (Vercel skills.sh compatible):
//! - SKILL.md with YAML frontmatter
//! - Optional scripts/, references/, assets/ directories
//!
//! Supports filesystem watching for hot-reload during development.

pub mod agent_skills;
pub mod search;
pub mod sync;

use crate::core::registry::Registry;
use crate::core::{
    BundledTool, CallableId, CallableKind, CallableRecord, CostHints, RiskTier, SchemaDigest,
};
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

#[derive(Error, Debug)]
pub enum SkillStoreError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Parse error: {0}")]
    Parse(String),

    #[error("Invalid skill manifest: {0}")]
    InvalidManifest(String),

    #[error("Skill not found: {0}")]
    NotFound(String),

    #[error("File not found: {0}")]
    FileNotFound(String),

    #[error("Skill already exists: {0}")]
    AlreadyExists(String),

    #[error("Validation error: {0}")]
    ValidationError(String),

    #[error("Circular dependency detected: {0}")]
    CircularDependency(String),

    #[error("Missing dependency: {0}")]
    MissingDependency(String),

    #[error("Agent Skills error: {0}")]
    AgentSkills(#[from] agent_skills::AgentSkillsError),
}

pub type Result<T> = std::result::Result<T, SkillStoreError>;

/// Skill manifest (skill.json)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillManifest {
    /// Unique skill identifier (slug)
    pub id: String,

    /// Human-readable title
    pub title: String,

    /// Semantic version
    pub version: String,

    /// Short description (1-2 lines)
    pub description: String,

    /// Input schema (JSON Schema)
    pub inputs: serde_json::Value,

    /// Output schema (JSON Schema, optional)
    #[serde(default)]
    pub outputs: Option<serde_json::Value>,

    /// Entrypoint type
    pub entrypoint: EntrypointType,

    /// Tool policy
    pub tool_policy: ToolPolicy,

    /// Hints for indexing and ranking
    #[serde(default)]
    pub hints: SkillHints,

    /// Risk tier override
    #[serde(default)]
    pub risk_tier: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EntrypointType {
    Workflow,
    Script,
    Prompted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolPolicy {
    /// Allowed tools (patterns, tags, or IDs)
    pub allow: Vec<String>,

    /// Denied tools
    #[serde(default)]
    pub deny: Vec<String>,

    /// Required capabilities
    #[serde(default)]
    pub required: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SkillHints {
    /// Intent keywords for ranking
    #[serde(default)]
    pub intent: Vec<String>,

    /// Domain tags
    #[serde(default)]
    pub domain: Vec<String>,

    /// Expected outcomes
    #[serde(default)]
    pub outcomes: Vec<String>,

    /// Expected number of tool calls
    #[serde(default)]
    pub expected_calls: Option<u32>,
}

/// Parsed skill from filesystem
#[derive(Debug, Clone)]
pub struct Skill {
    pub manifest: SkillManifest,
    pub path: PathBuf,
    pub documentation: Option<String>,
}

/// Skill content for progressive disclosure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillContent {
    pub skill_md: String,
    pub additional_files: Vec<String>,
    pub bundled_tools: Vec<BundledTool>,
    pub uses_tools: Vec<String>,
}

/// Request to create a new skill
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSkillRequest {
    pub name: String,
    pub version: String,
    pub description: String,
    pub skill_md_content: String,
    pub uses_tools: Vec<String>,
    pub bundled_files: Vec<(String, String)>, // (filename, content)
    pub tags: Vec<String>,
}

/// Skill validation result
#[derive(Debug, Clone)]
pub struct ValidationResult {
    pub valid: bool,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

impl ValidationResult {
    pub fn success() -> Self {
        ValidationResult {
            valid: true,
            errors: vec![],
            warnings: vec![],
        }
    }

    pub fn with_error(error: String) -> Self {
        ValidationResult {
            valid: false,
            errors: vec![error],
            warnings: vec![],
        }
    }

    pub fn add_error(&mut self, error: String) {
        self.errors.push(error);
        self.valid = false;
    }

    pub fn add_warning(&mut self, warning: String) {
        self.warnings.push(warning);
    }
}

/// Skill store manager
pub struct SkillStore {
    root: PathBuf,
    registry: Arc<Registry>,
    _watcher: Option<RecommendedWatcher>,
}

impl SkillStore {
    /// Create new skill store
    pub fn new(root: impl AsRef<Path>, registry: Arc<Registry>) -> Result<Self> {
        let root = root.as_ref().to_path_buf();

        if !root.exists() {
            warn!("Skill store root does not exist: {:?}", root);
            std::fs::create_dir_all(&root)?;
            info!("Created skill store root: {:?}", root);
        }

        Ok(SkillStore {
            root,
            registry,
            _watcher: None,
        })
    }

    /// Load all skills from disk
    pub async fn load_all(&self) -> Result<Vec<Skill>> {
        info!("Loading skills from: {:?}", self.root);

        let mut skills = Vec::new();

        if !self.root.exists() {
            warn!("Skill store root does not exist, skipping load");
            return Ok(skills);
        }

        let entries = std::fs::read_dir(&self.root)?;

        for entry in entries {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                match self.load_skill(&path).await {
                    Ok(skill) => {
                        debug!(
                            "Loaded skill: {} v{}",
                            skill.manifest.id, skill.manifest.version
                        );
                        skills.push(skill);
                    }
                    Err(e) => {
                        error!("Failed to load skill from {:?}: {}", path, e);
                    }
                }
            }
        }

        info!("Loaded {} skills", skills.len());
        Ok(skills)
    }

    /// Load skill content for progressive disclosure
    pub fn load_skill_content(&self, skill_id: &str) -> Result<SkillContent> {
        // Find the skill directory
        let skill_dir = self.find_skill_directory(skill_id)?;

        // Load SKILL.md
        let skill_md_path = skill_dir.join("SKILL.md");
        let skill_md = if skill_md_path.exists() {
            std::fs::read_to_string(&skill_md_path)?
        } else {
            return Err(SkillStoreError::FileNotFound(format!(
                "SKILL.md not found for skill: {}",
                skill_id
            )));
        };

        // Discover additional files (exclude skill.json, SKILL.md, and bundled scripts)
        let mut additional_files = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&skill_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                        if name != "skill.json"
                            && name != "SKILL.md"
                            && !name.ends_with(".py")
                            && !name.ends_with(".sh")
                            && !name.ends_with(".js")
                        {
                            additional_files.push(name.to_string());
                        }
                    }
                }
            }
        }

        // Discover bundled tools
        let bundled_tools = self.discover_bundled_tools(&skill_dir)?;

        // Load manifest to get uses_tools - try skill.json first, fallback to YAML frontmatter
        let manifest_path = skill_dir.join("skill.json");
        let uses_tools = if manifest_path.exists() {
            // New format: skill.json
            let manifest_content = std::fs::read_to_string(&manifest_path)?;
            let manifest: SkillManifest = serde_json::from_str(&manifest_content)
                .map_err(|e| SkillStoreError::Parse(e.to_string()))?;
            manifest.tool_policy.allow.clone()
        } else {
            // Old format: parse YAML frontmatter from SKILL.md
            match crate::agent_skills::parse_frontmatter_public(&skill_md) {
                Ok((frontmatter, _)) => {
                    // Parse allowed_tools (supports both string and array formats)
                    frontmatter
                        .allowed_tools
                        .map(|tools| tools.to_vec())
                        .unwrap_or_default()
                }
                Err(_) => Vec::new(), // If no frontmatter or parse error, return empty vec
            }
        };

        Ok(SkillContent {
            skill_md,
            additional_files,
            bundled_tools,
            uses_tools,
        })
    }

    /// Load a specific file from a skill directory
    ///
    /// Allows accessing files in subdirectories (e.g., "references/guide.md")
    /// while preventing path traversal attacks.
    pub fn load_skill_file(&self, skill_id: &str, filename: &str) -> Result<String> {
        // Reject absolute paths
        if filename.starts_with('/') || filename.starts_with('\\') {
            return Err(SkillStoreError::InvalidManifest(format!(
                "Absolute paths not allowed: {}",
                filename
            )));
        }

        // Reject path traversal attempts
        if filename.contains("..") {
            return Err(SkillStoreError::InvalidManifest(format!(
                "Path traversal not allowed: {}",
                filename
            )));
        }

        let skill_dir = self.find_skill_directory(skill_id)?;
        let file_path = skill_dir.join(filename);

        // Canonicalize and verify the path stays within skill directory
        let canonical_file = file_path.canonicalize().map_err(|_| {
            SkillStoreError::FileNotFound(format!(
                "File {} not found in skill {}",
                filename, skill_id
            ))
        })?;
        let canonical_skill = skill_dir.canonicalize().map_err(|_| {
            SkillStoreError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "Skill directory not found",
            ))
        })?;

        if !canonical_file.starts_with(&canonical_skill) {
            return Err(SkillStoreError::InvalidManifest(format!(
                "File {} is outside skill directory",
                filename
            )));
        }

        Ok(std::fs::read_to_string(&canonical_file)?)
    }

    /// Find the directory for a given skill ID
    fn find_skill_directory(&self, skill_id: &str) -> Result<PathBuf> {
        // Try direct match first
        let direct_path = self.root.join(skill_id);
        if direct_path.exists() && direct_path.is_dir() {
            return Ok(direct_path);
        }

        // Search all skill directories
        if let Ok(entries) = std::fs::read_dir(&self.root) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let manifest_path = path.join("skill.json");
                    if manifest_path.exists() {
                        if let Ok(content) = std::fs::read_to_string(&manifest_path) {
                            if let Ok(manifest) = serde_json::from_str::<SkillManifest>(&content) {
                                if manifest.id == skill_id {
                                    return Ok(path);
                                }
                            }
                        }
                    }
                }
            }
        }

        Err(SkillStoreError::NotFound(format!(
            "Skill not found: {}",
            skill_id
        )))
    }

    /// Discover bundled tools (scripts) in a skill directory
    fn discover_bundled_tools(&self, skill_dir: &Path) -> Result<Vec<BundledTool>> {
        let mut bundled_tools = Vec::new();

        if let Ok(entries) = std::fs::read_dir(skill_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                        let mut command = Vec::new();
                        let extension = path.extension().and_then(|e| e.to_str());

                        match extension {
                            Some("py") => {
                                command.push("python3".to_string());
                                command.push(path.to_string_lossy().to_string());
                            }
                            Some("sh") => {
                                command.push("bash".to_string());
                                command.push(path.to_string_lossy().to_string());
                            }
                            Some("js") => {
                                command.push("node".to_string());
                                command.push(path.to_string_lossy().to_string());
                            }
                            _ => continue,
                        }

                        // Try to load schema from companion .schema.json file
                        let schema_path = skill_dir.join(format!("{}.schema.json", name));
                        let schema = if schema_path.exists() {
                            std::fs::read_to_string(&schema_path)
                                .ok()
                                .and_then(|content| serde_json::from_str(&content).ok())
                                .unwrap_or_else(|| serde_json::json!({"type": "object"}))
                        } else {
                            serde_json::json!({"type": "object"})
                        };

                        bundled_tools.push(BundledTool {
                            name: name.to_string(),
                            description: format!("Bundled tool: {}", name),
                            command,
                            schema,
                        });
                    }
                }
            }
        }

        Ok(bundled_tools)
    }

    /// Load a single skill from a directory
    pub async fn load_skill(&self, path: &Path) -> Result<Skill> {
        let manifest_path = path.join("skill.json");
        let skill_md_path = path.join("SKILL.md");

        // Check if this is an Agent Skills format (has SKILL.md but no skill.json)
        if !manifest_path.exists() && skill_md_path.exists() {
            debug!("Detected Agent Skills format at {:?}", path);
            return self.load_agent_skill(path).await;
        }

        // Standard skills.rs format
        if !manifest_path.exists() {
            return Err(SkillStoreError::InvalidManifest(
                "Neither skill.json nor Agent Skills SKILL.md found".to_string(),
            ));
        }

        let manifest_content = std::fs::read_to_string(&manifest_path)?;
        let manifest: SkillManifest = serde_json::from_str(&manifest_content)
            .map_err(|e| SkillStoreError::Parse(e.to_string()))?;

        // Load documentation
        let doc_path = path.join("SKILL.md");
        let documentation = if doc_path.exists() {
            Some(std::fs::read_to_string(&doc_path)?)
        } else {
            None
        };

        Ok(Skill {
            manifest,
            path: path.to_path_buf(),
            documentation,
        })
    }

    /// Load a skill in Agent Skills format (Vercel skills.sh compatible)
    async fn load_agent_skill(&self, path: &Path) -> Result<Skill> {
        let agent_skill = agent_skills::AgentSkill::from_directory(path).await?;
        let manifest = agent_skill.to_skill_manifest();
        let documentation = Some(agent_skill.content.clone());

        info!(
            "Loaded Agent Skill: {} v{} from {:?}",
            manifest.id, manifest.version, path
        );

        Ok(Skill {
            manifest,
            path: path.to_path_buf(),
            documentation,
        })
    }

    /// Register a skill in the registry
    pub fn register_skill(&self, skill: &Skill) -> Result<CallableId> {
        let manifest = &skill.manifest;

        // Compute schema digest
        let digest = SchemaDigest::from_schema(&manifest.inputs)
            .map_err(|e| SkillStoreError::Parse(e.to_string()))?;

        // Create callable ID
        let id = CallableId::skill(&manifest.id, &manifest.version);

        // Determine risk tier
        let risk_tier = manifest
            .risk_tier
            .as_ref()
            .and_then(|s| s.parse().ok())
            .unwrap_or(RiskTier::Unknown);

        // Build tags from hints
        let mut tags = Vec::new();
        tags.extend(manifest.hints.intent.iter().cloned());
        tags.extend(manifest.hints.domain.iter().cloned());
        tags.extend(manifest.hints.outcomes.iter().cloned());
        tags.push("skill".to_string());

        // Create callable record
        // Discover bundled tools
        let bundled_tools = self.discover_bundled_tools(&skill.path).unwrap_or_default();

        // Discover additional files
        let mut additional_files = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&skill.path) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                        if name != "skill.json"
                            && name != "SKILL.md"
                            && !name.ends_with(".py")
                            && !name.ends_with(".sh")
                            && !name.ends_with(".js")
                            && !name.ends_with(".schema.json")
                        {
                            additional_files.push(name.to_string());
                        }
                    }
                }
            }
        }

        let record = CallableRecord {
            id: id.clone(),
            kind: CallableKind::Skill,
            fq_name: format!("skill.{}", manifest.id),
            name: manifest.id.clone(),
            title: Some(manifest.title.clone()),
            description: Some(manifest.description.clone()),
            tags,
            input_schema: manifest.inputs.clone(),
            output_schema: manifest.outputs.clone(),
            schema_digest: digest,
            server_alias: None,
            upstream_tool_name: None,
            skill_version: Some(manifest.version.clone()),
            uses: Vec::new(), // Note: uses_tools field is stored in manifest; full dependency resolution deferred to future enhancement
            skill_directory: Some(skill.path.clone()),
            bundled_tools: bundled_tools.clone(),
            additional_files,
            cost_hints: CostHints {
                expected_calls: manifest.hints.expected_calls,
                estimated_duration_ms: None,
                network_required: false,
                filesystem_access: false,
            },
            risk_tier,
            last_seen: chrono::Utc::now(),
        };

        self.registry
            .register(record)
            .map_err(|e| SkillStoreError::Parse(e.to_string()))?;

        info!("Registered skill: {} ({})", manifest.id, id.as_str());
        Ok(id)
    }

    /// Load and register all skills
    pub async fn load_and_register_all(&self) -> Result<Vec<CallableId>> {
        let skills = self.load_all().await?;
        let mut ids = Vec::new();

        for skill in &skills {
            match self.register_skill(skill) {
                Ok(id) => ids.push(id),
                Err(e) => error!("Failed to register skill {}: {}", skill.manifest.id, e),
            }
        }

        info!("Registered {} skills", ids.len());
        Ok(ids)
    }

    /// Start filesystem watcher for hot-reload
    pub fn start_watch(&mut self) -> Result<()> {
        let (tx, mut rx) = mpsc::channel(100);
        let root = self.root.clone();
        let _registry = self.registry.clone();

        let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
            if let Ok(event) = res {
                let _ = tx.blocking_send(event);
            }
        })
        .map_err(|e| SkillStoreError::Parse(format!("Failed to create watcher: {}", e)))?;

        watcher
            .watch(&self.root, RecursiveMode::Recursive)
            .map_err(|e| SkillStoreError::Parse(format!("Failed to watch directory: {}", e)))?;

        self._watcher = Some(watcher);

        // Spawn background task to handle events
        tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                debug!("Filesystem event: {:?}", event);
                // Note: Hot-reload of skills not yet implemented
                // Current behavior: filesystem events are logged only
            }
        });

        info!("Started filesystem watcher on: {:?}", root);
        Ok(())
    }

    /// Create a new skill
    pub async fn create_skill(&self, request: CreateSkillRequest) -> Result<CallableId> {
        // Validate skill name
        self.validate_skill_name(&request.name)?;

        // Check if skill already exists
        let skill_dir = self.root.join(&request.name);
        if skill_dir.exists() {
            return Err(SkillStoreError::AlreadyExists(request.name.clone()));
        }

        // Create skill directory
        std::fs::create_dir_all(&skill_dir)?;

        // Create skill.json manifest
        let manifest = SkillManifest {
            id: request.name.clone(),
            title: request.name.clone(),
            version: request.version.clone(),
            description: request.description.clone(),
            inputs: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
            outputs: None,
            entrypoint: EntrypointType::Prompted,
            tool_policy: ToolPolicy {
                allow: request.uses_tools.clone(),
                deny: vec![],
                required: vec![],
            },
            hints: SkillHints {
                intent: vec![],
                domain: request.tags.clone(),
                outcomes: vec![],
                expected_calls: None,
            },
            risk_tier: Some("read_only".to_string()),
        };

        let manifest_json = serde_json::to_string_pretty(&manifest)
            .map_err(|e| SkillStoreError::Parse(e.to_string()))?;
        std::fs::write(skill_dir.join("skill.json"), manifest_json)?;

        // Write SKILL.md
        std::fs::write(skill_dir.join("SKILL.md"), &request.skill_md_content)?;

        // Write bundled files
        for (filename, content) in &request.bundled_files {
            // Validate filename
            if filename.contains("..") || filename.contains('/') || filename.contains('\\') {
                return Err(SkillStoreError::ValidationError(format!(
                    "Invalid filename: {}",
                    filename
                )));
            }
            std::fs::write(skill_dir.join(filename), content)?;
        }

        info!("Created skill: {} at {:?}", request.name, skill_dir);

        // Load and register the skill
        let skill = match self.load_skill(&skill_dir).await {
            Ok(skill) => skill,
            Err(e) => {
                // Clean up on error
                let _ = std::fs::remove_dir_all(&skill_dir);
                return Err(e);
            }
        };

        let id = self.register_skill(&skill)?;

        info!("Skill {} created and registered successfully", request.name);
        Ok(id)
    }

    /// Update an existing skill
    pub async fn update_skill(
        &self,
        skill_id: &str,
        request: CreateSkillRequest,
    ) -> Result<CallableId> {
        // Find existing skill directory
        let skill_dir = self.find_skill_directory(skill_id)?;

        // Update skill.json manifest
        let manifest = SkillManifest {
            id: request.name.clone(),
            title: request.name.clone(),
            version: request.version.clone(),
            description: request.description.clone(),
            inputs: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
            outputs: None,
            entrypoint: EntrypointType::Prompted,
            tool_policy: ToolPolicy {
                allow: request.uses_tools.clone(),
                deny: vec![],
                required: vec![],
            },
            hints: SkillHints {
                intent: vec![],
                domain: request.tags.clone(),
                outcomes: vec![],
                expected_calls: None,
            },
            risk_tier: Some("read_only".to_string()),
        };

        let manifest_json = serde_json::to_string_pretty(&manifest)
            .map_err(|e| SkillStoreError::Parse(e.to_string()))?;
        std::fs::write(skill_dir.join("skill.json"), manifest_json)?;

        // Update SKILL.md
        std::fs::write(skill_dir.join("SKILL.md"), &request.skill_md_content)?;

        // Update bundled files
        for (filename, content) in &request.bundled_files {
            // Validate filename
            if filename.contains("..") || filename.contains('/') || filename.contains('\\') {
                return Err(SkillStoreError::ValidationError(format!(
                    "Invalid filename: {}",
                    filename
                )));
            }
            std::fs::write(skill_dir.join(filename), content)?;
        }

        info!("Updated skill: {} at {:?}", skill_id, skill_dir);

        // Reload and re-register the skill
        let skill = self.load_skill(&skill_dir).await?;
        let id = self.register_skill(&skill)?;

        info!("Skill {} updated and re-registered successfully", skill_id);
        Ok(id)
    }

    /// Delete a skill
    pub fn delete_skill(&self, skill_id: &str) -> Result<()> {
        // Find skill directory
        let skill_dir = self.find_skill_directory(skill_id)?;

        // Find and remove from registry - need to find by name since version is part of ID
        // Search registry for skills matching this name
        let skills = self.registry.by_kind(crate::core::CallableKind::Skill);
        for skill in skills {
            if let Some(name) = skill.id.skill_name() {
                if name == skill_id {
                    self.registry.remove(&skill.id);
                    info!("Removed skill from registry: {}", skill.id.as_str());
                    break;
                }
            }
        }

        // Delete directory
        std::fs::remove_dir_all(&skill_dir)?;

        info!("Deleted skill: {} from {:?}", skill_id, skill_dir);
        Ok(())
    }

    /// Validate skill name
    fn validate_skill_name(&self, name: &str) -> Result<()> {
        if name.is_empty() {
            return Err(SkillStoreError::ValidationError(
                "Skill name cannot be empty".to_string(),
            ));
        }

        if name.contains('/') || name.contains('\\') || name.contains("..") {
            return Err(SkillStoreError::ValidationError(format!(
                "Invalid skill name: {}",
                name
            )));
        }

        if name.len() > 100 {
            return Err(SkillStoreError::ValidationError(
                "Skill name too long (max 100 characters)".to_string(),
            ));
        }

        Ok(())
    }

    /// Validate a skill manifest and content
    pub fn validate_skill(&self, skill: &Skill) -> ValidationResult {
        let mut result = ValidationResult::success();

        // Validate ID format
        if !Self::is_valid_skill_id(&skill.manifest.id) {
            result.add_error(format!(
                "Invalid skill ID '{}': must be lowercase alphanumeric with hyphens",
                skill.manifest.id
            ));
        }

        // Validate version (semantic versioning)
        if let Err(e) = semver::Version::parse(&skill.manifest.version) {
            result.add_error(format!(
                "Invalid version '{}': {}",
                skill.manifest.version, e
            ));
        }

        // Validate description length
        if skill.manifest.description.is_empty() {
            result.add_error("Description cannot be empty".to_string());
        } else if skill.manifest.description.len() > 500 {
            result.add_warning("Description is very long (>500 chars)".to_string());
        }

        // Validate input schema
        if let Err(e) = self.validate_json_schema(&skill.manifest.inputs) {
            result.add_error(format!("Invalid input schema: {}", e));
        }

        // Validate output schema if present
        if let Some(ref outputs) = skill.manifest.outputs {
            if let Err(e) = self.validate_json_schema(outputs) {
                result.add_error(format!("Invalid output schema: {}", e));
            }
        }

        // Validate tool dependencies
        for tool in &skill.manifest.tool_policy.allow {
            if tool != "*" && !tool.contains('*') {
                // Check if tool exists in registry (warning only)
                let found = self
                    .registry
                    .all()
                    .iter()
                    .any(|r| r.name == *tool || r.fq_name == *tool || r.id.as_str() == *tool);
                if !found {
                    result.add_warning(format!(
                        "Tool '{}' not found in registry (may be loaded later)",
                        tool
                    ));
                }
            }
        }

        // Check for circular dependencies (if skill references itself)
        if skill
            .manifest
            .tool_policy
            .allow
            .contains(&skill.manifest.id)
        {
            result.add_error(format!(
                "Circular dependency: skill '{}' references itself",
                skill.manifest.id
            ));
        }

        // Validate risk tier
        if let Some(ref tier) = skill.manifest.risk_tier {
            if tier.parse::<crate::core::RiskTier>().is_err() {
                result.add_error(format!("Invalid risk tier: {}", tier));
            }
        }

        // Validate hints
        if let Some(calls) = skill.manifest.hints.expected_calls {
            if calls == 0 {
                result.add_warning("Expected calls is 0".to_string());
            } else if calls > 100 {
                result.add_warning(format!("Expected calls is very high: {}", calls));
            }
        }

        // Validate SKILL.md exists
        if skill.documentation.is_none() {
            result.add_warning("No SKILL.md documentation found".to_string());
        } else if let Some(ref doc) = skill.documentation {
            if doc.trim().is_empty() {
                result.add_warning("SKILL.md is empty".to_string());
            } else if doc.len() < 50 {
                result.add_warning("SKILL.md is very short (<50 chars)".to_string());
            }
        }

        // Validate bundled scripts exist and are executable
        let script_extensions = ["py", "sh", "js"];
        for entry in std::fs::read_dir(&skill.path)
            .unwrap_or_else(|_| {
                result.add_error(format!("Cannot read skill directory: {:?}", skill.path));
                std::fs::read_dir(".").unwrap() // Dummy to satisfy type
            })
            .flatten()
        {
            let path = entry.path();
            if path.is_file() {
                if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                    if script_extensions.contains(&ext) {
                        // Check if script is readable
                        if std::fs::read(&path).is_err() {
                            result.add_error(format!(
                                "Cannot read bundled script: {:?}",
                                path.file_name()
                            ));
                        }
                    }
                }
            }
        }

        result
    }

    /// Validate JSON Schema
    fn validate_json_schema(&self, schema: &serde_json::Value) -> Result<()> {
        // Basic JSON Schema validation
        if !schema.is_object() {
            return Err(SkillStoreError::ValidationError(
                "Schema must be an object".to_string(),
            ));
        }

        let obj = schema.as_object().unwrap();

        // Must have "type" field
        if !obj.contains_key("type") {
            return Err(SkillStoreError::ValidationError(
                "Schema must have 'type' field".to_string(),
            ));
        }

        // Validate type value
        if let Some(type_val) = obj.get("type") {
            if let Some(type_str) = type_val.as_str() {
                let valid_types = [
                    "object", "array", "string", "number", "integer", "boolean", "null",
                ];
                if !valid_types.contains(&type_str) {
                    return Err(SkillStoreError::ValidationError(format!(
                        "Invalid schema type: {}",
                        type_str
                    )));
                }
            }
        }

        Ok(())
    }

    /// Check if skill ID is valid format
    fn is_valid_skill_id(id: &str) -> bool {
        if id.is_empty() || id.len() > 100 {
            return false;
        }

        // Must start with letter
        if !id.chars().next().unwrap().is_ascii_lowercase() {
            return false;
        }

        // Only lowercase alphanumeric and hyphens
        id.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    }

    /// Detect circular dependencies in skills
    pub fn detect_circular_dependencies(&self, skill_id: &str) -> Result<Vec<String>> {
        let mut visited = std::collections::HashSet::new();
        let mut stack = vec![skill_id.to_string()];
        let mut path = vec![];

        while let Some(current) = stack.pop() {
            if visited.contains(&current) {
                // Found a cycle
                if let Some(pos) = path.iter().position(|s| s == &current) {
                    return Ok(path[pos..].to_vec());
                }
            }

            visited.insert(current.clone());
            path.push(current.clone());

            // Get dependencies for this skill
            if let Ok(content) = self.load_skill_content(&current) {
                for tool in &content.uses_tools {
                    // Check if this is a skill reference
                    if self.find_skill_directory(tool).is_ok() {
                        stack.push(tool.clone());
                    }
                }
            }
        }

        Ok(vec![]) // No cycle found
    }
}
