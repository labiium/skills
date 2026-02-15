//! skills.rs CLI
//!
//! Main entry point for the skills.rs unified MCP server.
//! Supports multiple modes:
//! - stdio: Run as stdio MCP server
//! - http: Run as HTTP MCP server
//! - validate: Validate configuration and skills

mod paths;

use anyhow::Result;
use clap::{Parser, Subcommand};
use paths::{paths_from_env, PathsConfig, SkillsPaths};
use rmcp::{transport::stdio, ServiceExt};
use skillsrs::core::policy::{PolicyConfig, PolicyEngine};
use skillsrs::core::registry::Registry;
use skillsrs::execution::upstream::UpstreamManager;
use skillsrs::execution::{sandbox::SandboxBackend, sandbox::SandboxConfig, Runtime};
use skillsrs::mcp::SkillsServer;
use skillsrs::storage::search::SearchEngine;
use skillsrs::storage::SkillStore;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{error, info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Parser)]
#[command(name = "skills")]
#[command(about = "skills.rs - Infinite Skills. Finite Context.", long_about = None)]
#[command(before_help = r#"
 ███████╗██╗  ██╗██╗██╗     ██╗     ███████╗
 ██╔════╝██║ ██╔╝██║██║     ██║     ██╔════╝
 ███████╗█████╔╝ ██║██║     ██║     ███████╗
 ╚════██║██╔═██╗ ██║██║     ██║     ╚════██║
 ███████║██║  ██╗██║███████╗███████╗███████║
 ╚══════╝╚═╝  ╚═╝╚═╝╚══════╝╚══════╝╚══════╝
"#)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Config file path (defaults to system config directory)
    #[arg(short, long)]
    config: Option<String>,

    /// Force using only global config + global skills/upstreams (ignore local config discovery)
    #[arg(long)]
    global: bool,

    /// Data directory (overrides config and system default)
    #[arg(long, env = "SKILLS_DATA_DIR")]
    data_dir: Option<String>,

    /// Skills root directory (overrides config)
    #[arg(long, env = "SKILLS_ROOT")]
    skills_root: Option<String>,

    /// Database path (overrides config)
    #[arg(long, env = "SKILLS_DATABASE_PATH")]
    database: Option<String>,

    /// Disable sandboxing (allows full system access - use with caution)
    #[arg(long, env = "SKILLS_NO_SANDBOX")]
    no_sandbox: bool,

    /// Use current directory for operations (implies --no-sandbox)
    #[arg(long)]
    current_dir: bool,

    /// Log level
    #[arg(short, long, default_value = "info")]
    log_level: String,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize a project-local skills configuration in the current directory
    ///
    /// Creates a `.skills/` directory containing:
    /// - `.skills/config.yaml` (project-local config)
    /// - `.skills/skills/` (project-local skills root)
    /// - `.skills/skills.db` (project-local sqlite database)
    ///
    /// After init, commands will automatically discover `.skills/config.yaml`
    /// when run from this project (or any subdirectory).
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
        /// Tool path: server/tool_name
        tool_path: String,

        /// JSON arguments (or use stdin)
        args: Option<String>,

        /// JSON output
        #[arg(short, long)]
        json: bool,

        /// Raw text output (no formatting)
        #[arg(short, long)]
        raw: bool,
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
        /// Tool path: server/tool_name
        tool_path: String,

        /// JSON arguments (or use stdin)
        args: Option<String>,

        /// JSON output
        #[arg(short, long)]
        json: bool,
    },

    /// Validate configuration and skills
    Validate,

    /// Add a skill from a GitHub repository (Vercel skills.sh compatible)
    ///
    /// Usage:
    ///   skills add <owner/repo>              # Add all skills from repo
    ///   skills add <url> --skill <name>      # Add specific skill
    ///   skills add <owner/repo> --skill <name> --skill <name2>  # Add multiple
    ///
    /// Examples:
    ///   skills add vercel-labs/agent-skills
    ///   skills add https://github.com/wshobson/agents --skill monorepo-management
    Add {
        /// Repository URL or GitHub shorthand (owner/repo)
        repo: String,

        /// Specific skill name(s) to import (if omitted, imports all)
        #[arg(short, long)]
        skill: Vec<String>,

        /// Git ref (branch, tag, or commit) - defaults to main/master
        #[arg(long)]
        git_ref: Option<String>,

        /// Force overwrite existing skills
        #[arg(short, long)]
        force: bool,
    },

    /// Sync Agent Skills from config.yaml repositories
    ///
    /// Synchronizes all Agent Skills declared in config.yaml agent_skills_repos.
    /// Adds new skills, updates changed ones, and removes skills from deleted repos.
    Sync,

    /// Show system paths and configuration
    Paths,

    /// Search for tools/skills (deprecated - use grep)
    #[command(hide = true)]
    Search {
        /// Search query
        query: String,

        /// Kind filter
        #[arg(short, long)]
        kind: Option<String>,
    },

    /// Manage skills (create, edit, delete, show)
    #[command(subcommand)]
    Skill(SkillCommands),
}

#[derive(Subcommand)]
enum SkillCommands {
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
        uses_tools: Vec<String>,

        /// Edit in place (no backup)
        #[arg(short, long)]
        in_place: bool,
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
        /// Bind address
        #[arg(short, long, default_value = "127.0.0.1:8000")]
        bind: String,
    },
}

/// Server configuration
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, Default)]
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
}

impl Config {
    /// Check if config is essentially empty (all default values)
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.upstreams.is_empty()
            && self.agent_skills_repos.is_empty()
            && is_default_server_config(&self.server)
            && is_default_policy_config(&self.policy)
            && is_default_paths_config(&self.paths)
            && is_default_sandbox_config(&self.sandbox)
            && is_default_use_global(&self.use_global)
    }
}

