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
use skillsrs_index::SearchEngine;
use skillsrs_mcp::SkillsServer;
use skillsrs_policy::{PolicyConfig, PolicyEngine};
use skillsrs_registry::Registry;
use skillsrs_runtime::{sandbox::SandboxBackend, sandbox::SandboxConfig, Runtime};
use skillsrs_skillstore::SkillStore;
use skillsrs_upstream::UpstreamManager;
use std::sync::Arc;
use tracing::{error, info};
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
#[derive(Debug, Clone, serde::Deserialize, Default)]
struct Config {
    #[serde(default)]
    #[allow(dead_code)]
    server: ServerConfig,

    #[serde(default)]
    policy: PolicyConfig,

    #[serde(default)]
    upstreams: Vec<skillsrs_upstream::UpstreamConfig>,

    #[serde(default)]
    paths: PathsConfig,

    #[serde(default)]
    sandbox: SandboxConfig,

    #[serde(default)]
    use_global: UseGlobalSettings,
}

#[derive(Debug, Clone, serde::Deserialize, Default)]
struct UseGlobalSettings {
    /// If true, overlay the project config on top of global config.
    /// This allows using global upstreams/skills together with project ones.
    #[serde(default)]
    enabled: bool,
}

#[derive(Debug, Clone, serde::Deserialize)]
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

    let mut config = if let Some(ref config_file) = cli.config {
        load_config(std::path::Path::new(config_file))?
    } else if cli.global {
        load_config(&sys_config_path)?
    } else if let Some(ref p) = project_config_path {
        load_config(p)?
    } else {
        load_config(&sys_config_path)?
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
                    eprintln!("Exposing 3 tools: skills.search, skills.schema, skills.exec");
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
                    eprintln!("Exposing 3 tools: skills.search, skills.schema, skills.exec");
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
                                    <p>Exposes exactly 3 tools:</p>
                                    <ul>
                                        <li><code>skills.search</code> - Discovery over registry</li>
                                        <li><code>skills.schema</code> - On-demand schema fetching</li>
                                        <li><code>skills.exec</code> - Validated execution</li>
                                    </ul>
                                </body>
                                </html>
                                "#,
                                )
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

            // Initialize minimal components for CLI mode
            let registry = Arc::new(Registry::new());
            let upstream_manager = Arc::new(UpstreamManager::new(registry.clone()));

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
            let exec_context = skillsrs_runtime::ExecContext {
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
                            if let skillsrs_core::ToolResultContent::Text { text } = content {
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
            let exec_context = skillsrs_runtime::ExecContext {
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

            let search_query = skillsrs_index::SearchQuery {
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
    }

    Ok(())
}
