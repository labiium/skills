//! Skill Store
//!
//! Loads and manages Skills from the filesystem.
//! Skills are folder-based packages with SKILL.md (documentation with YAML frontmatter).
//!
//! Directory structure:
//! - SKILL.md          # Instructions with YAML frontmatter (required)
//! - scripts/          # Executable scripts (optional)
//! - references/       # Reference documentation (optional)
//! - assets/           # Binary assets (optional)
//!
//! Supports filesystem watching for hot-reload during development.

pub mod agent_skills;
pub mod search;
pub mod sync;

use crate::core::registry::Registry;
use crate::core::{
    BundledTool, CallableId, CallableKind, CallableRecord, CostHints, RiskTier, SchemaDigest,
};
use crate::storage::search::SearchEngine;
use jiff::Timestamp;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
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

/// Skill manifest (derived from SKILL.md YAML frontmatter)
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
    pub name: String,
    pub version: String,
    pub description: String,
    pub directory: PathBuf,
    pub frontmatter: serde_yaml::Value,
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
    /// Scripts to be placed in scripts/ subdirectory
    pub scripts: Vec<(String, String)>, // (filename, content)
    /// Reference files to be placed in references/ subdirectory
    pub references: Vec<(String, String)>, // (filename, content)
    /// Asset files to be placed in assets/ subdirectory
    pub assets: Vec<(String, String)>, // (filename, content)
    pub tags: Vec<String>,
}

/// Omitted fields preserve the installed skill; supplied empty values are explicit edits.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UpdateSkillRequest {
    pub name: Option<String>,
    pub version: Option<String>,
    pub description: Option<String>,
    pub skill_md_content: Option<String>,
    pub uses_tools: Option<Vec<String>>,
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub scripts: Vec<(String, String)>,
    #[serde(default)]
    pub references: Vec<(String, String)>,
    #[serde(default)]
    pub assets: Vec<(String, String)>,
}

/// Skill validation result
#[derive(Debug, Clone)]
pub struct ValidationResult {
    pub valid: bool,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

impl ValidationResult {
    #[must_use]
    pub const fn success() -> Self {
        Self {
            valid: true,
            errors: vec![],
            warnings: vec![],
        }
    }