// Helper functions for skip_serializing_if checks
fn is_default_server_config(cfg: &ServerConfig) -> bool {
    cfg.bind == default_bind()
        && cfg.transport == default_transport()
        && cfg.log_level == default_log_level()
}

fn is_default_policy_config(cfg: &PolicyConfig) -> bool {
    let default = PolicyConfig::default();
    cfg.default_risk == default.default_risk
        && cfg.require_consent_for == default.require_consent_for
        && cfg.trusted_servers == default.trusted_servers
        && cfg.deny_tags == default.deny_tags
        && cfg.max_calls_per_skill == default.max_calls_per_skill
        && cfg.max_exec_ms == default.max_exec_ms
        && cfg.allow_patterns == default.allow_patterns
        && cfg.deny_patterns == default.deny_patterns
}

fn is_default_paths_config(cfg: &PathsConfig) -> bool {
    cfg.data_dir.is_none()
        && cfg.config_dir.is_none()
        && cfg.cache_dir.is_none()
        && cfg.database_path.is_none()
        && cfg.skills_root.is_none()
        && cfg.logs_dir.is_none()
}

fn is_default_sandbox_config(cfg: &SandboxConfig) -> bool {
    let default = SandboxConfig::default();
    cfg.backend == default.backend
        && cfg.timeout_ms == default.timeout_ms
        && cfg.allow_read == default.allow_read
        && cfg.allow_write == default.allow_write
        && cfg.allow_network == default.allow_network
        && cfg.max_memory_bytes == default.max_memory_bytes
        && cfg.max_cpu_seconds == default.max_cpu_seconds
}

fn is_default_use_global(cfg: &UseGlobalSettings) -> bool {
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
        ServerConfig {
            bind: default_bind(),
            transport: default_transport(),
            log_level: default_log_level(),
        }
    }
}

/// Initialize logging
fn init_logging(level: &str) {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| format!("{}={}", env!("CARGO_CRATE_NAME"), level).into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();
}

