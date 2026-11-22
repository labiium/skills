//! skills.rs CLI
//!
//! Main entry point for the skills.rs unified MCP server.
//! Supports multiple modes:
//! - stdio: Run as stdio MCP server
//! - http: Run as HTTP MCP server
//! - validate: Validate configuration and skills

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use rmcp::{
    transport::{
        stdio,
        streamable_http_server::{
            session::local::LocalSessionManager,
            tower::{StreamableHttpServerConfig, StreamableHttpService},
        },
    },
    ServiceExt,
};
use skillsrs::core::persistence::PersistenceLayer;
use skillsrs::core::policy::{ConsentLevel, PolicyConfig, PolicyEngine, PolicyMode};
use skillsrs::core::registry::Registry;
use skillsrs::core::CallableKind;
use skillsrs::execution::upstream::{
    Transport, UpstreamConfig, UpstreamManager, MCP_CURRENT_PROTOCOL_VERSION,
};
use skillsrs::execution::{sandbox::SandboxBackend, sandbox::SandboxConfig, Runtime};
use skillsrs::mcp::SkillsServer;
use skillsrs::paths::{self, paths_from_env, PathsConfig, SkillsPaths};
use skillsrs::storage::search::SearchEngine;
use skillsrs::storage::sync::{
    canonical_source, normalize_source, AgentSkillsRepoConfig, AgentSkillsSync,
};
use skillsrs::storage::SkillStore;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use tracing::{error, info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Parser)]
#[command(name = "skills")]
#[command(version)]
#[command(about = "skills.rs - Infinite Skills. Finite Context.", long_about = None)]
#[command(before_help = r#"
 ███████╗██╗  ██╗██╗██╗     ██╗     ███████╗
 ██╔════╝██║ ██╔╝██║██║     ██║     ██╔════╝
 ███████╗█████╔╝ ██║██║     ██║     ███████╗
 ╚════██║██╔═██╗ ██║██║     ██║     ╚════██║
 ███████║██║  ██╗██║███████╗███████╗███████║
 ╚══════╝╚═╝  ╚═╝╚═╝╚══════╝╚══════╝╚══════╝
"#)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "CLI switches are independent options, not exclusive states"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Config file path (defaults to system config directory)
    #[arg(short, long)]
    config: Option<String>,

    /// Force using only global config + global skills/upstreams (ignore local config discovery)
    #[arg(long)]
    global: bool,

    /// Root path for skills data (overrides config)
    #[arg(long, env = "SKILLS_PATH")]
    path: Option<String>,

    /// Disable sandboxing (allows full system access - use with caution)
    #[arg(long, env = "SKILLS_NO_SANDBOX")]
    no_sandbox: bool,

    /// Use current directory for operations (implies --no-sandbox)
    #[arg(long)]
    current_dir: bool,

    /// Load repository .agents/skills and .claude/skills from this trusted project
    #[arg(long)]
    trust_project_skills: bool,

    /// Additional read-only skill directories, in precedence order
    #[arg(long = "skill-root")]
    skill_roots: Vec<PathBuf>,

    /// Log level
    #[arg(short, long)]
    log_level: Option<String>,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize a project-local skills configuration in the current directory
    ///
    /// Creates a `.skills/` directory containing:
    /// - `.skills/config.yaml` (project-local config)
    /// - `.skills/skills/` (project-local skills root)
    ///
    /// After init, commands will automatically discover `.skills/config.yaml`
    /// when run from this project (or any subdirectory).
    ///
    /// Initialization fails if the configuration already exists, unless --force is supplied.
    Init {
        /// Overwrite existing `.skills/config.yaml` if it already exists
        #[arg(long)]
        force: bool,
    },

    /// Run server in stdio mode (default for MCP)
    Server {
        #[command(subcommand)]
        mode: Option<ServerMode>,
    },

    /// List all servers and tools (AI agent mode)
    #[command(visible_alias = "ls")]
    List {
        /// Server name to filter by
        server: Option<String>,

        /// Include descriptions
        #[arg(short, long)]
        descriptions: bool,

        /// JSON output
        #[arg(short, long)]
        json: bool,
    },

    /// Get tool schema or execute tool (AI agent mode)
    /// Usage: skills tool <server>/<tool> [args]
    Tool {
        /// Tool path: `server/tool_name`
        tool_path: String,

        /// JSON arguments (or use stdin)
        args: Option<String>,

        /// JSON output
        #[arg(short, long)]
        json: bool,

        /// Raw text output (no formatting)
        #[arg(short, long)]
        raw: bool,

        /// Host-confirmed consent for this invocation
        #[arg(long, default_value = "none")]
        consent: String,

        /// Execution deadline, capped by configured policy
        #[arg(long)]
        timeout_ms: Option<u64>,
        #[arg(long)]
        input_responses: Option<String>,
        #[arg(long)]
        request_state: Option<String>,
    },

    /// Search/grep tools by pattern (AI agent mode)
    Grep {
        /// Glob pattern (e.g., "*file*")
        pattern: String,

        /// Include descriptions
        #[arg(short, long)]
        descriptions: bool,
    },

    /// Execute a tool directly (alias for 'tool')
    #[command(visible_alias = "exec")]
    Execute {
        /// Tool path: `server/tool_name`
        tool_path: String,

        /// JSON arguments (or use stdin)
        args: Option<String>,

        /// JSON output
        #[arg(short, long)]
        json: bool,

        /// Host-confirmed consent for this invocation
        #[arg(long, default_value = "none")]
        consent: String,

        /// Execution deadline, capped by configured policy
        #[arg(long)]
        timeout_ms: Option<u64>,
        #[arg(long)]
        input_responses: Option<String>,
        #[arg(long)]
        request_state: Option<String>,
    },

    /// Validate configuration and skills
    Validate,

    /// Install instruction-first skills from a directory or Git source
    ///
    /// Usage:
    ///   skills add <owner/repo>              # Add all skills from repo
    ///   skills add <url> --skill <name>      # Add specific skill
    ///   skills add <owner/repo> --skill <name> --skill <name2>  # Add multiple
    ///
    /// Examples:
    ///   skills add vercel-labs/agent-skills
    ///   skills add <https://github.com/wshobson/agents> --skill monorepo-management
    Add {
        /// Local path, Git URL, or GitHub shorthand (owner/repo)
        repo: String,

        /// Specific skill name(s) to import (if omitted, imports all)
        #[arg(short, long)]
        skill: Vec<String>,

        /// Optional Git branch, tag, or commit
        #[arg(long)]
        git_ref: Option<String>,

        /// Force overwrite existing skills
        #[arg(short, long)]
        force: bool,

        /// List available packages without installing them
        #[arg(long)]
        list: bool,
    },

    /// Update configured skill sources and restore missing installations
    #[command(visible_alias = "update")]
    Sync,

    /// Preview source changes: exit 0 if up to date, 1 if changes exist, 2 on error
    Check,

    /// Show system paths and configuration
    Paths,

    /// Ranked discovery across installed skills and configured upstream tools
    Search {
        /// Search query
        query: String,

        /// Kind filter
        #[arg(short, long)]
        kind: Option<String>,
        #[arg(long, default_value_t = 10)]
        limit: usize,
        #[arg(long)]
        cursor: Option<String>,
        #[arg(long)]
        json: bool,
    },

    /// Manage skills (create, edit, delete, show)
    #[command(subcommand)]
    Skill(SkillCommands),

    /// Add an MCP server to the configuration
    #[command(after_help = format!(
        "Examples:\n  skills add-mcp http internal https://mcp.example.com/mcp\n  \
         skills add-mcp stdio local -- /path/to/server --root '/path with spaces'\n\n\
         Upstreams require MCP {MCP_CURRENT_PROTOCOL_VERSION}; older initialize-based revisions are not supported."
    ))]
    AddMcp {
        /// Transport type: http or stdio
        #[arg(value_enum)]
        transport: TransportType,

        /// Alias/name for the MCP server
        alias: String,

        /// HTTP URL, or a stdio executable followed by its arguments (use -- before the executable)
        #[arg(required = true, num_args = 1..)]
        endpoint: Vec<String>,

        /// Tags for categorization (comma-separated)
        #[arg(short, long, value_delimiter = ',')]
        tags: Vec<String>,

        /// Save without testing the connection
        #[arg(long)]
        no_test: bool,

        #[arg(short, long)]
        description: Option<String>,
    },

    /// Edit/update an existing upstream MCP server
    EditMcp {
        /// Alias of the upstream server to edit
        alias: String,

        /// New description
        #[arg(short, long)]
        description: Option<String>,

        /// New tags (comma-separated, replaces existing)
        #[arg(short, long, value_delimiter = ',')]
        tags: Vec<String>,

        /// New URL (for http transport)
        #[arg(short, long)]
        url: Option<String>,

        /// New stdio executable and arguments, following --
        #[arg(last = true, num_args = 1..)]
        command: Option<Vec<String>>,
    },
}