    #[must_use]
    pub fn with_error(error: String) -> Self {
        Self {
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
    read_roots: Vec<PathBuf>,
    registry: Arc<Registry>,
    search_engine: Option<Arc<SearchEngine>>,
    watcher: Option<RecommendedWatcher>,
    reload_tx: Option<tokio::sync::mpsc::Sender<PathBuf>>,
}

impl SkillStore {
    fn document_parts(content: &str) -> Result<(serde_yaml::Mapping, String)> {
        let normalized = content.replace("\r\n", "\n");
        if let Some(after_first) = normalized.strip_prefix("---\n") {
            let (yaml, body) = after_first
                .split_once("\n---\n")
                .ok_or_else(|| SkillStoreError::Parse("Unclosed YAML frontmatter".to_string()))?;
            let mapping = serde_yaml::from_str(yaml)
                .map_err(|error| SkillStoreError::Parse(error.to_string()))?;
            Ok((mapping, body.trim().to_string()))
        } else {
            Ok((serde_yaml::Mapping::new(), content.trim().to_string()))
        }
    }

    fn serialize_document(frontmatter: &serde_yaml::Mapping, body: &str) -> Result<String> {
        let yaml = serde_yaml::to_string(frontmatter)
            .map_err(|error| SkillStoreError::Parse(error.to_string()))?;
        let document = format!(
            "---\n{}---\n\n{}",
            yaml.trim_start_matches("---\n"),
            body.trim()
        );
        agent_skills::parse_frontmatter_public(&document)?;
        Ok(document)
    }

    fn set_field<T: Serialize>(
        mapping: &mut serde_yaml::Mapping,
        key: &str,
        value: T,
    ) -> Result<()> {
        mapping.insert(
            serde_yaml::Value::String(key.to_string()),
            serde_yaml::to_value(value)
                .map_err(|error| SkillStoreError::Parse(error.to_string()))?,
        );
        Ok(())
    }

    fn validate_filenames(files: &[(String, String)], kind: &str) -> Result<()> {
        for (filename, _) in files {
            if filename.is_empty()
                || !Path::new(filename)
                    .components()
                    .all(|part| matches!(part, std::path::Component::Normal(_)))
                || filename.contains('\\')
            {
                return Err(SkillStoreError::ValidationError(format!(
                    "Invalid {kind} filename: {filename}"
                )));
            }
        }
        Ok(())
    }

    fn build_skill_md_content(request: &CreateSkillRequest) -> Result<String> {
        let (mut frontmatter, body) = Self::document_parts(&request.skill_md_content)?;
        Self::set_field(&mut frontmatter, "name", &request.name)?;
        Self::set_field(&mut frontmatter, "description", &request.description)?;
        Self::set_field(&mut frontmatter, "version", &request.version)?;
        if !request.uses_tools.is_empty() {
            Self::set_field(&mut frontmatter, "allowed-tools", &request.uses_tools)?;
        }
        if !request.tags.is_empty() {
            Self::set_field(&mut frontmatter, "tags", &request.tags)?;
        }
        Self::serialize_document(&frontmatter, &body)
    }

    /// Create new skill store
    /// # Errors
    /// Returns an error if the root cannot be created or canonicalized.
    pub fn new(root: impl AsRef<Path>, registry: Arc<Registry>) -> Result<Self> {
        let root = root.as_ref().to_path_buf();

        if !root.exists() {
            warn!("Skill store root does not exist: {:?}", root);
            std::fs::create_dir_all(&root)?;
            info!("Created skill store root: {:?}", root);
        }

        Ok(Self {
            root: root.canonicalize()?,
            read_roots: Vec::new(),
            registry,
            search_engine: None,
            watcher: None,
            reload_tx: None,
        })
    }

    /// Create new skill store with search engine for automatic index updates
    /// # Errors
    /// Returns an error if the root cannot be created or canonicalized.
    pub fn with_search_engine(
        root: impl AsRef<Path>,
        registry: Arc<Registry>,
        search_engine: Arc<SearchEngine>,
    ) -> Result<Self> {
        let mut store = Self::new(root, registry)?;
        store.search_engine = Some(search_engine);
        Ok(store)
    }

    /// Add an existing read-only package or collection after all higher-precedence roots.
    pub fn add_read_root(&mut self, root: PathBuf) {
        let root = root.canonicalize().unwrap_or(root);
        if root != self.root && !self.read_roots.contains(&root) {
            self.read_roots.push(root);
        }
    }

    fn roots(&self) -> impl Iterator<Item = &PathBuf> {
        std::iter::once(&self.root).chain(&self.read_roots)
    }

    fn package_directories(&self) -> (Vec<PathBuf>, Vec<String>) {
        Self::package_directories_from(self.roots())
    }

    fn package_directories_from<'a>(
        roots: impl Iterator<Item = &'a PathBuf>,
    ) -> (Vec<PathBuf>, Vec<String>) {
        let mut directories = Vec::new();
        let mut errors = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for root in roots {
            if !root.exists() {
                continue;
            }
            let mut pending = vec![root.clone()];
            while let Some(path) = pending.pop() {
                let canonical = match path.canonicalize() {
                    Ok(canonical) => canonical,
                    Err(error) => {
                        errors.push(format!("{}: {error}", path.display()));
                        continue;
                    }
                };
                if !canonical.starts_with(root) || !seen.insert(canonical.clone()) {
                    continue;
                }
                if path.join("SKILL.md").exists() {
                    directories.push(canonical);
                    continue;
                }
                let entries = match sorted_entries(&path) {
                    Ok(entries) => entries,
                    Err(error) => {
                        errors.push(format!("{}: {error}", path.display()));
                        continue;
                    }
                };
                let mut children = entries
                    .into_iter()
                    .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
                    .filter(|entry| !excluded_directory(&entry.file_name()))
                    .map(|entry| entry.path())
                    .collect::<Vec<_>>();
                if children.is_empty() && path != *root {
                    directories.push(canonical);
                } else {
                    children.reverse();
                    pending.extend(children);
                }
            }
        }
        (directories, errors)
    }
    /// Load all skills from disk
    /// # Errors
    /// Returns an error if the operation cannot be completed.
    pub async fn load_all(&self) -> Result<Vec<Skill>> {
        let mut skills = Vec::new();
        let mut names = std::collections::HashSet::new();
        let (paths, errors) = self.package_directories();
        for error in errors {
            error!("Failed to inspect skill root: {error}");
        }
        for path in paths {
            match self.load_skill(&path).await {
                Ok(skill) if names.insert(skill.manifest.id.clone()) => skills.push(skill),
                Ok(_) => {}
                Err(error) => error!("Failed to load skill from {:?}: {}", path, error),
            }
        }
        Ok(skills)
    }

    /// Load skill content for progressive disclosure
    /// # Errors
    /// Returns an error if the skill is missing, its document is invalid, or its resources
    /// cannot be read safely within the package.
    pub fn load_skill_content(&self, skill_id: &str) -> Result<SkillContent> {
        let skill_dir = self.find_skill_directory(skill_id)?;
        let skill_document = self.load_skill_file(skill_id, "SKILL.md")?;
        let (frontmatter, _) = agent_skills::parse_frontmatter_public(&skill_document)?;
        let files = inventory_files(&skill_dir)?;
        let bundled_tools = Self::discover_bundled_tools(&skill_dir, &files);
        let additional_files = resource_paths(&files, &bundled_tools);
        let uses_tools = frontmatter
            .allowed_tools
            .as_ref()
            .map(agent_skills::AllowedTools::to_vec)
            .unwrap_or_default();
        Ok(SkillContent {
            name: frontmatter.name.clone(),
            version: frontmatter.version.clone(),
            description: frontmatter.description.clone().unwrap_or_default(),
            directory: skill_dir,
            frontmatter: serde_yaml::to_value(&frontmatter)
                .map_err(|error| SkillStoreError::Parse(error.to_string()))?,
            skill_md: skill_document,
            additional_files,
            bundled_tools,
            uses_tools,
        })
    }

    /// Load a specific file from a skill directory
    ///
    /// Allows accessing files in subdirectories (e.g., "references/guide.md")
    /// while preventing path traversal attacks.
    /// # Errors
    /// Returns an error if the skill or file is missing, the path escapes its package,
    /// the file cannot be read, or its contents are not UTF-8.
    pub fn load_skill_file(&self, skill_id: &str, filename: &str) -> Result<String> {
        String::from_utf8(self.load_skill_file_bytes(skill_id, filename)?)
            .map_err(|error| SkillStoreError::Parse(error.to_string()))
    }

    /// Read a resource without assuming UTF-8 encoding.
    /// # Errors
    /// Returns an error if the skill or file is missing, the path escapes its package,
    /// or the file cannot be read.
    pub fn load_skill_file_bytes(&self, skill_id: &str, filename: &str) -> Result<Vec<u8>> {
        let skill_dir = self.find_skill_directory(skill_id)?;
        let file = contained_file(&skill_dir, filename)?;
        Ok(std::fs::read(file)?)
    }

    /// Find the directory for a given skill ID
    fn find_skill_directory(&self, skill_id: &str) -> Result<PathBuf> {
        Self::validate_skill_name(skill_id)?;
        let (paths, errors) = self.package_directories();
        for error in errors {
            warn!("Failed to inspect skill root: {error}");
        }
        for path in paths {
            let Ok(document) = contained_file(&path, "SKILL.md").and_then(std::fs::read_to_string)
            else {
                continue;
            };
            if let Ok((frontmatter, _)) = agent_skills::parse_frontmatter_public(&document) {
                if frontmatter.name == skill_id {
                    return Ok(path);
                }
            }
        }
        Err(SkillStoreError::NotFound(skill_id.to_string()))
    }

    fn discover_bundled_tools(skill_dir: &Path, files: &[PathBuf]) -> Vec<BundledTool> {
        files
            .iter()
            .filter_map(|relative| {
                let name = relative.to_str()?;
                let extension = relative
                    .extension()
                    .and_then(|extension| extension.to_str());
                let interpreter = match extension {
                    Some("py") => Some("python3"),
                    Some("sh") => Some("bash"),
                    Some("js" | "mjs" | "cjs") => Some("node"),
                    _ => None,
                };
                let path = skill_dir.join(relative);
                #[cfg(unix)]
                let executable = interpreter.is_none() && {
                    use std::os::unix::fs::PermissionsExt;
                    path.metadata()
                        .is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0)
                };
                #[cfg(not(unix))]
                let executable =
                    extension.is_some_and(|extension| matches!(extension, "exe" | "bat" | "cmd"));
                if interpreter.is_none() && (!relative.starts_with("scripts") || !executable) {
                    return None;
                }
                let mut command = Vec::new();
                if let Some(interpreter) = interpreter {
                    command.push(interpreter.to_string());
                }
                command.push(path.to_string_lossy().into_owned());
                let schema_name = format!("{name}.schema.json");
                let schema = contained_file(skill_dir, &schema_name)
                    .ok()
                    .and_then(|path| std::fs::read(path).ok())
                    .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                    .unwrap_or_else(|| serde_json::json!({"type": "object"}));
                Some(BundledTool {
                    name: name.to_string(),
                    description: format!("Bundled script: {name}"),
                    command,
                    schema,
                })
            })
            .collect()
    }