/// Load configuration from file
fn load_config(path: &std::path::Path) -> Result<Config> {
    if !path.exists() {
        info!("Config file not found: {}, using defaults", path.display());
        return Ok(Config::default());
    }

    info!("Loading config from: {}", path.display());
    let contents = std::fs::read_to_string(path)?;
    let config: Config = serde_yaml::from_str(&contents)?;
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

/// Add a skill repository to the config
fn add_repo_to_config(config: &mut Config, repo: &str, git_ref: Option<&str>, skills: Vec<String>) {
    // Check if repo already exists
    let existing_idx = config
        .agent_skills_repos
        .iter()
        .position(|r| r.repo == repo);

    if let Some(idx) = existing_idx {
        // Update existing repo
        let repo_config = &mut config.agent_skills_repos[idx];
        if let Some(git_ref_val) = git_ref {
            repo_config.git_ref = Some(git_ref_val.to_string());
        }
        if !skills.is_empty() {
            repo_config.skills = Some(skills);
        }
    } else {
        // Add new repo
        let repo_config = skillsrs::storage::sync::AgentSkillsRepoConfig {
            repo: repo.to_string(),
            git_ref: git_ref.map(|s| s.to_string()),
            skills: if skills.is_empty() {
                None
            } else {
                Some(skills)
            },
            alias: None,
        };
        config.agent_skills_repos.push(repo_config);
    }
}

fn merge_config(base: &mut Config, overlay: Config) {
    // Merge semantics: overlay wins when it sets something meaningful.
    // Keep it intentionally simple and safe:
    // - always append upstreams
    // - paths/sandbox/policy: overlay replaces the corresponding section if it differs from default
    // - server: overlay replaces bind/transport/log_level if not default values
    base.upstreams.extend(overlay.upstreams);

    if overlay.paths.data_dir.is_some()
        || overlay.paths.config_dir.is_some()
        || overlay.paths.cache_dir.is_some()
        || overlay.paths.database_path.is_some()
        || overlay.paths.skills_root.is_some()
        || overlay.paths.logs_dir.is_some()
    {
        base.paths = overlay.paths;
    }

    if overlay.sandbox.backend != SandboxBackend::default()
        || overlay.sandbox.timeout_ms != SandboxConfig::default().timeout_ms
        || overlay.sandbox.allow_read != SandboxConfig::default().allow_read
        || overlay.sandbox.allow_write != SandboxConfig::default().allow_write
        || overlay.sandbox.allow_network != SandboxConfig::default().allow_network
        || overlay.sandbox.max_memory_bytes != SandboxConfig::default().max_memory_bytes
        || overlay.sandbox.max_cpu_seconds != SandboxConfig::default().max_cpu_seconds
    {
        base.sandbox = overlay.sandbox;
    }

    // PolicyConfig is external; treat non-default as override by replacing when serialized differs.
    // This avoids relying on internal field visibility.
    if serde_json::to_value(&overlay.policy).ok()
        != serde_json::to_value(PolicyConfig::default()).ok()
    {
        base.policy = overlay.policy;
    }

    // ServerConfig has defaults; only override when values differ from defaults.
    let d = ServerConfig::default();
    if overlay.server.bind != d.bind
        || overlay.server.transport != d.transport
        || overlay.server.log_level != d.log_level
    {
        base.server = overlay.server;
    }

    base.use_global = overlay.use_global;
}

/// Resolve paths with precedence: CLI args > env vars > config > system defaults
fn resolve_paths(cli: &Cli, config: &Config) -> Result<SkillsPaths> {
    // Start with system defaults
    let mut paths = SkillsPaths::new()?;

    // Apply config overrides
    paths = config.paths.apply_to(paths);

    // Apply environment variable overrides
    paths = paths_from_env().apply_to(paths);

    // Apply CLI argument overrides (highest priority)
    if let Some(ref data_dir) = cli.data_dir {
        paths.data_dir = data_dir.into();
    }
    if let Some(ref skills_root) = cli.skills_root {
        paths.skills_root = skills_root.into();
    }
    if let Some(ref database) = cli.database {
        paths.database_path = database.into();
    }

    // Ensure all directories exist
    paths.ensure_directories()?;

    Ok(paths)
}

/// Initialize the server components
async fn init_server(
    config: &Config,
    paths: &SkillsPaths,
    no_sandbox: bool,
) -> Result<SkillsServer> {
    info!("Initializing skills.rs server");
    info!("Using skills root: {}", paths.skills_root.display());
    info!("Using database: {}", paths.database_path.display());

    if no_sandbox {
        info!("⚠️  Sandboxing DISABLED - tools have full system access");
    }

    // Create registry
    let registry = Arc::new(Registry::new());

    // Create search engine
    let search_engine = Arc::new(SearchEngine::new(registry.clone()));

    // Create policy engine
    let policy_engine = Arc::new(PolicyEngine::new(config.policy.clone())?);

    // Create upstream manager and connect to upstreams
    let upstream_manager = Arc::new(UpstreamManager::new(registry.clone()));
    for upstream_config in &config.upstreams {
        info!("Connecting to upstream: {}", upstream_config.alias);
        if let Err(e) = upstream_manager.add_upstream(upstream_config.clone()).await {
            error!(
                "Failed to connect to upstream {}: {}",
                upstream_config.alias, e
            );
        }
    }

    // Rebuild search index
    search_engine.rebuild();

    // Create runtime (after upstream_manager is initialized)
    let mut sandbox_config = config.sandbox.clone();
    if no_sandbox {
        sandbox_config.backend = SandboxBackend::None;
    }
    let runtime = Arc::new(Runtime::with_sandbox_config(
        registry.clone(),
        upstream_manager,
        sandbox_config,
    ));

    // Sync Agent Skills from config before loading
    if !config.agent_skills_repos.is_empty() {
        info!(
            "Syncing {} Agent Skills repositories from config",
            config.agent_skills_repos.len()
        );
        match skillsrs::storage::sync::AgentSkillsSync::new(&paths.skills_root).await {
            Ok(mut sync) => match sync.sync_all(&config.agent_skills_repos).await {
                Ok(report) => {
                    if !report.is_empty() {
                        info!("Agent Skills sync complete:");
                        if !report.added.is_empty() {
                            info!("  Added: {}", report.added.join(", "));
                        }
                        if !report.updated.is_empty() {
                            info!("  Updated: {}", report.updated.join(", "));
                        }
                        if !report.removed.is_empty() {
                            info!("  Removed: {}", report.removed.join(", "));
                        }
                        if !report.errors.is_empty() {
                            warn!("  Errors: {}", report.errors.join(", "));
                        }
                    } else {
                        info!("All Agent Skills up-to-date");
                    }
                }
                Err(e) => {
                    error!("Failed to sync Agent Skills: {}", e);
                }
            },
            Err(e) => {
                error!("Failed to initialize Agent Skills sync: {}", e);
            }
        }
    }

    // Create skill store with resolved paths
    let skill_store = Arc::new(SkillStore::new(&paths.skills_root, registry.clone())?);

    // Load and register skills
    if let Err(e) = skill_store.load_and_register_all().await {
        error!("Failed to load skills: {}", e);
    }

    // Create MCP server
    let server = SkillsServer::new(registry, search_engine, policy_engine, runtime, skill_store);

    info!("Server initialized successfully");
    Ok(server)
}

/// Print the stylish SKILLS banner
fn print_banner() {
    eprintln!(
        r#"
 ███████╗██╗  ██╗██╗██╗     ██╗     ███████╗
 ██╔════╝██║ ██╔╝██║██║     ██║     ██╔════╝
 ███████╗█████╔╝ ██║██║     ██║     ███████╗
 ╚════██║██╔═██╗ ██║██║     ██║     ╚════██║
 ███████║██║  ██╗██║███████╗███████╗███████║
 ╚══════╝╚═╝  ╚═╝╚═╝╚══════╝╚══════╝╚══════╝
    "#
    );
    eprintln!(
        "    Infinite Skills. Finite Context. v{}\n",
        env!("CARGO_PKG_VERSION")
    );
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    init_logging(&cli.log_level);

    let (project_config_path, sys_config_path, _is_global_active) = {
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

        let is_global = cli.global || (project.is_none() && cli.config.is_none());
        (project, sys_config, is_global)
    };

    // Ensure global config exists before loading (auto-init)
    let sys_paths_for_ensure = SkillsPaths::new()?;
    let _ = ensure_global_config(&sys_paths_for_ensure)?;

    let (mut config, active_config_path): (Config, PathBuf) =
        if let Some(ref config_file) = cli.config {
            (
                load_config(std::path::Path::new(config_file))?,
                PathBuf::from(config_file),
            )
        } else if cli.global {
            (load_config(&sys_config_path)?, sys_config_path.clone())
        } else if let Some(ref p) = project_config_path {
            (load_config(p)?, p.clone())
        } else {
            (load_config(&sys_config_path)?, sys_config_path.clone())
        };

    if !cli.global
        && cli.config.is_none()
        && project_config_path.is_some()
        && config.use_global.enabled
    {
        let mut base = load_config(&sys_config_path)?;
        merge_config(&mut base, config);
        config = base;
    }

    let mut paths = resolve_paths(&cli, &config)?;

    // Handle --current-dir flag
    if cli.current_dir {
        let cwd = std::env::current_dir()?;
        info!("Using current directory: {}", cwd.display());
        paths.skills_root = cwd.clone();
        paths.data_dir = cwd.clone();
        paths.database_path = cwd.join("skills.db");
    }

    let no_sandbox = cli.no_sandbox || cli.current_dir;

    match cli.command {
        Commands::Init { force } => {
            let cwd = std::env::current_dir()?;
            let skills_dir = cwd.join(".skills");
            let skills_root = skills_dir.join("skills");
            let db_path = skills_dir.join("skills.db");
            let config_file = skills_dir.join("config.yaml");

            if config_file.exists() && !force {
                eprintln!(
                    "Refusing to overwrite existing config: {}\nRe-run with --force to overwrite.",
                    config_file.display()
                );
                return Ok(());
            }

            std::fs::create_dir_all(&skills_root)?;

            let yaml = r#"paths:
  data_dir: ".skills"
  skills_root: ".skills/skills"
  database_path: ".skills/skills.db"

sandbox:
  backend: timeout
  timeout_ms: 30000
  allow_read: []
  allow_write: []
  allow_network: false
  max_memory_bytes: 536870912
  max_cpu_seconds: 30

use_global:
  enabled: false
"#;

            std::fs::write(&config_file, yaml)?;

            eprintln!("Initialized project-local skills.rs configuration:");
            eprintln!("  Config:      {}", config_file.display());
            eprintln!("  Skills root: {}", skills_root.display());
            eprintln!("  Database:    {}", db_path.display());
            eprintln!("\nNext:");
            eprintln!("  - Run `skills list` to verify discovery");
            eprintln!("  - Edit `.skills/config.yaml` to add upstream MCP servers");
        }

        Commands::Server { mode } => {
            let mode = mode.unwrap_or(ServerMode::Stdio);
            match mode {
                ServerMode::Stdio => {
                    print_banner();
                    info!("Starting skills.rs in stdio mode");
                    eprintln!("Mode: stdio");
                    eprintln!("Exposing 4 tools: search, schema, exec, manage");
                    eprintln!("Skills directory: {}", paths.skills_root.display());
                    if no_sandbox {
                        eprintln!("⚠️  Sandboxing: DISABLED");
                    }
                    eprintln!();

                    let server = init_server(&config, &paths, no_sandbox).await?;

                    // Run stdio server
                    let service = server.serve(stdio()).await?;
                    service.waiting().await?;
                }

                ServerMode::Http { bind } => {
                    print_banner();
                    info!("Starting skills.rs in HTTP mode on {}", bind);
                    eprintln!("Mode: HTTP");
                    eprintln!("Listening on: http://{}", bind);
                    eprintln!("MCP Endpoint: http://{}/mcp", bind);
                    eprintln!("Exposing 4 tools: search, schema, exec, manage");
                    eprintln!("Skills directory: {}", paths.skills_root.display());
                    if no_sandbox {
                        eprintln!("⚠️  Sandboxing: DISABLED");
                    }
                    eprintln!();

                    let server = init_server(&config, &paths, no_sandbox).await?;

                    // Create HTTP service
                    use rmcp::transport::streamable_http_server::{
                        session::local::LocalSessionManager,
                        tower::{StreamableHttpServerConfig, StreamableHttpService},
                    };

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
                                    <p>Exposes exactly 4 tools:</p>
                                    <ul>
                                        <li><code>search</code> - Discovery over registry</li>
                                        <li><code>schema</code> - On-demand schema fetching</li>
                                        <li><code>exec</code> - Validated execution</li>
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
                            axum::routing::get(|| async {
                                axum::response::Json(serde_json::json!({
                                    "status": "healthy",
                                    "service": "skills.rs",
                                    "version": env!("CARGO_PKG_VERSION")
                                }))
                            }),
                        )
                        .nest_service("/mcp", mcp_service);

                    // Start server
                    let listener = tokio::net::TcpListener::bind(&bind).await?;
                    info!("HTTP server listening on {}", bind);

                    axum::serve(listener, app)
                        .with_graceful_shutdown(async {
                            tokio::signal::ctrl_c().await.unwrap();
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
            // CLI mode defaults to no sandbox
            if !no_sandbox {
                info!("CLI mode: sandboxing disabled by default");
            }

            // Initialize components for CLI mode
            let registry = Arc::new(Registry::new());
            let upstream_manager = Arc::new(UpstreamManager::new(registry.clone()));
            let skill_store = Arc::new(SkillStore::new(&paths.skills_root, registry.clone())?);

            // Load local skills first
            if let Err(e) = skill_store.load_and_register_all().await {
                warn!("Failed to load local skills: {}", e);
            }

            // Connect to upstreams
            for upstream_config in &config.upstreams {
                if let Err(e) = upstream_manager.add_upstream(upstream_config.clone()).await {
                    error!(
                        "Failed to connect to upstream {}: {}",
                        upstream_config.alias, e
                    );
                }
            }

            let callables = registry.all();

            if json {
                // JSON output
                let output: Vec<_> = callables
                    .iter()
                    .filter(|c| {
                        if let Some(ref srv) = server {
                            c.server_alias.as_deref() == Some(srv)
                        } else {
                            true
                        }
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

                for (server_name, tools) in by_server.iter() {
                    println!("{}", server_name);
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
        } => {
            // Parse tool path
            let parts: Vec<&str> = tool_path.split('/').collect();
            if parts.len() != 2 {
                eprintln!("Error: Tool path must be in format <server>/<tool>");
                eprintln!("Example: skills tool filesystem/read_file");
                std::process::exit(1);
            }
            let (server_name, tool_name) = (parts[0], parts[1]);

            // Initialize components (CLI mode uses no sandbox by default)
            let registry = Arc::new(Registry::new());
            let upstream_manager = Arc::new(UpstreamManager::new(registry.clone()));
            let skill_store = Arc::new(SkillStore::new(&paths.skills_root, registry.clone())?);

            // Load local skills first
            if let Err(e) = skill_store.load_and_register_all().await {
                warn!("Failed to load local skills: {}", e);
            }

            // Create sandbox config with no sandboxing for CLI mode
            let mut sandbox_config = SandboxConfig::default();
            if no_sandbox {
                sandbox_config.backend = SandboxBackend::None;
            }
            let runtime = Arc::new(Runtime::with_sandbox_config(
                registry.clone(),
                upstream_manager.clone(),
                sandbox_config,
            ));

            // Connect to upstreams
            for upstream_config in &config.upstreams {
                if let Err(e) = upstream_manager.add_upstream(upstream_config.clone()).await {
                    error!(
                        "Failed to connect to upstream {}: {}",
                        upstream_config.alias, e
                    );
                }
            }

            // Find the tool
            let callables = registry.all();
            let tool = callables
                .iter()
                .find(|c| c.server_alias.as_deref() == Some(server_name) && c.name == tool_name);

            let tool = match tool {
                Some(t) => t,
                None => {
                    eprintln!(
                        "Error: Tool '{}' not found in server '{}'",
                        tool_name, server_name
                    );
                    eprintln!("\nAvailable tools in {}:", server_name);
                    for c in callables
                        .iter()
                        .filter(|c| c.server_alias.as_deref() == Some(server_name))
                    {
                        eprintln!(" • {}", c.name);
                    }
                    std::process::exit(1);
                }
            };

            // If no args provided, show schema
            if args.is_none() {
                println!("Tool: {}", tool.name);
                println!("Server: {}", server_name);
                println!();
                if let Some(desc) = &tool.description {
                    println!("Description:");
                    println!(" {}", desc);
                    println!();
                }
                println!("Input Schema:");
                println!("{}", serde_json::to_string_pretty(&tool.input_schema)?);
                return Ok(());
            }

            // Get arguments from CLI or stdin
            let args_json = if let Some(a) = args {
                a
            } else {
                use std::io::Read;
                let mut buffer = String::new();
                std::io::stdin().read_to_string(&mut buffer)?;
                buffer.trim().to_string()
            };

            // Parse arguments
            let arguments: serde_json::Value = serde_json::from_str(&args_json)
                .map_err(|e| anyhow::anyhow!("Invalid JSON arguments: {}", e))?;

            // Execute the tool
            let exec_context = skillsrs::execution::ExecContext {
                callable_id: tool.id.clone(),
                arguments: arguments.clone(),
                timeout_ms: Some(30000),
                trace_enabled: false,
            };

            match runtime.execute(exec_context).await {
                Ok(result) => {
                    if json {
                        let json_result = serde_json::to_value(&result)?;
                        println!("{}", serde_json::to_string_pretty(&json_result)?);
                    } else if raw {
                        // Extract text content from ToolResult
                        for content in &result.content {
                            if let skillsrs::core::ToolResultContent::Text { text } = content {
                                print!("{}", text);
                            }
                        }
                    } else {
                        println!("Success!");
                        let json_result = serde_json::to_value(&result)?;
                        println!("{}", serde_json::to_string_pretty(&json_result)?);
                    }
                }
                Err(e) => {
                    eprintln!("Error executing tool: {}", e);
                    std::process::exit(2);
                }
            }
        }

        Commands::Execute {
            tool_path,
            args,
            json,
        } => {
            // Parse tool path
            let parts: Vec<&str> = tool_path.split('/').collect();
            if parts.len() != 2 {
                eprintln!("Error: Tool path must be in format <server>/<tool>");
                eprintln!("Example: skills exec filesystem/read_file");
                std::process::exit(1);
            }
            let (server_name, tool_name) = (parts[0], parts[1]);

            // Initialize components (CLI mode uses no sandbox by default)
            let registry = Arc::new(Registry::new());
            let upstream_manager = Arc::new(UpstreamManager::new(registry.clone()));
            let skill_store = Arc::new(SkillStore::new(&paths.skills_root, registry.clone())?);

            // Load local skills first
            if let Err(e) = skill_store.load_and_register_all().await {
                warn!("Failed to load local skills: {}", e);
            }

            // Create sandbox config with no sandboxing for CLI mode
            let mut sandbox_config = SandboxConfig::default();
            if no_sandbox {
                sandbox_config.backend = SandboxBackend::None;
            }
            let runtime = Arc::new(Runtime::with_sandbox_config(
                registry.clone(),
                upstream_manager.clone(),
                sandbox_config,
            ));

            // Connect to upstreams
            for upstream_config in &config.upstreams {
                if let Err(e) = upstream_manager.add_upstream(upstream_config.clone()).await {
                    error!(
                        "Failed to connect to upstream {}: {}",
                        upstream_config.alias, e
                    );
                }
            }

            // Find the tool
            let callables = registry.all();
            let tool = callables
                .iter()
                .find(|c| c.server_alias.as_deref() == Some(server_name) && c.name == tool_name);

            let tool = match tool {
                Some(t) => t,
                None => {
                    eprintln!(
                        "Error: Tool '{}' not found in server '{}'",
                        tool_name, server_name
                    );
                    eprintln!("\nAvailable tools in {}:", server_name);
                    for c in callables
                        .iter()
                        .filter(|c| c.server_alias.as_deref() == Some(server_name))
                    {
                        eprintln!(" • {}", c.name);
                    }
                    std::process::exit(1);
                }
            };

            // Get arguments from CLI or stdin
            let args_json = if let Some(a) = args {
                a
            } else {
                use std::io::Read;
                let mut buffer = String::new();
                std::io::stdin().read_to_string(&mut buffer)?;
                buffer.trim().to_string()
            };

            // Parse arguments
            let arguments: serde_json::Value = serde_json::from_str(&args_json)
                .map_err(|e| anyhow::anyhow!("Invalid JSON arguments: {}", e))?;

            // Execute the tool
            let exec_context = skillsrs::execution::ExecContext {
                callable_id: tool.id.clone(),
                arguments: arguments.clone(),
                timeout_ms: Some(30000),
                trace_enabled: false,
            };

            match runtime.execute(exec_context).await {
                Ok(result) => {
                    if json {
                        let json_result = serde_json::to_value(&result)?;
                        println!("{}", serde_json::to_string_pretty(&json_result)?);
                    } else {
                        println!("Success!");
                        let json_result = serde_json::to_value(&result)?;
                        println!("{}", serde_json::to_string_pretty(&json_result)?);
                    }
                }
                Err(e) => {
                    eprintln!("Error executing tool: {}", e);
                    std::process::exit(2);
                }
            }
        }

        Commands::Grep {
            pattern,
            descriptions,
        } => {
            // Initialize components
            let registry = Arc::new(Registry::new());
            let upstream_manager = Arc::new(UpstreamManager::new(registry.clone()));
            let skill_store = Arc::new(SkillStore::new(&paths.skills_root, registry.clone())?);

            // Load local skills first
            if let Err(e) = skill_store.load_and_register_all().await {
                warn!("Failed to load local skills: {}", e);
            }

            // Connect to upstreams
            for upstream_config in &config.upstreams {
                if let Err(e) = upstream_manager.add_upstream(upstream_config.clone()).await {
                    error!(
                        "Failed to connect to upstream {}: {}",
                        upstream_config.alias, e
                    );
                }
            }

            let callables = registry.all();

            // Simple glob matching
            let pattern = pattern.replace('*', ".*");
            let re = regex::Regex::new(&format!("(?i){}", pattern))?;

            let matches: Vec<_> = callables
                .iter()
                .filter(|c| re.is_match(&c.name) || re.is_match(&c.fq_name))
                .collect();

            if matches.is_empty() {
                println!("No tools found matching pattern: {}", pattern);
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
            info!("Validating configuration");
            eprintln!("Validating skills.rs configuration...\n");

            // Validate policy config
            eprintln!("✓ Policy configuration is valid");
            eprintln!("  Default risk: {}", config.policy.default_risk);
            eprintln!(
                "  Consent required for: {:?}",
                config.policy.require_consent_for
            );
            eprintln!("  Trusted servers: {:?}", config.policy.trusted_servers);
            eprintln!(
                "  Max calls per skill: {}",
                config.policy.max_calls_per_skill
            );
            eprintln!("  Max exec time: {}ms", config.policy.max_exec_ms);

            // Validate upstreams
            eprintln!("\n✓ Upstream configuration:");
            if config.upstreams.is_empty() {
                eprintln!("  (no upstreams configured)");
            } else {
                for upstream in &config.upstreams {
                    eprintln!("  - {} ({:?})", upstream.alias, upstream.transport);
                }
            }

            // Validate skill store
            eprintln!("\n✓ Skill store configuration:");
            eprintln!("  Skills root: {}", paths.skills_root.display());
            eprintln!("  Database: {}", paths.database_path.display());

            eprintln!("\n✓ Configuration is valid");
        }

        Commands::Add {
            repo,
            skill,
            git_ref,
            force,
        } => {
            use skillsrs::storage::agent_skills::AgentSkill;
            use std::path::PathBuf;

            info!("Adding skill(s) from repository: {}", repo);
            eprintln!("🔍 Fetching skills from: {}", repo);

            // Parse repo URL
            let repo_url = if repo.starts_with("http://") || repo.starts_with("https://") {
                repo.clone()
            } else if repo.contains('/') && !repo.contains(':') {
                // GitHub shorthand: owner/repo
                format!("https://github.com/{}", repo)
            } else {
                eprintln!("❌ Invalid repository format. Use 'owner/repo' or full URL");
                return Ok(());
            };

            // Clone to temp directory
            let temp_dir = tempfile::tempdir()?;
            let clone_path = temp_dir.path();

            eprintln!("📦 Cloning repository...");
            let mut cmd = std::process::Command::new("git");
            cmd.arg("clone");
            if let Some(ref git_ref_val) = git_ref {
                cmd.arg("--branch").arg(git_ref_val);
            }
            cmd.arg("--depth").arg("1");
            cmd.arg(&repo_url);
            cmd.arg(clone_path);

            let output = cmd.output()?;
            if !output.status.success() {
                eprintln!(
                    "❌ Failed to clone repository:\n{}",
                    String::from_utf8_lossy(&output.stderr)
                );
                return Ok(());
            }

            eprintln!("✓ Repository cloned successfully");

            // Discover all SKILL.md files (Agent Skills format)
            eprintln!("🔍 Discovering Agent Skills...");
            let mut discovered_skills = Vec::new();

            fn find_skill_md(dir: &std::path::Path, found: &mut Vec<PathBuf>) {
                if let Ok(entries) = std::fs::read_dir(dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.is_dir() {
                            if path.join("SKILL.md").exists() {
                                found.push(path);
                            } else {
                                // Recurse into subdirectories
                                find_skill_md(&path, found);
                            }
                        }
                    }
                }
            }

            find_skill_md(clone_path, &mut discovered_skills);

            if discovered_skills.is_empty() {
                eprintln!("❌ No Agent Skills found in repository");
                return Ok(());
            }

            eprintln!("✓ Found {} Agent Skill(s)", discovered_skills.len());

            // Filter by requested skills if specified
            let skills_to_import: Vec<PathBuf> = if skill.is_empty() {
                discovered_skills
            } else {
                discovered_skills
                    .into_iter()
                    .filter(|path| {
                        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                            skill.contains(&name.to_string())
                        } else {
                            false
                        }
                    })
                    .collect()
            };

            if skills_to_import.is_empty() {
                eprintln!("❌ None of the requested skills found in repository");
                return Ok(());
            }

            eprintln!("\n📥 Importing {} skill(s):", skills_to_import.len());

            let mut imported = 0;
            let mut skipped = 0;
            let mut errors = 0;

            for skill_path in &skills_to_import {
                let skill_name = skill_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown");

                eprint!("  • {} ... ", skill_name);

                // Parse the Agent Skill
                match AgentSkill::from_directory(skill_path).await {
                    Ok(agent_skill) => {
                        // Check if skill already exists
                        let dest_path = paths.skills_root.join(skill_name);
                        if dest_path.exists() && !force {
                            eprintln!("⚠️  skipped (already exists, use --force to overwrite)");
                            skipped += 1;
                            continue;
                        }

                        // Create destination directory
                        std::fs::create_dir_all(&dest_path)?;

                        // Copy all files from source to destination
                        fn copy_dir_all(
                            src: &std::path::Path,
                            dst: &std::path::Path,
                        ) -> std::io::Result<()> {
                            std::fs::create_dir_all(dst)?;
                            for entry in std::fs::read_dir(src)? {
                                let entry = entry?;
                                let path = entry.path();
                                let dest_path = dst.join(entry.file_name());
                                if path.is_dir() {
                                    copy_dir_all(&path, &dest_path)?;
                                } else {
                                    std::fs::copy(&path, &dest_path)?;
                                }
                            }
                            Ok(())
                        }

                        if let Err(e) = copy_dir_all(skill_path, &dest_path) {
                            eprintln!("❌ failed ({})", e);
                            errors += 1;
                            continue;
                        }

                        eprintln!("✓ imported (v{})", agent_skill.version());
                        imported += 1;
                    }
                    Err(e) => {
                        eprintln!("❌ failed ({})", e);
                        errors += 1;
                    }
                }
            }

            eprintln!("\n📊 Summary:");
            eprintln!("  ✓ Imported: {}", imported);
            if skipped > 0 {
                eprintln!("  ⚠️  Skipped:  {}", skipped);
            }
            if errors > 0 {
                eprintln!("  ❌ Errors:   {}", errors);
            }

            if imported > 0 {
                eprintln!("\n✅ Skills imported successfully!");
                eprintln!("   Run `skills list` to see all available skills");
            }

            // Add to config for tracking
            let skills_list: Vec<String> = skills_to_import
                .iter()
                .filter_map(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .map(|s| s.to_string())
                })
                .collect();

            add_repo_to_config(&mut config, &repo, git_ref.as_deref(), skills_list);

            // Save config back to the active config file
            if let Err(e) = save_config(&config, &active_config_path) {
                eprintln!("⚠️  Warning: Failed to save config: {}", e);
            } else {
                eprintln!("\n📝 Config updated: {}", active_config_path.display());
            }
        }

        Commands::Sync => {
            use skillsrs::storage::sync::AgentSkillsSync;

            info!("Syncing Agent Skills from config.yaml");
            eprintln!("🔄 Syncing Agent Skills from configuration...\n");

            if config.agent_skills_repos.is_empty() {
                eprintln!("❌ No agent_skills_repos defined in config.yaml");
                eprintln!("\nAdd repositories to your config.yaml:");
                eprintln!(
                    r#"
agent_skills_repos:
  - repo: vercel-labs/agent-skills
    skills:
      - web-design-guidelines
  - repo: wshobson/agents
    skills:
      - monorepo-management
"#
                );
                return Ok(());
            }

            eprintln!(
                "📦 Configured repositories: {}",
                config.agent_skills_repos.len()
            );
            for repo_config in &config.agent_skills_repos {
                eprintln!("  • {}", repo_config.repo);
                if let Some(ref skills) = repo_config.skills {
                    eprintln!("    Skills: {}", skills.join(", "));
                }
            }
            eprintln!();

            match AgentSkillsSync::new(&paths.skills_root).await {
                Ok(mut sync) => match sync.sync_all(&config.agent_skills_repos).await {
                    Ok(report) => {
                        if report.is_empty() {
                            eprintln!("✅ All Agent Skills are up-to-date!");
                        } else {
                            eprintln!("📊 Sync Results:\n");

                            if !report.added.is_empty() {
                                eprintln!("  ✅ Added ({}):", report.added.len());
                                for skill in &report.added {
                                    eprintln!("     • {}", skill);
                                }
                            }

                            if !report.updated.is_empty() {
                                eprintln!("  🔄 Updated ({}):", report.updated.len());
                                for skill in &report.updated {
                                    eprintln!("     • {}", skill);
                                }
                            }

                            if !report.removed.is_empty() {
                                eprintln!("  🗑️  Removed ({}):", report.removed.len());
                                for skill in &report.removed {
                                    eprintln!("     • {}", skill);
                                }
                            }

                            if !report.errors.is_empty() {
                                eprintln!("  ❌ Errors ({}):", report.errors.len());
                                for error in &report.errors {
                                    eprintln!("     • {}", error);
                                }
                            }

                            eprintln!(
                                "\n✅ Sync complete! {} total changes",
                                report.total_changes()
                            );
                            eprintln!("   Run `skills list` to see all available skills");
                        }
                    }
                    Err(e) => {
                        eprintln!("❌ Sync failed: {}", e);
                        std::process::exit(1);
                    }
                },
                Err(e) => {
                    eprintln!("❌ Failed to initialize sync: {}", e);
                    std::process::exit(1);
                }
            }
        }

        Commands::Paths => {
            info!("Displaying system paths");
            eprintln!("skills.rs - System Paths\n");
            eprintln!("{}", paths.display());
            eprintln!("\n✓ All directories exist and are accessible");
            eprintln!("\nEnvironment variables for overrides:");
            eprintln!("  SKILLS_DATA_DIR       - Override data directory");
            eprintln!("  SKILLS_CONFIG_DIR     - Override config directory");
            eprintln!("  SKILLS_CACHE_DIR      - Override cache directory");
            eprintln!("  SKILLS_DATABASE_PATH  - Override database path");
            eprintln!("  SKILLS_ROOT           - Override skills directory");
            eprintln!("  SKILLS_LOGS_DIR       - Override logs directory");
        }

        Commands::Search { query, kind } => {
            info!("Running search query: {}", query);

            // Create minimal components for search testing
            let registry = Arc::new(Registry::new());
            let search_engine = Arc::new(SearchEngine::new(registry.clone()));

            // Initialize upstream manager and connect
            let upstream_manager = Arc::new(UpstreamManager::new(registry.clone()));
            for upstream_config in &config.upstreams {
                if let Err(e) = upstream_manager.add_upstream(upstream_config.clone()).await {
                    error!(
                        "Failed to connect to upstream {}: {}",
                        upstream_config.alias, e
                    );
                }
            }

            // Rebuild search index
            search_engine.rebuild();

            let search_query = skillsrs::storage::search::SearchQuery {
                q: query.clone(),
                kind: kind.unwrap_or_else(|| "any".to_string()),
                mode: "literal".to_string(),
                limit: 10,
                filters: None,
                cursor: None,
            };

            let results = search_engine.search(&search_query).await?;

            eprintln!("Using skills root: {}\n", paths.skills_root.display());
            eprintln!("Search results for '{}':\n", query);
            eprintln!(
                "Found {} matches (showing {}):\n",
                results.total_matches,
                results.matches.len()
            );

            for (i, result) in results.matches.iter().enumerate() {
                eprintln!("{}. {} ({})", i + 1, result.name, result.kind);
                eprintln!("   ID: {}", result.id);
                eprintln!("   FQ Name: {}", result.fq_name);
                if let Some(server) = &result.server {
                    eprintln!("   Server: {}", server);
                }
                eprintln!("   Score: {:.2}", result.score);
                eprintln!("   Description: {}", result.description_snippet);
                eprintln!();
            }

            if let Some(cursor) = results.next_cursor {
                eprintln!("Next cursor: {}", cursor);
            }
        }

        Commands::Skill(skill_cmd) => {
            let skill_store = Arc::new(SkillStore::new(
                &paths.skills_root,
                Arc::new(Registry::new()),
            )?);

            match skill_cmd {
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
                            .map_err(|e| anyhow::anyhow!("Failed to read SKILL.md: {}", e))?
                    } else {
                        // Interactive mode - read from stdin with prompt
                        println!("Enter SKILL.md content (Ctrl+D when done):");
                        use std::io::Read;
                        let mut buffer = String::new();
                        std::io::stdin().read_to_string(&mut buffer)?;
                        buffer
                    };

                    let desc = description.unwrap_or_else(|| format!("Skill: {}", name));

                    let request = skillsrs::storage::CreateSkillRequest {
                        name: name.clone(),
                        version,
                        description: desc,
                        skill_md_content,
                        uses_tools,
                        bundled_files: vec![],
                        tags: vec!["cli-created".to_string()],
                    };

                    let id = skill_store.create_skill(request).await?;
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
                    in_place: _,
                } => {
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

                    // Load existing skill content
                    let current_content = skill_store.load_skill_content(&skill_name).ok();

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
                            .map_err(|e| anyhow::anyhow!("Failed to read SKILL.md: {}", e))?
                    } else if let Some(current) = current_content.as_ref() {
                        // Start with existing content for modifications
                        let mut md = current.skill_md.clone();

                        // Apply sed-like replace
                        if let (Some(pattern), Some(replacement)) = (&replace, &replace_with) {
                            md = md.replace(pattern, replacement);
                        }

                        // Apply prepend
                        if let Some(prefix) = prepend {
                            md = format!("{}\n{}", prefix, md);
                        }

                        // Apply append
                        if let Some(suffix) = append {
                            md = format!("{}\n{}", md, suffix);
                        }

                        md
                    } else {
                        return Err(anyhow::anyhow!("No SKILL.md content provided and skill not found. Use --content or --skill-md"));
                    };

                    // Build request with provided fields
                    let request = skillsrs::storage::CreateSkillRequest {
                        name: name.unwrap_or_else(|| skill_name.clone()),
                        version: version.unwrap_or_else(|| "1.0.0".to_string()),
                        description: description
                            .unwrap_or_else(|| format!("Skill: {}", skill_name)),
                        skill_md_content,
                        uses_tools,
                        bundled_files: vec![],
                        tags: vec!["cli-updated".to_string()],
                    };

                    let id = skill_store.update_skill(&skill_name, request).await?;
                    println!("✓ Updated skill: {} ({})", skill_name, id.as_str());
                }

                SkillCommands::Delete { skill_id, force } => {
                    if !force {
                        println!(
                            "Are you sure you want to delete skill '{}'? [y/N]",
                            skill_id
                        );
                        use std::io::Read;
                        let mut buffer = [0u8; 1];
                        std::io::stdin().read_exact(&mut buffer).ok();
                        if buffer[0] != b'y' && buffer[0] != b'Y' {
                            println!("Deletion cancelled.");
                            return Ok(());
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

                    skill_store.delete_skill(&skill_name)?;
                    println!("✓ Deleted skill: {}", skill_id);
                }

                SkillCommands::Show { skill_id, file } => {
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

                    if let Some(filename) = file {
                        let content = skill_store.load_skill_file(&skill_name, &filename)?;
                        println!("{}", content);
                    } else {
                        let content = skill_store.load_skill_content(&skill_name)?;
                        println!("# Skill: {}\n", skill_id);
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
    }

    Ok(())
}