#[derive(Clone, Debug, clap::ValueEnum)]
enum TransportType {
    Http,
    Stdio,
}

#[derive(Subcommand)]
enum SkillCommands {
    /// Activate instructions, or explicitly execute a selected supporting script
    Run {
        skill_id: String,
        #[arg(long)]
        script: Option<String>,
        #[arg(long, default_value = "{}")]
        args: String,
        #[arg(long, default_value = "none")]
        consent: String,
        #[arg(long)]
        timeout_ms: Option<u64>,
    },

    /// Create a new skill
    Create {
        /// Skill name (unique identifier)
        name: String,

        /// Skill description
        #[arg(short, long)]
        description: Option<String>,

        /// Version (defaults to 1.0.0)
        #[arg(short, long, default_value = "1.0.0")]
        version: String,

        /// Path to SKILL.md file (or use stdin)
        #[arg(short, long)]
        skill_md: Option<String>,

        /// SKILL.md content as inline text (use - to read from stdin)
        #[arg(long)]
        content: Option<String>,

        /// Tools this skill uses (comma-separated)
        #[arg(short, long, value_delimiter = ',')]
        uses_tools: Vec<String>,
    },

    /// Edit/update an existing skill
    Edit {
        /// Skill ID or name
        skill_id: String,

        /// New name
        #[arg(short, long)]
        name: Option<String>,

        /// New description
        #[arg(short, long)]
        description: Option<String>,

        /// New version
        #[arg(short, long)]
        version: Option<String>,

        /// Path to updated SKILL.md file
        #[arg(short, long)]
        skill_md: Option<String>,

        /// SKILL.md content as inline text (use - to read from stdin)
        #[arg(long)]
        content: Option<String>,

        /// Replace pattern (sed-like)
        #[arg(long, requires = "replace_with")]
        replace: Option<String>,

        /// Replacement text for pattern
        #[arg(long)]
        replace_with: Option<String>,

        /// Append text to SKILL.md
        #[arg(long)]
        append: Option<String>,

        /// Prepend text to SKILL.md
        #[arg(long)]
        prepend: Option<String>,

        /// Updated tools list (comma-separated)
        #[arg(short, long, value_delimiter = ',')]
        uses_tools: Option<Vec<String>>,
    },

    /// Delete a skill
    Delete {
        /// Skill ID or name to delete
        skill_id: String,

        /// Force deletion without confirmation
        #[arg(short, long)]
        force: bool,
    },

    /// Show/display skill content
    Show {
        /// Skill ID or name
        skill_id: String,

        /// Show specific file instead of SKILL.md
        #[arg(short, long)]
        file: Option<String>,
    },
}

#[derive(Subcommand)]
enum ServerMode {
    /// Run in stdio mode (default for MCP)
    Stdio,

    /// Run in HTTP mode
    Http {
        /// Bind address (defaults to server.bind, then 127.0.0.1:8000)
        #[arg(short, long)]
        bind: Option<String>,
    },
}

/// Server configuration
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, Default)]
#[serde(deny_unknown_fields)]
struct Config {
    #[serde(default, skip_serializing_if = "is_default_server_config")]
    #[allow(dead_code)]
    server: ServerConfig,

    #[serde(default, skip_serializing_if = "is_default_policy_config")]
    policy: PolicyConfig,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    upstreams: Vec<skillsrs::execution::upstream::UpstreamConfig>,

    #[serde(default, skip_serializing_if = "is_default_paths_config")]
    paths: PathsConfig,

    #[serde(default, skip_serializing_if = "is_default_sandbox_config")]
    sandbox: SandboxConfig,

    #[serde(default, skip_serializing_if = "is_default_use_global")]
    use_global: UseGlobalSettings,

    /// Agent Skills repositories to auto-sync
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    agent_skills_repos: Vec<skillsrs::storage::sync::AgentSkillsRepoConfig>,

    #[serde(skip)]
    sections: ConfigSections,
    #[serde(skip)]
    paths_base: Option<PathBuf>,
}

#[derive(Debug, Clone, Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "Each flag records whether an independent config section was supplied"
)]
struct ConfigSections {
    paths: bool,
    sandbox: bool,
    policy: bool,
    server: bool,
}

// Helper functions for skip_serializing_if checks
fn is_default_server_config(cfg: &ServerConfig) -> bool {
    cfg.bind == default_bind()
        && cfg.transport == default_transport()
        && cfg.log_level == default_log_level()
        && cfg.auth_token_env.is_none()
}

fn is_default_policy_config(cfg: &PolicyConfig) -> bool {
    let default = PolicyConfig::default();
    cfg.mode == default.mode
        && cfg.default_risk == default.default_risk
        && cfg.require_consent_for == default.require_consent_for
        && cfg.trusted_servers == default.trusted_servers
        && cfg.deny_tags == default.deny_tags
        && cfg.management_enabled == default.management_enabled
        && cfg.enforce_server_trust == default.enforce_server_trust
        && cfg.max_exec_ms == default.max_exec_ms
        && cfg.allow_patterns == default.allow_patterns
        && cfg.deny_patterns == default.deny_patterns
}

const fn is_default_paths_config(cfg: &PathsConfig) -> bool {
    cfg.data_dir.is_none()
        && cfg.config_dir.is_none()
        && cfg.cache_dir.is_none()
        && cfg.database_path.is_none()
        && cfg.logs_dir.is_none()
}

fn is_default_sandbox_config(cfg: &SandboxConfig) -> bool {
    cfg == &SandboxConfig::default()
}

const fn is_default_use_global(cfg: &UseGlobalSettings) -> bool {
    !cfg.enabled
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, Default)]
struct UseGlobalSettings {
    /// If true, overlay the project config on top of global config.
    /// This allows using global upstreams/skills together with project ones.
    #[serde(default)]
    enabled: bool,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct ServerConfig {
    #[serde(default = "default_bind")]
    #[allow(dead_code)]
    bind: String,

    #[serde(default = "default_transport")]
    #[allow(dead_code)]
    transport: String,

    #[serde(default = "default_log_level")]
    #[allow(dead_code)]
    log_level: String,

    /// Environment variable containing the HTTP bearer token
    #[serde(default, skip_serializing_if = "Option::is_none")]
    auth_token_env: Option<String>,
}

fn default_bind() -> String {
    "127.0.0.1:8000".to_string()
}

fn default_transport() -> String {
    "stdio".to_string()
}

fn default_log_level() -> String {
    "info".to_string()
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: default_bind(),
            transport: default_transport(),
            log_level: default_log_level(),
            auth_token_env: None,
        }
    }
}

/// Initialize logging
fn init_logging(
    level: &str,
) -> tracing_subscriber::reload::Handle<tracing_subscriber::EnvFilter, tracing_subscriber::Registry>
{
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| format!("{}={level}", env!("CARGO_CRATE_NAME")).into());
    let (filter, handle) = tracing_subscriber::reload::Layer::new(filter);
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .init();
    handle
}

/// Load configuration from file
fn load_config(path: &std::path::Path) -> Result<Config> {
    if !path.exists() {
        info!("Config file not found: {}, using defaults", path.display());
        return Ok(Config::default());
    }

    info!("Loading config from: {}", path.display());
    let contents = std::fs::read_to_string(path)?;
    let document: serde_yaml::Value = serde_yaml::from_str(&contents)?;
    let mut sections = ConfigSections::default();
    if let Some(mapping) = document.as_mapping() {
        for key in mapping.keys().filter_map(serde_yaml::Value::as_str) {
            match key {
                "paths" => sections.paths = true,
                "sandbox" => sections.sandbox = true,
                "policy" => sections.policy = true,
                "server" => sections.server = true,
                _ => {}
            }
        }
    }
    let mut config: Config = serde_yaml::from_value(document)?;
    config.sections = sections;
    let base = configuration_base(path)?;
    config.paths_base = Some(base.clone());
    for source in &mut config.agent_skills_repos {
        let local = Path::new(&source.repo);
        if let Some(rest) = source.repo.strip_prefix("~/") {
            source.repo = directories::BaseDirs::new()
                .context("Cannot determine home directory")?
                .home_dir()
                .join(rest)
                .to_string_lossy()
                .into_owned();
        } else if !local.is_absolute()
            && (source.repo.starts_with('.')
                || (!source.repo.contains('/') && !source.repo.contains(':')))
        {
            source.repo = base.join(local).to_string_lossy().into_owned();
        } else if !local.is_absolute() && !source.repo.contains(':') {
            source.repo = format!("https://github.com/{}", source.repo);
        }
    }
    Ok(config)
}