    /// Load a single skill from a directory.
    ///
    /// Supports only Agent Skills format (SKILL.md with YAML frontmatter).
    /// # Errors
    /// Returns an error if the directory is outside configured roots, the document is
    /// missing or invalid, or package files cannot be read safely.
    pub async fn load_skill(&self, path: &Path) -> Result<Skill> {
        let canonical = self.skill_directory(path)?;
        let document = tokio::fs::read_to_string(contained_file(&canonical, "SKILL.md")?).await?;
        Self::skill_from_document(&canonical, &document)
    }

    fn load_skill_directory(&self, path: &Path) -> Result<Skill> {
        Self::load_agent_skill(&self.skill_directory(path)?)
    }

    fn skill_directory(&self, path: &Path) -> Result<PathBuf> {
        let canonical = path.canonicalize()?;
        if !self.roots().any(|root| canonical.starts_with(root)) {
            return Err(SkillStoreError::InvalidManifest(
                "Skill is outside configured roots".to_string(),
            ));
        }
        let skill_md_path = path.join("SKILL.md");

        // Only support Agent Skills format
        if skill_md_path.exists() {
            debug!("Loading Agent Skills format at {:?}", path);
            return Ok(canonical);
        }

        // SKILL.md is required
        Err(SkillStoreError::InvalidManifest(
            "SKILL.md not found. Skills must use Agent Skills format with YAML frontmatter."
                .to_string(),
        ))
    }