/// Ensure global configuration file exists, creating it with defaults if needed
fn ensure_global_config(paths: &SkillsPaths) -> Result<PathBuf> {
    let config_path = paths.default_config_file();

    if !config_path.exists() {
        info!(
            "Creating default global config at: {}",
            config_path.display()
        );

        // Ensure parent directory exists
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Create default config
        let config = Config::default();
        save_config(&config, &config_path)?;

        info!("Global config created successfully");
    }

    Ok(config_path)
}

/// Save configuration to file
fn save_config(config: &Config, path: &std::path::Path) -> Result<()> {
    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let yaml = serde_yaml::to_string(config)?;
    std::fs::write(path, yaml)?;
    info!("Config saved to: {}", path.display());
    Ok(())
}

fn source_matches(source: &AgentSkillsRepoConfig, identity: &str) -> bool {
    normalize_source(&source.repo).is_ok_and(|candidate| candidate == identity)
}

fn record_source_add(
    config: &mut Config,
    existing: Option<&AgentSkillsRepoConfig>,
    mut source: AgentSkillsRepoConfig,
    identity: &str,
) {
    if let Some(existing) = existing {
        source.alias.clone_from(&existing.alias);
        if let (Some(new), Some(previous)) = (&mut source.skills, &existing.skills) {
            new.extend(previous.iter().cloned());
            new.sort_unstable();
            new.dedup();
        } else {
            source.skills = None;
        }
    }
    if let Some(entry) = config
        .agent_skills_repos
        .iter_mut()
        .find(|entry| source_matches(entry, identity))
    {
        *entry = source;
    } else {
        config.agent_skills_repos.push(source);
    }
}

fn merge_config(base: &mut Config, overlay: Config) {
    for upstream in overlay.upstreams {
        if let Some(existing) = base
            .upstreams
            .iter_mut()
            .find(|existing| existing.alias == upstream.alias)
        {
            *existing = upstream;
        } else {
            base.upstreams.push(upstream);
        }
    }
    for source in overlay.agent_skills_repos {
        if let Some(existing) = base
            .agent_skills_repos
            .iter_mut()
            .find(|existing| existing.repo == source.repo)
        {
            *existing = source;
        } else {
            base.agent_skills_repos.push(source);
        }
    }
    if overlay.sections.paths || !is_default_paths_config(&overlay.paths) {
        base.paths = overlay.paths;
        base.paths_base = overlay.paths_base;
    }
    if overlay.sections.sandbox || !is_default_sandbox_config(&overlay.sandbox) {
        base.sandbox = overlay.sandbox;
    }
    if overlay.sections.policy || !is_default_policy_config(&overlay.policy) {
        base.policy = overlay.policy;
    }
    if overlay.sections.server || !is_default_server_config(&overlay.server) {
        base.server = overlay.server;
    }
    base.use_global = overlay.use_global;
}

fn configuration_base(path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let parent = absolute
        .parent()
        .context("Configuration has no parent directory")?;
    Ok(
        if parent.file_name().is_some_and(|name| name == ".skills") {
            parent.parent().unwrap_or(parent)
        } else {
            parent
        }
        .to_path_buf(),
    )
}

/// Resolve paths with precedence: CLI args > env vars > config > system defaults
fn resolve_paths(cli: &Cli, config: &Config, config_file: &Path) -> Result<SkillsPaths> {
    // Start with system defaults
    let mut paths = SkillsPaths::new()?;

    let base = config
        .paths_base
        .clone()
        .map_or_else(|| configuration_base(config_file), Ok)?;
    let mut configured_paths = config.paths.clone();
    for value in [
        &mut configured_paths.data_dir,
        &mut configured_paths.config_dir,
        &mut configured_paths.cache_dir,
        &mut configured_paths.database_path,
        &mut configured_paths.logs_dir,
    ]
    .into_iter()
    .flatten()
    {
        if value.is_relative() {
            *value = base.join(&*value);
        }
    }
    paths = configured_paths.apply_to(paths);

    // Apply environment variable overrides
    paths = paths_from_env().apply_to(paths);

    // Apply CLI argument override (highest priority)
    if let Some(ref path) = cli.path {
        let base = PathBuf::from(path);
        paths.data_dir.clone_from(&base);
        paths.config_dir = base.join("config");
        paths.cache_dir = base.join("cache");
        paths.database_path = base.join("skills.db");
        paths.skills_root = base.join("skills");
        paths.logs_dir = base.join("logs");
    }

    Ok(paths)
}

fn read_skill_roots(cli: &Cli) -> Result<Vec<PathBuf>> {
    let mut roots = cli.skill_roots.clone();
    for root in &roots {
        anyhow::ensure!(
            root.is_dir(),
            "Skill root is not a directory: {}",
            root.display()
        );
    }
    if cli.path.is_none() && std::env::var_os("SKILLS_PATH").is_none() {
        let home = directories::BaseDirs::new().context("Cannot determine home directory")?;
        let cwd = std::env::current_dir()?;
        let project = (cli.trust_project_skills && !cli.global).then_some(cwd.as_path());
        roots.extend(paths::shared_skill_roots(project, home.home_dir()));
    }
    Ok(roots)
}

fn configure_read_roots(store: &mut SkillStore, roots: &[PathBuf]) {
    for root in roots {
        store.add_read_root(root.clone());
    }
}

fn make_store(
    paths: &SkillsPaths,
    registry: Arc<Registry>,
    roots: &[PathBuf],
) -> Result<SkillStore> {
    let mut store = SkillStore::new(&paths.skills_root, registry)?;
    configure_read_roots(&mut store, roots);
    Ok(store)
}

fn validate_http_endpoint(url: &str) -> Result<()> {
    let parsed = reqwest::Url::parse(url).context("Invalid upstream URL")?;
    anyhow::ensure!(
        matches!(parsed.scheme(), "http" | "https")
            && parsed.username().is_empty()
            && parsed.password().is_none(),
        "Upstream URL must be HTTP(S) without embedded credentials"
    );
    Ok(())
}

fn effective_sandbox(config: &Config, disabled: bool) -> SandboxConfig {
    let mut sandbox = config.sandbox.clone();
    if disabled {
        sandbox.backend = SandboxBackend::None;
        sandbox.allow_network = true;
        sandbox.allow_read.clear();
        sandbox.allow_write.clear();
        sandbox.timeout_ms = 0;
        sandbox.max_memory_bytes = 0;
        sandbox.max_cpu_seconds = 0;
    }
    sandbox
}

struct CliComponents {
    registry: Arc<Registry>,
    policy: PolicyEngine,
    runtime: Runtime,
}

#[derive(Clone, Copy)]
enum CliUpstreams<'a> {
    None,
    All,
    One(&'a str),
}

async fn init_cli_components(
    config: &Config,
    paths: &SkillsPaths,
    roots: &[PathBuf],
    no_sandbox: bool,
    upstreams: CliUpstreams<'_>,
) -> Result<CliComponents> {
    let registry = Arc::new(Registry::new());
    let policy = PolicyEngine::new(PolicyConfig::from_environment(config.policy.clone()))?;
    let manager = Arc::new(UpstreamManager::new(registry.clone()));
    let sandbox = effective_sandbox(config, no_sandbox || policy.is_open());
    manager.set_sandbox_config(sandbox.clone()).await;
    if policy.is_open() {
        manager.set_unrestricted();
    }
    let store = make_store(paths, registry.clone(), roots)?;
    store.load_and_register_all().await?;
    for upstream in &config.upstreams {
        let explicitly_requested = match upstreams {
            CliUpstreams::None => break,
            CliUpstreams::All => false,
            CliUpstreams::One(alias) if alias == upstream.alias => true,
            CliUpstreams::One(_) => continue,
        };
        if let Err(error) = manager.add_upstream(upstream.clone()).await {
            if explicitly_requested || upstream.required {
                return Err(error)
                    .with_context(|| format!("Cannot connect upstream {}", upstream.alias));
            }
            eprintln!(
                "Warning: Cannot connect optional upstream {}: {error}",
                upstream.alias
            );
        }
    }
    let mut runtime = Runtime::with_sandbox_config(registry.clone(), manager, sandbox);
    if policy.is_open() {
        runtime.set_unrestricted();
    }
    Ok(CliComponents {
        registry,
        policy,
        runtime,
    })
}

fn parse_consent(value: &str) -> Result<ConsentLevel> {
    match value {
        "none" => Ok(ConsentLevel::None),
        "user_confirmed" => Ok(ConsentLevel::UserConfirmed),
        "admin_confirmed" => Ok(ConsentLevel::AdminConfirmed),
        _ => anyhow::bail!("Consent must be none, user_confirmed, or admin_confirmed"),
    }
}