    /// Load a skill in Agent Skills format (Vercel skills.sh compatible)
    fn load_agent_skill(path: &Path) -> Result<Skill> {
        let document = std::fs::read_to_string(contained_file(path, "SKILL.md")?)?;
        Self::skill_from_document(path, &document)
    }

    fn skill_from_document(path: &Path, document: &str) -> Result<Skill> {
        let (frontmatter, content) = agent_skills::parse_frontmatter_public(document)?;
        let files = inventory_files(path)?;
        let in_directory = |directory: &str| {
            files
                .iter()
                .filter(|file| file.starts_with(directory))
                .map(|file| path.join(file))
                .collect()
        };
        let agent_skill = agent_skills::AgentSkill {
            frontmatter,
            content,
            path: path.to_path_buf(),
            scripts: in_directory("scripts"),
            references: in_directory("references"),
            assets: in_directory("assets"),
        };
        if path.file_name().and_then(|name| name.to_str())
            != Some(agent_skill.frontmatter.name.as_str())
        {
            warn!(
                "Skill '{}' does not match directory {}",
                agent_skill.frontmatter.name,
                path.display()
            );
        }
        let manifest = agent_skill.to_skill_manifest();
        let documentation = Some(agent_skill.content);

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
    /// # Errors
    /// Returns an error if the operation cannot be completed.
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

        let files = inventory_files(&skill.path)?;
        let bundled_tools = Self::discover_bundled_tools(&skill.path, &files);
        let additional_files = resource_paths(&files, &bundled_tools);

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
            bundled_tools,
            additional_files,
            cost_hints: CostHints {
                expected_calls: manifest.hints.expected_calls,
                estimated_duration_ms: None,
                network_required: false,
                filesystem_access: false,
            },
            risk_tier,
            last_seen: Timestamp::now(),
            sandbox_config: None,
        };

        self.registry
            .register(record.clone())
            .map_err(|e| SkillStoreError::Parse(e.to_string()))?;

        // Update search index if search engine is configured
        if let Some(ref search_engine) = self.search_engine {
            search_engine.update_record(&record);
            debug!("Updated search index for skill: {}", manifest.id);
        }

        info!("Registered skill: {} ({})", manifest.id, id.as_str());
        Ok(id)
    }

    /// Load and register all skills
    /// # Errors
    /// Returns an error if the operation cannot be completed.
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
    /// Returns a receiver channel that receives paths to skills that need reloading
    /// # Errors
    /// Returns an error if the operation cannot be completed.
    pub fn start_watch(&mut self) -> Result<tokio::sync::mpsc::Receiver<PathBuf>> {
        let (reload_tx, reload_rx) = tokio::sync::mpsc::channel(100);
        let (event_tx, mut event_rx) = mpsc::unbounded_channel();
        let root = self.root.clone();

        let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
            if let Ok(event) = res {
                // Reads generate access events; reloading them would create a feedback loop.
                if matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)) {
                    let _ = event_tx.send(event);
                }
            }
        })
        .map_err(|e| SkillStoreError::Parse(format!("Failed to create watcher: {e}")))?;

        for root in self.roots().filter(|root| root.exists()) {
            watcher
                .watch(root, RecursiveMode::Recursive)
                .map_err(|e| SkillStoreError::Parse(format!("Failed to watch directory: {e}")))?;
        }

        self.watcher = Some(watcher);
        self.reload_tx = Some(reload_tx.clone());
        // Preserve both paths of directory renames as well as changes within packages.
        tokio::spawn(async move {
            while let Some(event) = event_rx.recv().await {
                for path in &event.paths {
                    let directory = if path.is_file()
                        || path.file_name().is_some_and(|name| name == "SKILL.md")
                    {
                        path.parent().unwrap_or(path)
                    } else {
                        path
                    };
                    let _ = reload_tx.send(directory.to_path_buf()).await;
                }
            }
        });


        info!("Started filesystem watcher on: {:?}", root);
        Ok(reload_rx)
    }

    /// Reconcile packages beneath a changed or deleted directory against every root.
    ///
    /// Directory renames must refresh both the previous and destination paths. Existing
    /// registry paths retain previous frontmatter names, so renames and version edits
    /// remove stale callable IDs without reloading unrelated packages.
    /// # Errors
    /// Returns an error for paths outside the configured roots or unreadable winning packages.
    pub fn refresh_directory(&self, path: &Path) -> Result<()> {
        let mut scope = canonical_directory(path)?;
        if !self.roots().any(|root| scope.starts_with(root)) {
            return Err(SkillStoreError::InvalidManifest(
                "Skill is outside configured roots".to_string(),
            ));
        }
        if !self.roots().any(|root| {
            scope.strip_prefix(root).is_ok_and(|relative| {
                !relative
                    .components()
                    .any(|part| excluded_directory(part.as_os_str()))
            })
        }) {
            return Ok(());
        }
        let mut package_scope = scope.clone();
        for ancestor in scope.ancestors() {
            if !self.roots().any(|root| ancestor.starts_with(root)) {
                break;
            }
            if ancestor.join("SKILL.md").is_file() {
                package_scope = ancestor.to_path_buf();
            }
        }
        scope = package_scope;
        let registered = self.registry.by_kind(CallableKind::Skill);
        for record in &registered {
            if let Some(directory) = &record.skill_directory {
                if scope.starts_with(directory) {
                    scope.clone_from(directory);
                }
            }
        }
        let mut names = registered
            .into_iter()
            .filter(|record| {
                record
                    .skill_directory
                    .as_ref()
                    .is_some_and(|directory| directory.starts_with(&scope))
            })
            .map(|record| record.name)
            .collect::<std::collections::BTreeSet<_>>();
        let (directories, errors) = Self::package_directories_from(std::iter::once(&scope));
        for error in errors {
            warn!("Failed to inspect changed skill directory: {error}");
        }
        for directory in directories {
            let Ok(document) =
                contained_file(&directory, "SKILL.md").and_then(std::fs::read_to_string)
            else {
                continue;
            };
            if let Ok((frontmatter, _)) = agent_skills::parse_frontmatter_public(&document) {
                names.insert(frontmatter.name);
            }
        }
        self.reconcile_skill_names(names)
    }

    fn reconcile_skill_names(&self, names: impl IntoIterator<Item = String>) -> Result<()> {
        for name in names {
            let directory = self.find_skill_directory(&name);
            self.remove_registered_skill(&name);
            match directory {
                Ok(directory) => {
                    let skill = self.load_skill_directory(&directory)?;
                    self.register_skill(&skill)?;
                }
                Err(SkillStoreError::NotFound(_)) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    /// Create a new skill
    /// # Errors
    /// Returns an error for invalid metadata or resource paths, an existing destination,
    /// filesystem failures, or a failure to register the created package.
    pub fn create_skill(&self, request: CreateSkillRequest) -> Result<CallableId> {
        // Validate skill name
        Self::validate_skill_name(&request.name)?;

        // Check if skill already exists
        let skill_dir = self.root.join(&request.name);
        if skill_dir.exists() {
            return Err(SkillStoreError::AlreadyExists(request.name));
        }

        let skill_md_content = Self::build_skill_md_content(&request)?;

        // Validate all filenames before writing anything to disk
        Self::validate_filenames(&request.scripts, "script")?;
        Self::validate_filenames(&request.references, "reference")?;
        Self::validate_filenames(&request.assets, "asset")?;

        std::fs::create_dir(&skill_dir)?;
        std::fs::write(skill_dir.join("SKILL.md"), skill_md_content)?;
        Self::write_files(&skill_dir, "scripts", &request.scripts)?;
        Self::write_files(&skill_dir, "references", &request.references)?;
        Self::write_files(&skill_dir, "assets", &request.assets)?;

        info!("Created skill: {} at {:?}", request.name, skill_dir);

        // Load and register the skill
        let skill = match self.load_skill_directory(&skill_dir) {
            Ok(skill) => skill,
            Err(e) => {
                // Clean up on error
                let _ = std::fs::remove_dir_all(&skill_dir);
                return Err(e);
            }
        };

        self.remove_registered_skill(&skill.manifest.id);
        let id = self.register_skill(&skill)?;

        info!("Skill {} created and registered successfully", request.name);
        Ok(id)
    }

    /// Update an existing skill
    ///
    /// Updates a skill in Agent Skills format (SKILL.md with YAML frontmatter).
    /// # Errors
    /// Returns an error if the skill is missing or read-only, an edit is invalid,
    /// a rename conflicts, or filesystem or registry reconciliation fails.
    pub fn update_skill(&self, skill_id: &str, request: UpdateSkillRequest) -> Result<CallableId> {
        let mut skill_dir = self.find_skill_directory(skill_id)?;
        self.require_managed(&skill_dir)?;
        let existing_content = self.load_skill_file(skill_id, "SKILL.md")?;
        let (mut frontmatter, mut body) = Self::document_parts(&existing_content)?;
        if let Some(content) = request.skill_md_content {
            let (supplied_frontmatter, supplied_body) = Self::document_parts(&content)?;
            frontmatter.extend(supplied_frontmatter);
            body = supplied_body;
        }
        if let Some(name) = &request.name {
            Self::validate_skill_name(name)?;
            Self::set_field(&mut frontmatter, "name", name)?;
        }
        if let Some(version) = &request.version {
            Self::set_field(&mut frontmatter, "version", version)?;
        }
        if let Some(description) = &request.description {
            Self::set_field(&mut frontmatter, "description", description)?;
        }
        if let Some(tools) = &request.uses_tools {
            Self::set_field(&mut frontmatter, "allowed-tools", tools)?;
        }
        if let Some(tags) = &request.tags {
            Self::set_field(&mut frontmatter, "tags", tags)?;
        }
        let skill_md_content = Self::serialize_document(&frontmatter, &body)?;
        let (updated, _) = agent_skills::parse_frontmatter_public(&skill_md_content)?;
        let renamed = updated.name != skill_id;
        let destination = if renamed {
            if skill_dir == self.root {
                return Err(SkillStoreError::ValidationError(
                    "Cannot rename the managed root package".to_string(),
                ));
            }
            let destination = self.root.join(&updated.name);
            if destination.exists() {
                return Err(SkillStoreError::AlreadyExists(updated.name));
            }
            Some(destination)
        } else {
            None
        };
        for (directory, files) in [
            ("scripts", &request.scripts),
            ("references", &request.references),
            ("assets", &request.assets),
        ] {
            Self::validate_filenames(files, directory)?;
            for (filename, _) in files {
                writable_file(&skill_dir, &Path::new(directory).join(filename))?;
            }
        }
        let skill_md_path = writable_file(&skill_dir, Path::new("SKILL.md"))?;
        std::fs::write(skill_md_path, skill_md_content)?;
        Self::write_files(&skill_dir, "scripts", &request.scripts)?;
        Self::write_files(&skill_dir, "references", &request.references)?;
        Self::write_files(&skill_dir, "assets", &request.assets)?;
        if let Some(destination) = destination {
            std::fs::rename(&skill_dir, &destination)?;
            skill_dir = destination;
        }
        self.reconcile_skill_names(
            std::iter::once(skill_id.to_string()).chain(renamed.then(|| updated.name.clone())),
        )?;

        info!("Updated skill: {} at {:?}", skill_id, skill_dir);

        let id = self
            .registry
            .get_by_fq_name(&format!("skill.{}", updated.name))
            .ok_or_else(|| SkillStoreError::NotFound(updated.name.clone()))?
            .id;

        info!("Skill {} updated and re-registered successfully", skill_id);
        Ok(id)
    }

    fn require_managed(&self, path: &Path) -> Result<()> {
        if !path.canonicalize()?.starts_with(&self.root) {
            return Err(SkillStoreError::ValidationError(
                "Read-only overlay skills cannot be modified".to_string(),
            ));
        }
        Ok(())
    }

    fn write_files(root: &Path, directory: &str, files: &[(String, String)]) -> Result<()> {
        for (filename, content) in files {
            let target = writable_file(root, &Path::new(directory).join(filename))?;
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(target, content)?;
        }
        Ok(())
    }

    fn remove_registered_skill(&self, skill_id: &str) {
        for skill in self.registry.by_kind(CallableKind::Skill) {
            if skill.name == skill_id {
                if let Some(search_engine) = &self.search_engine {
                    search_engine.remove_record(&skill.id);
                }
                self.registry.remove(&skill.id);
            }
        }
    }

    /// Delete a skill
    /// # Errors
    /// Returns an error if the operation cannot be completed.
    pub fn delete_skill(&self, skill_id: &str) -> Result<()> {
        let skill_dir = self.find_skill_directory(skill_id)?;
        self.require_managed(&skill_dir)?;
        if skill_dir == self.root {
            return Err(SkillStoreError::ValidationError(
                "Cannot delete the managed root package".to_string(),
            ));
        }
        std::fs::remove_dir_all(&skill_dir)?;
        self.reconcile_skill_names([skill_id.to_string()])?;

        info!("Deleted skill: {} from {:?}", skill_id, skill_dir);
        Ok(())
    }

    /// Validate skill name
    fn validate_skill_name(name: &str) -> Result<()> {
        if !Self::is_valid_skill_id(name) {
            return Err(SkillStoreError::ValidationError(format!(
                "Invalid skill name: {name}; expected 1-64 lowercase letters, digits or single hyphens"
            )));
        }
        Ok(())
    }

    /// Validate every installed package, including packages that cannot be loaded.
    pub async fn validate_all(&self) -> ValidationResult {
        let mut result = ValidationResult::success();
        let (paths, errors) = self.package_directories();
        for error in errors {
            result.add_error(error);
        }
        for path in paths {
            match agent_skills::AgentSkill::from_directory(&path).await {
                Ok(agent_skill) => {
                    let skill = Skill {
                        manifest: agent_skill.to_skill_manifest(),
                        path: path.clone(),
                        documentation: Some(agent_skill.content),
                    };
                    let diagnostics = self.validate_skill(&skill);
                    for error in diagnostics.errors {
                        result.add_error(format!("{}: {error}", path.display()));
                    }
                    for warning in diagnostics.warnings {
                        result.add_warning(format!("{}: {warning}", path.display()));
                    }
                }
                Err(error) => result.add_error(format!("{}: {error}", path.display())),
            }
        }
        result
    }

    /// Validate a skill manifest and content
    #[must_use]
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
        if skill.manifest.description.trim().is_empty() {
            result.add_error("Description cannot be empty".to_string());
        } else if skill.manifest.description.chars().count() > 1024 {
            result.add_error("Description exceeds 1024 characters".to_string());
        }

        // Validate input schema
        if let Err(e) = Self::validate_json_schema(&skill.manifest.inputs) {
            result.add_error(format!("Invalid input schema: {e}"));
        }

        // Validate output schema if present
        if let Some(ref outputs) = skill.manifest.outputs {
            if let Err(e) = Self::validate_json_schema(outputs) {
                result.add_error(format!("Invalid output schema: {e}"));
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
                        "Tool '{tool}' not found in registry (may be loaded later)"
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
                result.add_error(format!("Invalid risk tier: {tier}"));
            }
        }

        // Validate hints
        if let Some(calls) = skill.manifest.hints.expected_calls {
            if calls == 0 {
                result.add_warning("Expected calls is 0".to_string());
            } else if calls > 100 {
                result.add_warning(format!("Expected calls is very high: {calls}"));
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

        match inventory_files(&skill.path) {
            Ok(files) => {
                for file in files {
                    if let Err(error) = std::fs::File::open(skill.path.join(&file)) {
                        result.add_error(format!("Cannot read {}: {error}", file.display()));
                    }
                }
            }
            Err(error) => result.add_error(error.to_string()),
        }

        result
    }

    /// Validate JSON Schema
    fn validate_json_schema(schema: &serde_json::Value) -> Result<()> {
        // Basic JSON Schema validation
        if !schema.is_object() {
            return Err(SkillStoreError::ValidationError(
                "Schema must be an object".to_string(),
            ));
        }

        let Some(obj) = schema.as_object() else {
            return Err(SkillStoreError::ValidationError(
                "Schema must be an object".to_string(),
            ));
        };

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
                        "Invalid schema type: {type_str}"
                    )));
                }
            }
        }

        Ok(())
    }

    /// Check if skill ID is valid format
    fn is_valid_skill_id(id: &str) -> bool {
        !id.is_empty()
            && id.chars().count() <= 64
            && !id.starts_with('-')
            && !id.ends_with('-')
            && !id.contains("--")
            && id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    }

    /// Detect circular dependencies in skills
    /// # Errors
    /// Returns an error if the operation cannot be completed.
    pub fn detect_circular_dependencies(&self, skill_id: &str) -> Result<Vec<String>> {
        let mut visited = std::collections::HashSet::new();
        let mut stack = vec![skill_id.to_string()];
        let mut path = vec![];

        while let Some(current) = stack.pop() {
            if visited.contains(&current) {
                // Found a cycle
                if let Some(pos) = path.iter().position(|s| s == &current) {
                    return Ok(path.get(pos..).unwrap_or_default().to_vec());
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
fn canonical_directory(path: &Path) -> std::io::Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut ancestor = absolute.as_path();
    let mut missing = Vec::new();
    loop {
        match ancestor.canonicalize() {
            Ok(mut canonical) => {
                for component in missing.into_iter().rev() {
                    canonical.push(component);
                }
                return Ok(canonical);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let Some(name) = ancestor.file_name() else {
                    return Err(error);
                };
                missing.push(name);
                ancestor = ancestor.parent().ok_or(error)?;
            }
            Err(error) => return Err(error),
        }
    }
}

fn excluded_directory(name: &std::ffi::OsStr) -> bool {
    name == ".git" || name == "node_modules"
}

fn sorted_entries(path: &Path) -> std::io::Result<Vec<std::fs::DirEntry>> {
    let mut entries = std::fs::read_dir(path)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    Ok(entries)
}

fn contained_file(root: &Path, filename: &str) -> std::io::Result<PathBuf> {
    let relative = Path::new(filename);
    if filename.is_empty()
        || filename.contains('\\')
        || !relative
            .components()
            .all(|part| matches!(part, std::path::Component::Normal(_)))
        || relative
            .components()
            .any(|part| excluded_directory(part.as_os_str()))
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "Invalid resource path",
        ));
    }
    let root = root.canonicalize()?;
    let file = root.join(relative).canonicalize()?;
    if !file.starts_with(&root) || !file.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "Resource escapes skill directory or is not a file",
        ));
    }
    Ok(file)
}

fn writable_file(root: &Path, relative: &Path) -> std::io::Result<PathBuf> {
    let mut target = root.to_path_buf();
    for component in relative.components() {
        if !matches!(component, std::path::Component::Normal(_)) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "Invalid write path",
            ));
        }
        target.push(component);
        match std::fs::symlink_metadata(&target) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "Cannot modify symlink resources",
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(target)
}

fn inventory_files(root: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in sorted_entries(&directory)? {
            let kind = entry.file_type()?;
            if kind.is_symlink() || excluded_directory(&entry.file_name()) {
                continue;
            }
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() {
                if let Ok(relative) = entry.path().strip_prefix(root) {
                    files.push(relative.to_path_buf());
                }
            }
        }
    }
    files.sort();
    Ok(files)
}

fn resource_paths(files: &[PathBuf], tools: &[BundledTool]) -> Vec<String> {
    files
        .iter()
        .filter_map(|path| {
            let name = path.to_str()?;
            (name != "SKILL.md" && !tools.iter().any(|tool| tool.name == name))
                .then(|| name.to_string())
        })
        .collect()
}