struct CliToolRequest {
    path: String,
    args: Option<String>,
    json: bool,
    raw: bool,
    describe_if_empty: bool,
    consent: String,
    timeout_ms: Option<u64>,
    input_responses: Option<String>,
    request_state: Option<String>,
}

async fn run_cli_tool(
    config: &Config,
    paths: &SkillsPaths,
    roots: &[PathBuf],
    no_sandbox: bool,
    request: CliToolRequest,
) -> Result<()> {
    let (server, name) = request
        .path
        .split_once('/')
        .filter(|(server, name)| !server.is_empty() && !name.is_empty() && !name.contains('/'))
        .context("Tool path must be <server>/<tool>")?;
    let components =
        init_cli_components(config, paths, roots, no_sandbox, CliUpstreams::One(server)).await?;
    let record = components
        .registry
        .all()
        .into_iter()
        .find(|record| {
            record.name == name && record.server_alias.as_deref().unwrap_or("local") == server
        })
        .with_context(|| format!("Callable not found: {}", request.path))?;
    let visibility = components.policy.authorize_read(&record);
    anyhow::ensure!(visibility.allowed, "Access denied: {}", visibility.reason);
    if request.describe_if_empty && request.args.is_none() {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "name": record.name, "description": record.description, "inputSchema": record.input_schema,
            }))?
        );
        return Ok(());
    }
    let arguments = match request.args {
        Some(args) if args != "-" => args,
        _ => {
            let mut input = String::new();
            std::io::stdin().read_to_string(&mut input)?;
            input
        }
    };
    let arguments: serde_json::Value =
        serde_json::from_str(&arguments).context("Invalid JSON arguments")?;
    let authorization = if record.kind == CallableKind::Skill {
        components.policy.authorize_read(&record)
    } else {
        components
            .policy
            .authorize(&record, &arguments, parse_consent(&request.consent)?)?
    };
    anyhow::ensure!(
        authorization.allowed,
        "Execution denied: {}",
        authorization.reason
    );
    let input_responses = request
        .input_responses
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .context("Invalid input responses JSON")?;
    let request_state = request
        .request_state
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .context("Invalid request state JSON")?;
    let result = components
        .runtime
        .execute_with_continuation(
            skillsrs::ExecContext {
                callable_id: record.id,
                arguments,
                timeout_ms: Some(components.policy.check_timeout(request.timeout_ms)?),
                trace_enabled: false,
            },
            input_responses,
            request_state,
        )
        .await?;
    if request.raw {
        for content in &result.content {
            if let Some(text) = content.as_text() {
                print!("{text}");
            }
        }
    } else {
        if !request.json {
            eprintln!("Execution status: {:?}", result.status);
        }
        println!("{}", serde_json::to_string_pretty(&result)?);
    }
    anyhow::ensure!(!result.is_error, "Upstream returned a tool error");
    Ok(())
}

fn http_token(
    config: &ServerConfig,
    address: std::net::SocketAddr,
    policy: &PolicyConfig,
) -> Result<Option<Arc<str>>> {
    let token = config
        .auth_token_env
        .as_ref()
        .map(|name| {
            let value = std::env::var(name)
                .with_context(|| format!("HTTP token environment variable {name} is missing"))?;
            anyhow::ensure!(
                !value.trim().is_empty(),
                "HTTP bearer token must not be empty"
            );
            Ok::<Arc<str>, anyhow::Error>(Arc::from(value))
        })
        .transpose()?;
    anyhow::ensure!(
        token.is_some()
            || (policy.mode == PolicyMode::Standard
                && !policy.management_enabled
                && policy.enforce_server_trust
                && policy.trusted_servers.is_empty()
                && policy.default_risk.eq_ignore_ascii_case("unknown")
                && policy
                    .require_consent_for
                    .iter()
                    .any(|risk| risk.eq_ignore_ascii_case("unknown"))),
        "An execution-enabled HTTP profile requires server.auth_token_env, including on loopback"
    );
    anyhow::ensure!(
        address.ip().is_loopback() || token.is_some(),
        "Non-loopback HTTP binding requires server.auth_token_env"
    );
    Ok(token)
}

fn bearer_matches(header: Option<&axum::http::HeaderValue>, expected: &str) -> bool {
    let Some(candidate) = header
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
    else {
        return false;
    };
    if candidate.len() != expected.len() {
        return false;
    }
    candidate
        .bytes()
        .zip(expected.bytes())
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

/// Initialize the server components
async fn init_server(
    config: &Config,
    paths: &SkillsPaths,
    no_sandbox: bool,
    read_roots: &[PathBuf],
) -> Result<SkillsServer> {
    info!("Initializing skills.rs server");
    info!("Using skills root: {}", paths.skills_root.display());
    info!("Using database: {}", paths.database_path.display());

    // Initialize persistence layer for FTS search
    let _persistence = Arc::new(
        PersistenceLayer::new(&paths.database_path)
            .await
            .context("Failed to initialize persistence")?,
    );

    if no_sandbox {
        info!("⚠️  Sandboxing DISABLED - tools have full system access");
    }

    // Create registry
    let registry = Arc::new(Registry::new());

    // Create search engine
    let search_engine = Arc::new(SearchEngine::new(registry.clone()));

    // Create policy engine
    let policy = PolicyConfig::from_environment(config.policy.clone());
    PolicyConfig::mesurii_source_revision(policy.mode == PolicyMode::Open)
        .context("invalid Mesurii Skills source identity")?;
    let policy_engine = Arc::new(PolicyEngine::new(policy)?);

    // Create upstream manager and connect to upstreams
    let upstream_manager = Arc::new(UpstreamManager::new(registry.clone()));
    let sandbox_config = effective_sandbox(config, no_sandbox || policy_engine.is_open());
    upstream_manager
        .set_sandbox_config(sandbox_config.clone())
        .await;
    if policy_engine.is_open() {
        upstream_manager.set_unrestricted();
    }
    for upstream_config in &config.upstreams {
        info!("Connecting to upstream: {}", upstream_config.alias);
        if let Err(e) = upstream_manager.add_upstream(upstream_config.clone()).await {
            if upstream_config.required
                || (policy_engine.is_open()
                    && matches!(upstream_config.alias.as_str(), "codiium" | "dociium"))
            {
                return Err(anyhow::anyhow!(
                    "Required open-profile upstream {} failed readiness: {e}",
                    upstream_config.alias
                ));
            }
            error!(
                "Failed to connect to upstream {}: {}",
                upstream_config.alias, e
            );
        }
    }

    // Rebuild search index
    search_engine.rebuild();

    let mut runtime_config =
        Runtime::with_sandbox_config(registry.clone(), upstream_manager.clone(), sandbox_config);
    if policy_engine.is_open() {
        runtime_config.set_unrestricted();
    }
    let runtime = Arc::new(runtime_config);

    // Create skill store with search engine so incremental updates can touch index.
    let mut skill_store = SkillStore::with_search_engine(
        &paths.skills_root,
        registry.clone(),
        search_engine.clone(),
    )?;
    configure_read_roots(&mut skill_store, read_roots);

    // Load and register skills
    if let Err(e) = skill_store.load_and_register_all().await {
        error!("Failed to load skills: {}", e);
    }

    // Start filesystem watcher to keep registry/search in sync with direct file edits.
    match skill_store.start_watch() {
        Ok(mut reload_rx) => {
            let watch_registry = registry.clone();
            let watch_search = search_engine.clone();
            let watch_root = paths.skills_root.clone();
            let watch_read_roots = read_roots.to_vec();
            tokio::spawn(async move {
                while let Some(skill_path) = reload_rx.recv().await {
                    let mut hot_store = match SkillStore::with_search_engine(
                        &watch_root,
                        watch_registry.clone(),
                        watch_search.clone(),
                    ) {
                        Ok(store) => store,
                        Err(e) => {
                            error!("Hot-reload: failed to create skill store: {}", e);
                            continue;
                        }
                    };

                    configure_read_roots(&mut hot_store, &watch_read_roots);
                    if let Err(error) = hot_store.refresh_directory(&skill_path) {
                        warn!(
                            "Hot-reload: cannot refresh {}: {error}",
                            skill_path.display()
                        );
                    }
                }
            });
        }
        Err(e) => warn!("Failed to start skill watcher: {}", e),
    }

    let skill_store = Arc::new(skill_store);

    // Create MCP server
    let server = SkillsServer::new(registry, search_engine, policy_engine, runtime, skill_store)
        .with_upstream_manager(upstream_manager);

    info!("Server initialized successfully");
    Ok(server)
}

/// Print the stylish SKILLS banner
fn print_banner() {
    eprintln!(
        r"
 ███████╗██╗  ██╗██╗██╗     ██╗     ███████╗
 ██╔════╝██║ ██╔╝██║██║     ██║     ██╔════╝
 ███████╗█████╔╝ ██║██║     ██║     ███████╗
 ╚════██║██╔═██╗ ██║██║     ██║     ╚════██║
 ███████║██║  ██╗██║███████╗███████╗███████║
 ╚══════╝╚═╝  ╚═╝╚═╝╚══════╝╚══════╝╚══════╝
    "
    );
    eprintln!(
        "    Infinite Skills. Finite Context. v{}\n",
        env!("CARGO_PKG_VERSION")
    );
}

async fn wait_for_shutdown() -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => result,
            _ = terminate.recv() => Ok(()),
        }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await
}

fn main() -> Result<ExitCode> {
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("__wasm-worker")) {
        tokio::runtime::Runtime::new()?.block_on(skillsrs::execution::wasm::run_worker())?;
        return Ok(ExitCode::SUCCESS);
    }
    let cli = Cli::parse();
    let checking = matches!(cli.command, Commands::Check);
    let result = tokio::runtime::Runtime::new()
        .context("Cannot initialize Tokio runtime")
        .and_then(|runtime| runtime.block_on(run(cli)));
    match result {
        Err(error) if checking => {
            eprintln!("Error: {error:#}");
            Ok(ExitCode::from(2))
        }
        result => result,
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "the CLI command dispatcher keeps command-specific control flow in one entry point"
)]
async fn run(cli: Cli) -> Result<ExitCode> {
    let initializing = matches!(cli.command, Commands::Init { .. });
    let read_only_setup = initializing
        || matches!(
            cli.command,
            Commands::Check | Commands::Add { list: true, .. }
        );

    let logging = init_logging(cli.log_level.as_deref().unwrap_or("info"));

    let (project_config_path, sys_config_path) = {
        let sys_paths = SkillsPaths::new()?;
        let sys_config = sys_paths.default_config_file();

        let project = if cli.global || cli.config.is_some() {
            None
        } else {
            let cwd = std::env::current_dir()?;

            let mut cursor: Option<&std::path::Path> = Some(cwd.as_path());
            let mut found: Option<std::path::PathBuf> = None;

            while let Some(dir) = cursor {
                let candidate = dir.join(".skills").join("config.yaml");
                if candidate.exists() {
                    found = Some(candidate);
                    break;
                }
                cursor = dir.parent();
            }

            found
        };

        (project, sys_config)
    };

    if !read_only_setup && cli.config.is_none() && !matches!(cli.command, Commands::Add { .. }) {
        let sys_paths = SkillsPaths::new()?;
        ensure_global_config(&sys_paths)?;
    }

    let active_config_path = if let Some(config_file) = &cli.config {
        anyhow::ensure!(
            Path::new(config_file).is_file(),
            "Configuration file not found: {config_file}"
        );
        PathBuf::from(config_file)
    } else {
        project_config_path
            .clone()
            .unwrap_or_else(|| sys_config_path.clone())
    };
    let mut config = if initializing {
        Config::default()
    } else {
        load_config(&active_config_path)?
    };
    let mut writable_config = config.clone();

    if !cli.global
        && cli.config.is_none()
        && project_config_path.is_some()
        && config.use_global.enabled
    {
        let mut base = load_config(&sys_config_path)?;
        merge_config(&mut base, config);
        config = base;
    }
    if cli.log_level.is_none() {
        let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            format!("{}={}", env!("CARGO_CRATE_NAME"), config.server.log_level).into()
        });
        logging.reload(filter)?;
    }
    anyhow::ensure!(
        matches!(config.server.transport.as_str(), "stdio" | "http"),
        "server.transport must be stdio or http"
    );

    let mut paths = resolve_paths(&cli, &config, &active_config_path)?;
    if !read_only_setup {
        paths.ensure_directories()?;
    }

    // Handle --current-dir flag
    if cli.current_dir {
        let cwd = std::env::current_dir()?;
        info!("Using current directory: {}", cwd.display());
        paths.skills_root.clone_from(&cwd);
        paths.data_dir.clone_from(&cwd);
        paths.database_path = cwd.join("skills.db");
    }

    let no_sandbox = cli.no_sandbox || cli.current_dir;
    let read_roots = read_skill_roots(&cli)?;

    match cli.command {
        Commands::Init { force } => {
            let cwd = std::env::current_dir()?;
            let skills_dir = cwd.join(".skills");
            let skills_root = skills_dir.join("skills");
            let config_file = skills_dir.join("config.yaml");

            anyhow::ensure!(
                !config_file.exists() || force,
                "Configuration already exists: {} (use --force to overwrite)",
                config_file.display()
            );

            std::fs::create_dir_all(&skills_root)?;

            let project_config = Config {
                paths: PathsConfig {
                    data_dir: Some(PathBuf::from(".skills")),
                    database_path: Some(PathBuf::from(".skills/skills.db")),
                    ..Default::default()
                },
                ..Default::default()
            };
            save_config(&project_config, &config_file)?;

            eprintln!("Initialized project-local skills.rs configuration:");
            eprintln!("  Config:      {}", config_file.display());
            eprintln!("  Skills root: {}", skills_root.display());
            eprintln!("\nNext:");
            eprintln!("  - Run `skills list` to verify discovery");
            eprintln!("  - Edit `.skills/config.yaml` to add upstream MCP servers");
        }

        Commands::Server { mode } => {
            let mode = mode.unwrap_or_else(|| {
                if config.server.transport == "http" {
                    ServerMode::Http { bind: None }
                } else {
                    ServerMode::Stdio
                }
            });
            match mode {
                ServerMode::Stdio => {
                    print_banner();
                    info!("Starting skills.rs in stdio mode");
                    eprintln!("Mode: stdio");
                    eprintln!("Exposing 3 tools: search, exec, manage");
                    eprintln!("Skills directory: {}", paths.skills_root.display());
                    if no_sandbox {
                        eprintln!("⚠️  Sandboxing: DISABLED");
                    }
                    eprintln!();

                    let server = init_server(&config, &paths, no_sandbox, &read_roots).await?;

                    // Run stdio server
                    let service = server.serve(stdio()).await?;
                    service.waiting().await?;
                }

                ServerMode::Http { bind } => {
                    let bind: std::net::SocketAddr = bind
                        .as_deref()
                        .unwrap_or(&config.server.bind)
                        .parse()
                        .context("HTTP bind must be an IP address and port")?;
                    let policy = PolicyConfig::from_environment(config.policy.clone());
                    let token = http_token(&config.server, bind, &policy)?;
                    print_banner();
                    info!("Starting skills.rs in HTTP mode on {}", bind);
                    eprintln!("Mode: HTTP");
                    eprintln!("Listening on: http://{bind}");
                    eprintln!("MCP Endpoint: http://{bind}/mcp");
                    eprintln!("Exposing 3 tools: search, exec, manage");
                    eprintln!("Skills directory: {}", paths.skills_root.display());
                    if no_sandbox {
                        eprintln!("⚠️  Sandboxing: DISABLED");
                    }
                    eprintln!();

                    let server = init_server(&config, &paths, no_sandbox, &read_roots).await?;

                    let health_server = server.clone();
                    let mcp_service = StreamableHttpService::new(
                        move || Ok(server.clone()),
                        LocalSessionManager::default().into(),
                        StreamableHttpServerConfig::default(),
                    );

                    // Create router
                    let app = axum::Router::new()
                        .route(
                            "/",
                            axum::routing::get(|| async {
                                axum::response::Html(
                                    r#"
                                <!DOCTYPE html>
                                <html>
                                <head><title>skills.rs</title></head>
                                <body>
                                    <h1>skills.rs - Infinite Skills. Finite Context.</h1>
                                    <p>MCP endpoint available at: <a href="/mcp">/mcp</a></p>
                                    <p>Exposes exactly 3 tools:</p>
                                    <ul>
                                        <li><code>search</code> - Discovery over registry</li>
                                        <li><code>exec</code> - Description, validation, and execution</li>
                                        <li><code>manage</code> - Skill lifecycle management</li>
                                    </ul>
                                </body>
                                </html>
                                "#,
                                )
                            }),
                        )
                        .route(
                            "/health",
                            axum::routing::get(move || {
                                let server = health_server.clone();
                                async move {
                                    let profile = server.component_profile().await;
                                    let status = if profile.ready {
                                        axum::http::StatusCode::OK
                                    } else {
                                        axum::http::StatusCode::SERVICE_UNAVAILABLE
                                    };
                                    (status, axum::response::Json(serde_json::json!({
                                        "status": if profile.ready { "healthy" } else { "unready" },
                                        "service": "skills.rs",
                                        "version": env!("CARGO_PKG_VERSION"),
                                        "profile": profile
                                    })))
                                }
                            }),
                        )
                        .nest_service("/mcp", mcp_service);
                    let app = if let Some(token) = token {
                        app.layer(axum::middleware::from_fn(
                            move |request: axum::extract::Request, next: axum::middleware::Next| {
                                let token = token.clone();
                                async move {
                                    if !bearer_matches(
                                        request.headers().get(axum::http::header::AUTHORIZATION),
                                        &token,
                                    ) {
                                        return axum::response::IntoResponse::into_response((
                                            axum::http::StatusCode::UNAUTHORIZED,
                                            [(axum::http::header::WWW_AUTHENTICATE, "Bearer")],
                                            "Authentication required",
                                        ));
                                    }
                                    next.run(request).await
                                }
                            },
                        ))
                    } else {
                        app
                    };

                    // Start server
                    let listener = tokio::net::TcpListener::bind(bind).await?;
                    info!("HTTP server listening on {}", bind);

                    axum::serve(listener, app)
                        .with_graceful_shutdown(async {
                            if let Err(error) = wait_for_shutdown().await {
                                warn!("Failed to listen for shutdown signal: {error}");
                            }
                            info!("Shutting down...");
                        })
                        .await?;
                }
            }
        }

        Commands::List {
            server,
            descriptions,
            json,
        } => {
            let upstreams = server
                .as_deref()
                .map_or(CliUpstreams::All, CliUpstreams::One);
            let components =
                init_cli_components(&config, &paths, &read_roots, no_sandbox, upstreams).await?;
            let mut callables = components.registry.all();
            callables.retain(|record| components.policy.authorize_read(record).allowed);
            callables.sort_by(|left, right| left.fq_name.cmp(&right.fq_name));

            if json {
                // JSON output
                let output: Vec<_> = callables
                    .iter()
                    .filter(|c| {
                        server
                            .as_ref()
                            .is_none_or(|srv| c.server_alias.as_deref() == Some(srv))
                    })
                    .map(|c| {
                        serde_json::json!({
                            "name": c.name,
                            "fq_name": c.fq_name,
                            "kind": format!("{:?}", c.kind),
                            "server": c.server_alias,
                            "description": c.description,
                        })
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&output)?);
            } else {
                // Human-readable output grouped by server
                let mut by_server: std::collections::HashMap<String, Vec<_>> =
                    std::collections::HashMap::new();

                for callable in callables {
                    if let Some(ref srv) = server {
                        if callable.server_alias.as_deref() != Some(srv) {
                            continue;
                        }
                    }
                    let server_name = callable
                        .server_alias
                        .clone()
                        .unwrap_or_else(|| "local".to_string());
                    by_server
                        .entry(server_name)
                        .or_insert_with(Vec::new)
                        .push(callable);
                }

                for (server_name, tools) in &by_server {
                    println!("{server_name}");
                    for tool in tools {
                        if descriptions {
                            println!(
                                " • {} - {}",
                                tool.name,
                                tool.description.as_deref().unwrap_or("")
                            );
                        } else {
                            println!(" • {}", tool.name);
                        }
                    }
                    println!();
                }
            }
        }

        Commands::Tool {
            tool_path,
            args,
            json,
            raw,
            consent,
            timeout_ms,
            input_responses,
            request_state,
        } => {
            run_cli_tool(
                &config,
                &paths,
                &read_roots,
                no_sandbox,
                CliToolRequest {
                    path: tool_path,
                    args,
                    json,
                    raw,
                    describe_if_empty: true,
                    consent,
                    timeout_ms,
                    input_responses,
                    request_state,
                },
            )
            .await?;
        }

        Commands::Execute {
            tool_path,
            args,
            json,
            consent,
            timeout_ms,
            input_responses,
            request_state,
        } => {
            run_cli_tool(
                &config,
                &paths,
                &read_roots,
                no_sandbox,
                CliToolRequest {
                    path: tool_path,
                    args,
                    json,
                    raw: false,
                    describe_if_empty: false,
                    consent,
                    timeout_ms,
                    input_responses,
                    request_state,
                },
            )
            .await?;
        }

        Commands::Grep {
            pattern,
            descriptions,
        } => {
            let name_pattern = glob::Pattern::new(&pattern)?;
            let components =
                init_cli_components(&config, &paths, &read_roots, no_sandbox, CliUpstreams::All)
                    .await?;
            let mut callables = components.registry.all();
            callables.retain(|record| components.policy.authorize_read(record).allowed);
            callables.sort_by(|left, right| left.fq_name.cmp(&right.fq_name));
            let options = glob::MatchOptions {
                case_sensitive: false,
                ..Default::default()
            };

            let matches: Vec<_> = callables
                .iter()
                .filter(|c| {
                    name_pattern.matches_with(&c.name, options)
                        || name_pattern.matches_with(&c.fq_name, options)
                })
                .collect();

            if matches.is_empty() {
                println!("No tools found matching pattern: {pattern}");
            } else {
                println!("Found {} matching tools:\n", matches.len());
                for tool in matches {
                    let server = tool.server_alias.as_deref().unwrap_or("local");
                    if descriptions {
                        println!(
                            "{}/{} - {}",
                            server,
                            tool.name,
                            tool.description.as_deref().unwrap_or("")
                        );
                    } else {
                        println!("{}/{}", server, tool.name);
                    }
                }
            }
        }

        Commands::Validate => {
            PolicyEngine::new(PolicyConfig::from_environment(config.policy.clone()))?;
            let mut aliases = std::collections::HashSet::new();
            for upstream in &config.upstreams {
                anyhow::ensure!(
                    aliases.insert(&upstream.alias),
                    "Duplicate upstream alias: {}",
                    upstream.alias
                );
                anyhow::ensure!(
                    !upstream.alias.trim().is_empty(),
                    "Upstream alias must not be empty"
                );
                match upstream.transport {
                    skillsrs::Transport::Stdio => {
                        anyhow::ensure!(
                            upstream.command.as_ref().is_some_and(|command| command
                                .first()
                                .is_some_and(|program| !program.is_empty())),
                            "Stdio upstream {} needs a command",
                            upstream.alias
                        );
                    }
                    skillsrs::Transport::Http | skillsrs::Transport::HttpSse => {
                        let url = upstream
                            .url
                            .as_deref()
                            .context("HTTP upstream needs a URL")?;
                        validate_http_endpoint(url)?;
                    }
                    _ => {}
                }
            }
            let store = make_store(&paths, Arc::new(Registry::new()), &read_roots)?;
            let validation = store.validate_all().await;
            for warning in &validation.warnings {
                eprintln!("Warning: {warning}");
            }
            for error in &validation.errors {
                eprintln!("Error: {error}");
            }
            anyhow::ensure!(validation.valid, "Skill validation failed");
            eprintln!("Configuration and installed skills are valid");
        }

        Commands::Add {
            repo,
            skill,
            git_ref,
            force,
            list,
        } => {
            let mut repository = AgentSkillsRepoConfig {
                repo: canonical_source(&repo)?,
                git_ref,
                skills: (!skill.is_empty()).then_some(skill),
                alias: None,
            };
            let identity = normalize_source(&repository.repo)?;
            let existing = config
                .agent_skills_repos
                .iter()
                .find(|source| source_matches(source, &identity));
            if let Some(source) = existing {
                repository.repo.clone_from(&source.repo);
                if repository.git_ref.is_none() {
                    repository.git_ref.clone_from(&source.git_ref);
                }
            }
            if list {
                for name in AgentSkillsSync::discover_source(&repository).await? {
                    println!("{name}");
                }
            } else {
                record_source_add(
                    &mut writable_config,
                    existing,
                    repository.clone(),
                    &identity,
                );
                let tracking = serde_yaml::to_string(&writable_config)?;
                let mut sync = AgentSkillsSync::new(&paths.skills_root).await?;
                let report = sync
                    .add_with_tracking(&repository, force, &active_config_path, tracking.as_bytes())
                    .await?;
                for error in &report.errors {
                    eprintln!("Error: {error}");
                }
                anyhow::ensure!(report.errors.is_empty(), "Skill import failed");
                eprintln!("Installed {} package changes", report.total_changes());
            }
        }

        Commands::Sync => {
            let mut sync = AgentSkillsSync::new(&paths.skills_root).await?;
            let report = sync.sync_all(&config.agent_skills_repos).await?;
            for error in &report.errors {
                eprintln!("Error: {error}");
            }
            anyhow::ensure!(report.errors.is_empty(), "Synchronization failed");
            eprintln!(
                "Synchronization complete: {} changes",
                report.total_changes()
            );
        }

        Commands::Check => {
            let sync = AgentSkillsSync::new(&paths.skills_root).await?;
            let report = sync.check_all(&config.agent_skills_repos).await?;
            for error in &report.errors {
                eprintln!("Error: {error}");
            }
            anyhow::ensure!(report.errors.is_empty(), "Update check failed");
            if !report.is_empty() {
                eprintln!(
                    "Updates available: {} package changes",
                    report.total_changes()
                );
                return Ok(ExitCode::from(1));
            }
            eprintln!("Installed sources are up to date");
        }

        Commands::Paths => {
            info!("Displaying system paths");
            eprintln!("skills.rs - System Paths\n");
            eprintln!("{}", paths.display());
            eprintln!("\n✓ All directories exist and are accessible");
            eprintln!("\nEnvironment variable for override:");
            eprintln!(
                "  SKILLS_PATH  - Set root path for all skills data (default: platform-specific)"
            );
            eprintln!("\nDirectory structure:");
            eprintln!("  {{SKILLS_PATH}}/");
            eprintln!("  ├── config.yaml       # Configuration file");
            eprintln!("  ├── skills/           # Skills directory");
            eprintln!("  ├── skills.db         # SQLite database");
            eprintln!("  ├── cache/            # Cache files");
            eprintln!("  └── logs/             # Log files");
        }

        Commands::Search {
            query,
            kind,
            limit,
            cursor,
            json,
        } => {
            anyhow::ensure!(
                (1..=50).contains(&limit),
                "Search limit must be between 1 and 50"
            );
            let upstreams = if kind.as_deref() == Some("skills") {
                CliUpstreams::None
            } else {
                CliUpstreams::All
            };
            let components =
                init_cli_components(&config, &paths, &read_roots, no_sandbox, upstreams).await?;
            let engine = SearchEngine::new(components.registry.clone());
            engine.rebuild();
            let query = skillsrs::SearchQuery {
                q: query,
                kind: kind.unwrap_or_else(|| "any".into()),
                limit,
                filters: None,
                cursor,
            };
            let results = engine.search_visible(&query, |record| {
                components.policy.authorize_read(record).allowed
            })?;
            if json {
                println!("{}", serde_json::to_string_pretty(&results)?);
            } else {
                for result in results.matches {
                    println!("{}\t{}", result.id, result.description_snippet);
                }
                if let Some(cursor) = results.next_cursor {
                    eprintln!("Next cursor: {cursor}");
                }
            }
        }

        Commands::Skill(skill_cmd) => {
            let skill_store = || make_store(&paths, Arc::new(Registry::new()), &read_roots);

            match skill_cmd {
                SkillCommands::Run {
                    skill_id,
                    script,
                    args,
                    consent,
                    timeout_ms,
                } => {
                    let components = init_cli_components(
                        &config,
                        &paths,
                        &read_roots,
                        no_sandbox,
                        CliUpstreams::None,
                    )
                    .await?;
                    let mut record = components
                        .registry
                        .by_kind(CallableKind::Skill)
                        .into_iter()
                        .find(|record| record.id.as_str() == skill_id || record.name == skill_id)
                        .with_context(|| format!("Skill not found: {skill_id}"))?;
                    record.risk_tier = if script.is_some() {
                        skillsrs::RiskTier::Unknown
                    } else {
                        skillsrs::RiskTier::ReadOnly
                    };
                    let arguments =
                        serde_json::from_str(&args).context("Invalid JSON arguments")?;
                    let authorization = components.policy.authorize(
                        &record,
                        &arguments,
                        parse_consent(&consent)?,
                    )?;
                    anyhow::ensure!(
                        authorization.allowed,
                        "Execution denied: {}",
                        authorization.reason
                    );
                    let ctx = skillsrs::ExecContext {
                        callable_id: record.id,
                        arguments,
                        trace_enabled: false,
                        timeout_ms: Some(components.policy.check_timeout(timeout_ms)?),
                    };
                    let result = if let Some(script) = script {
                        components.runtime.execute_script(ctx, &script).await?
                    } else {
                        components.runtime.execute(ctx).await?
                    };
                    println!("{}", serde_json::to_string_pretty(&result)?);
                    anyhow::ensure!(!result.is_error, "Script execution failed");
                }
                SkillCommands::Create {
                    name,
                    description,
                    version,
                    skill_md,
                    content,
                    uses_tools,
                } => {
                    // Determine SKILL.md content
                    let skill_md_content = if let Some(content_str) = content {
                        // Inline content provided
                        if content_str == "-" {
                            // Read from stdin
                            let mut buffer = String::new();
                            std::io::stdin().read_to_string(&mut buffer)?;
                            buffer
                        } else {
                            content_str
                        }
                    } else if let Some(path) = skill_md {
                        // Read from file
                        std::fs::read_to_string(&path)
                            .map_err(|e| anyhow::anyhow!("Failed to read SKILL.md: {e}"))?
                    } else {
                        // Interactive mode - read from stdin with prompt
                        println!("Enter SKILL.md content (Ctrl+D when done):");
                        let mut buffer = String::new();
                        std::io::stdin().read_to_string(&mut buffer)?;
                        buffer
                    };

                    let desc = match description {
                        Some(description) => description,
                        None => skillsrs::agent_skills::parse_frontmatter_public(&skill_md_content)
                            .ok()
                            .and_then(|(metadata, _)| metadata.description)
                            .context(
                                "Provide --description or a SKILL.md with a non-empty description",
                            )?,
                    };

                    let request = skillsrs::storage::CreateSkillRequest {
                        name: name.clone(),
                        version,
                        description: desc,
                        skill_md_content,
                        uses_tools,
                        tags: vec![],
                        scripts: vec![],
                        references: vec![],
                        assets: vec![],
                    };

                    let id = skill_store()?.create_skill(request)?;
                    println!("✓ Created skill: {} ({})", name, id.as_str());
                }

                SkillCommands::Edit {
                    skill_id,
                    name,
                    description,
                    version,
                    skill_md,
                    content,
                    replace,
                    replace_with,
                    append,
                    prepend,
                    uses_tools,
                } => {
                    // Parse skill_id to get skill name
                    let skill_name = if skill_id.starts_with("skill:") {
                        skill_id
                            .strip_prefix("skill:")
                            .and_then(|s| s.split('@').next())
                            .unwrap_or(&skill_id)
                            .to_string()
                    } else {
                        skill_id
                    };

                    let skill_store = skill_store()?;
                    // Load existing skill content
                    let current_content = skill_store.load_skill_content(&skill_name)?;

                    // Determine new SKILL.md content
                    let skill_md_content = if let Some(content_str) = content {
                        // Inline content provided
                        if content_str == "-" {
                            // Read from stdin
                            use std::io::Read;
                            let mut buffer = String::new();
                            std::io::stdin().read_to_string(&mut buffer)?;
                            buffer
                        } else {
                            content_str
                        }
                    } else if let Some(path) = skill_md {
                        // Read from file
                        std::fs::read_to_string(&path)
                            .map_err(|e| anyhow::anyhow!("Failed to read SKILL.md: {e}"))?
                    } else {
                        let (_, mut md) = skillsrs::agent_skills::parse_frontmatter_public(
                            &current_content.skill_md,
                        )?;

                        if let (Some(pattern), Some(replacement)) = (&replace, &replace_with) {
                            md = md.replace(pattern, replacement);
                        }

                        // Apply prepend
                        if let Some(prefix) = prepend {
                            md = format!("{prefix}\n{md}");
                        }

                        // Apply append
                        if let Some(suffix) = append {
                            md = format!("{md}\n{suffix}");
                        }

                        md
                    };

                    let request = skillsrs::storage::UpdateSkillRequest {
                        name,
                        version,
                        description,
                        skill_md_content: Some(skill_md_content),
                        uses_tools: uses_tools.map(|tools| {
                            tools.into_iter().filter(|tool| !tool.is_empty()).collect()
                        }),
                        ..Default::default()
                    };

                    let id = skill_store.update_skill(&skill_name, request)?;
                    println!("✓ Updated skill: {} ({})", skill_name, id.as_str());
                }

                SkillCommands::Delete { skill_id, force } => {
                    if !force {
                        println!("Are you sure you want to delete skill '{skill_id}'? [y/N]");
                        let mut buffer = [0u8; 1];
                        std::io::stdin().read_exact(&mut buffer).ok();
                        if !matches!(buffer.first(), Some(b'y' | b'Y')) {
                            println!("Deletion cancelled.");
                            return Ok(ExitCode::SUCCESS);
                        }
                    }

                    // Parse skill_id to get skill name
                    let skill_name = if skill_id.starts_with("skill:") {
                        skill_id
                            .strip_prefix("skill:")
                            .and_then(|s| s.split('@').next())
                            .unwrap_or(&skill_id)
                            .to_string()
                    } else {
                        skill_id.clone()
                    };

                    skill_store()?.delete_skill(&skill_name)?;
                    println!("✓ Deleted skill: {skill_id}");
                }

                SkillCommands::Show { skill_id, file } => {
                    let skill_name = if skill_id.starts_with("skill:") {
                        skill_id
                            .strip_prefix("skill:")
                            .and_then(|s| s.split('@').next())
                            .unwrap_or(&skill_id)
                            .to_string()
                    } else {
                        skill_id.clone()
                    };
                    let skill_store = skill_store()?;

                    if let Some(filename) = file {
                        let content = skill_store.load_skill_file_bytes(&skill_name, &filename)?;
                        std::io::stdout().lock().write_all(&content)?;
                    } else {
                        let content = skill_store.load_skill_content(&skill_name)?;
                        println!("# Skill: {skill_id}\n");
                        println!("{}", content.skill_md);
                        println!("\n---\n");
                        println!("## Metadata");
                        println!("- Uses tools: {}", content.uses_tools.join(", "));
                        println!(
                            "- Bundled tools: {}",
                            content
                                .bundled_tools
                                .iter()
                                .map(|t| t.name.clone())
                                .collect::<Vec<_>>()
                                .join(", ")
                        );
                        println!(
                            "- Additional files: {}",
                            content.additional_files.join(", ")
                        );
                    }
                }
            }
        }

        Commands::AddMcp {
            transport,
            alias,
            endpoint,
            tags,
            no_test,
            description,
        } => {
            anyhow::ensure!(!alias.trim().is_empty(), "Upstream alias must not be empty");
            anyhow::ensure!(
                !config
                    .upstreams
                    .iter()
                    .any(|upstream| upstream.alias == alias),
                "Upstream '{alias}' already exists"
            );

            let (transport, url, command) = match transport {
                TransportType::Http => {
                    let [url] = endpoint.as_slice() else {
                        anyhow::bail!("HTTP upstream needs exactly one URL");
                    };
                    validate_http_endpoint(url)?;
                    (Transport::Http, endpoint.into_iter().next(), None)
                }
                TransportType::Stdio => {
                    anyhow::ensure!(
                        endpoint.first().is_some_and(|program| !program.is_empty()),
                        "Stdio upstream needs an executable"
                    );
                    (Transport::Stdio, None, Some(endpoint))
                }
            };
            let upstream_config = UpstreamConfig {
                alias,
                required: false,
                transport,
                url,
                command,
                auth: None,
                repo: None,
                git_ref: None,
                skills: None,
                roots: None,
                tags,
                sandbox_config: None,
                description,
            };

            if !no_test {
                let upstream_manager = UpstreamManager::new(Arc::new(Registry::new()));
                let policy = PolicyConfig::from_environment(config.policy.clone());
                upstream_manager
                    .set_sandbox_config(effective_sandbox(
                        &config,
                        no_sandbox || policy.mode == PolicyMode::Open,
                    ))
                    .await;
                if policy.mode == PolicyMode::Open {
                    upstream_manager.set_unrestricted();
                }
                upstream_manager
                    .add_upstream(upstream_config.clone())
                    .await
                    .with_context(|| format!(
                        "Upstream '{}' failed discovery using MCP {MCP_CURRENT_PROTOCOL_VERSION}; configuration unchanged",
                        upstream_config.alias
                    ))?;
                let _ = upstream_manager.disconnect(&upstream_config.alias).await;
            }

            writable_config.upstreams.push(upstream_config);

            save_config(&writable_config, &active_config_path)?;
            eprintln!("Added MCP upstream to {}", active_config_path.display());
            if no_test {
                eprintln!("Connection not tested (--no-test)");
            }
        }

        Commands::EditMcp {
            alias,
            description,
            tags,
            url,
            command,
        } => {
            let mut upstream = config
                .upstreams
                .into_iter()
                .find(|upstream| upstream.alias == alias)
                .with_context(|| format!("Upstream '{alias}' not found"))?;

            if let Some(desc) = description {
                upstream.description = Some(desc);
            }
            if !tags.is_empty() {
                upstream.tags = tags;
            }
            if let Some(new_url) = url {
                anyhow::ensure!(
                    matches!(upstream.transport, Transport::Http | Transport::HttpSse),
                    "--url requires an HTTP upstream"
                );
                validate_http_endpoint(&new_url)?;
                upstream.url = Some(new_url);
            }
            if let Some(command) = command {
                anyhow::ensure!(
                    matches!(upstream.transport, Transport::Stdio),
                    "A command can only be set on a stdio upstream"
                );
                anyhow::ensure!(
                    command.first().is_some_and(|program| !program.is_empty()),
                    "Stdio upstream needs an executable"
                );
                upstream.command = Some(command);
            }
            if let Some(existing) = writable_config
                .upstreams
                .iter_mut()
                .find(|existing| existing.alias == alias)
            {
                *existing = upstream;
            } else {
                writable_config.upstreams.push(upstream);
            }

            save_config(&writable_config, &active_config_path)?;

            eprintln!(
                "✓ Updated MCP server '{}' in {}",
                alias,
                active_config_path.display()
            );
            eprintln!("\nThe changes will take effect after restarting skills.rs:");
            eprintln!("  skills server stdio  # Or restart your MCP client");
        }
    }

    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    #[test]
    fn configuration_merge_preserves_sources_and_explicit_security_defaults() -> anyhow::Result<()>
    {
        let directory = tempfile::tempdir()?;
        let global = directory.path().join("global.yaml");
        let project = directory.path().join("project.yaml");
        std::fs::write(&global, "policy:\n  management_enabled: true\nagent_skills_repos:\n  - repo: https://github.com/example/global\n")?;
        std::fs::write(&project, "use_global:\n  enabled: true\npolicy:\n  management_enabled: false\nagent_skills_repos:\n  - repo: https://github.com/example/project\n")?;
        let mut config = super::load_config(&global)?;
        super::merge_config(&mut config, super::load_config(&project)?);
        anyhow::ensure!(
            !config.policy.management_enabled,
            "Project policy was not applied"
        );
        anyhow::ensure!(
            config
                .agent_skills_repos
                .iter()
                .any(|source| source.repo.ends_with("/global")),
            "Global source was lost during merge"
        );
        anyhow::ensure!(
            config
                .agent_skills_repos
                .iter()
                .any(|source| source.repo.ends_with("/project")),
            "Project source was lost during merge"
        );
        anyhow::ensure!(
            config.agent_skills_repos.len() == 2,
            "Sources were duplicated"
        );
        Ok(())
    }

    #[test]
    fn docker_only_configuration_survives_overlay_and_persistence() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let project = directory.path().join("project.yaml");
        std::fs::write(
            &project,
            "sandbox:\n  docker:\n    image: reviewed-helper:fixed\n    memory_limit: 67108864\n",
        )?;
        let mut config = super::Config::default();
        super::merge_config(&mut config, super::load_config(&project)?);
        super::save_config(&config, &project)?;
        let persisted = super::load_config(&project)?;
        anyhow::ensure!(
            persisted.sandbox.docker.image == "reviewed-helper:fixed",
            "Docker image override was lost"
        );
        anyhow::ensure!(
            persisted.sandbox.docker.memory_limit == 67_108_864,
            "Docker memory override was lost"
        );
        Ok(())
    }

    #[test]
    fn unauthenticated_http_cannot_enable_management_or_execution() -> anyhow::Result<()> {
        let server = super::ServerConfig::default();
        let address = "127.0.0.1:18744".parse()?;
        anyhow::ensure!(
            super::http_token(&server, address, &skillsrs::PolicyConfig::default())?.is_none(),
            "Read-only loopback unexpectedly requires a token"
        );
        for policy in [
            skillsrs::PolicyConfig {
                management_enabled: true,
                ..Default::default()
            },
            skillsrs::PolicyConfig {
                trusted_servers: vec!["upstream".into()],
                ..Default::default()
            },
            skillsrs::PolicyConfig {
                require_consent_for: vec![],
                ..Default::default()
            },
            skillsrs::PolicyConfig::open(),
        ] {
            anyhow::ensure!(
                super::http_token(&server, address, &policy).is_err(),
                "Executable HTTP profile accepted missing authentication"
            );
        }
        anyhow::ensure!(
            super::http_token(
                &server,
                "0.0.0.0:18744".parse()?,
                &skillsrs::PolicyConfig::default()
            )
            .is_err(),
            "Remote bind accepted missing authentication"
        );
        Ok(())
    }
}
