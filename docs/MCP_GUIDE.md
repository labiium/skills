# MCP Server Guide

A comprehensive guide for adding and using MCP servers in skills.rs.

---

## Table of Contents

1. [Overview](#overview)
2. [Quick Start (5 minutes)](#quick-start-5-minutes)
3. [Configuration Reference](#configuration-reference)
4. [Finding MCP Servers](#finding-mcp-servers)
5. [Using MCP Tools in Skills](#using-mcp-tools-in-skills)
6. [Troubleshooting](#troubleshooting)
7. [Advanced Topics](#advanced-topics)

---

## Overview

### What are MCP Servers?

**MCP** (Model Context Protocol) servers are programs that expose "tools" that an AI can call. They give your AI the ability to interact with the real world:

- Read and write files
- Search the web
- Query databases
- Control external APIs
- Execute shell commands
- And much more

Without MCP servers, your AI can only talk. With them, it can *do* things.

### How skills.rs Aggregates MCP Servers

**skills.rs** acts as a unified gateway that connects to multiple MCP servers (called "upstreams" in configuration) and aggregates all their tools into a single interface.

```
┌─────────────────────────────────────────────────────────────┐
│                    skills.rs Gateway                         │
│                                                              │
│   ┌─────────────┐  ┌─────────────┐  ┌─────────────┐         │
│   │  Upstream   │  │  Upstream   │  │  Upstream   │         │
│   │  filesystem │  │   brave     │  │   github    │         │
│   └──────┬──────┘  └──────┬──────┘  └──────┬──────┘         │
│          │                │                │                │
│   ┌──────┴────────────────┴────────────────┴──────┐         │
│   │         Unified Tool Registry                 │         │
│   │    filesystem/read_file                      │         │
│   │    filesystem/write_file                     │         │
│   │    brave_search/search                       │         │
│   │    github/create_issue                       │         │
│   └───────────────────────────────────────────────┘         │
└─────────────────────────────────────────────────────────────┘
```

### Why Use MCP Servers?

1. **Expand AI Capabilities** - Give your AI access to external systems
2. **Progressive Disclosure** - skills.rs exposes 7 meta-tools instead of hundreds, saving context tokens
3. **Unified Interface** - Access tools from multiple servers through one gateway
4. **Security** - Built-in sandboxing and policy controls
5. **Audit Trail** - All tool executions are logged to SQLite

---

## Quick Start (5 minutes)

### Step 1: Install Prerequisites

You'll need Node.js installed for most MCP servers:

```bash
# macOS
brew install node

# Ubuntu/Debian
sudo apt install nodejs npm

# Verify
node --version  # Should be v18+
npm --version
```

### Step 2: Initialize skills.rs

```bash
# Install skills.rs
cargo install --git https://github.com/labiium/skills

# Navigate to your project
cd my-project

# Initialize configuration
skills init
```

This creates a `.skills/` folder with:
- `config.yaml` - Your configuration file
- `skills/` - Where custom skills live
- `skills.db` - SQLite database for tracking

### Step 3: Add Filesystem MCP Server

Edit `.skills/config.yaml` and add an upstream:

```yaml
paths:
  data_dir: ".skills"
  skills_root: ".skills/skills"
  database_path: ".skills/skills.db"

sandbox:
  backend: timeout
  timeout_ms: 30000

upstreams:
  - alias: filesystem
    transport: stdio
    command: ["npx", "-y", "@modelcontextprotocol/server-filesystem", "."]
    tags:
      - filesystem
      - local
```

### Step 4: Verify It Works

List all available tools:

```bash
skills list
```

You should see:
```
filesystem/read_file
filesystem/write_file
filesystem/list_directory
filesystem/directory_tree
...
```

Test a tool:

```bash
skills tool filesystem/read_file '{"path": "./README.md"}'
```

### Step 5: Use It in a Skill

Create a skill that uses the filesystem tool:

```bash
mkdir -p .skills/skills/file-analyzer

# Create skill.json
cat > .skills/skills/file-analyzer/skill.json << 'EOF'
{
  "id": "file-analyzer",
  "title": "File Analyzer",
  "version": "1.0.0",
  "description": "Reads and analyzes file contents",
  "inputs": {
    "type": "object",
    "properties": {
      "file_path": {
        "type": "string",
        "description": "Path to the file to analyze"
      }
    },
    "required": ["file_path"]
  },
  "entrypoint": "prompted",
  "tool_policy": {
    "allow": ["filesystem/read_file"],
    "deny": [],
    "required": ["filesystem/read_file"]
  },
  "risk_tier": "read_only"
}
EOF

# Create SKILL.md
cat > .skills/skills/file-analyzer/SKILL.md << 'EOF'
# File Analyzer

## Purpose
Reads a file and provides analysis of its contents.

## Instructions
1. Read the file using `filesystem/read_file`
2. Analyze the content for key information
3. Provide a summary of findings

## Tools Used
- `filesystem/read_file` - Read the target file
EOF
```

Test your skill:

```bash
skills grep "*file-analyzer*"
```

---

## Configuration Reference

### stdio Transport (Most Common)

The `stdio` transport runs a command locally and communicates over stdin/stdout. This is the most common transport for MCP servers.

#### Basic Structure

```yaml
upstreams:
  - alias: <name>           # Unique identifier for this upstream
    transport: stdio        # Transport type
    command:                # Command to execute
      - "npx"
      - "-y"
      - "@modelcontextprotocol/server-name"
      - "/allowed/path"
    tags:                   # Optional categorization tags
      - "category1"
      - "category2"
```

#### Example: Filesystem MCP

```yaml
upstreams:
  - alias: filesystem
    transport: stdio
    command:
      - "npx"
      - "-y"
      - "@modelcontextprotocol/server-filesystem"
      - "/home/user/documents"   # Allowed directory
    tags:
      - filesystem
      - local
```

#### Example: Brave Search (with API key via environment)

```yaml
upstreams:
  - alias: brave
    transport: stdio
    command:
      - "npx"
      - "-y"
      - "@modelcontextprotocol/server-brave-search"
    env:                      # Environment variables
      BRAVE_API_KEY: "${BRAVE_API_KEY}"  # Reference to host env var
    tags:
      - search
      - web
```

> **Note:** Set the actual API key in your shell environment: `export BRAVE_API_KEY=your_key_here`

#### Example: GitHub MCP

```yaml
upstreams:
  - alias: github
    transport: stdio
    command:
      - "npx"
      - "-y"
      - "@modelcontextprotocol/server-github"
    env:
      GITHUB_PERSONAL_ACCESS_TOKEN: "${GITHUB_TOKEN}"
    tags:
      - github
      - git
      - remote
```

#### Example: Custom Local Tool

```yaml
upstreams:
  - alias: my-local-tool
    transport: stdio
    command:
      - "/usr/local/bin/my-mcp-server"
      - "--config"
      - "/etc/my-tool/config.yaml"
    tags:
      - custom
      - local
```

### HTTP Transport

The `http` transport connects to a remote MCP server over HTTP.

#### Basic Structure

```yaml
upstreams:
  - alias: <name>
    transport: http
    url: "https://api.example.com/mcp"
    auth:
      type: <auth_type>     # bearer, header, or none
      <auth_config>
    tags:
      - remote
```

#### Example: Bearer Token Authentication

```yaml
upstreams:
  - alias: github-api
    transport: http
    url: "https://api.github.com/mcp"
    auth:
      type: bearer
      env: "GITHUB_TOKEN"    # Reads from GITHUB_TOKEN env var
    tags:
      - github
      - remote
```

#### Example: Custom Header Authentication

```yaml
upstreams:
  - alias: internal-api
    transport: http
    url: "https://internal.company.com/mcp"
    auth:
      type: header
      header: "X-API-Key"
      env: "INTERNAL_API_KEY"
    tags:
      - internal
      - remote
```

#### Example: No Authentication

```yaml
upstreams:
  - alias: public-api
    transport: http
    url: "https://public.example.com/mcp"
    tags:
      - public
      - remote
```

### Per-Server Sandboxing

Configure different sandbox levels for different upstreams:

```yaml
upstreams:
  # Trusted local server - minimal sandboxing
  - alias: local-fs
    transport: stdio
    command: ["npx", "-y", "@modelcontextprotocol/server-filesystem", "."]
    sandbox:
      backend: timeout
      timeout_ms: 30000
    tags:
      - trusted

  # Untrusted remote server - full sandboxing
  - alias: remote-api
    transport: http
    url: "https://untrusted-api.com/mcp"
    sandbox:
      backend: restricted
      timeout_ms: 10000
      allow_read: []
      allow_write: []
      allow_network: false
    tags:
      - untrusted

  # Production-grade isolation (Linux only)
  - alias: risky-tool
    transport: stdio
    command: ["npx", "-y", "risky-mcp-server"]
    sandbox:
      backend: bubblewrap      # Requires bubblewrap installed
      timeout_ms: 30000
      max_memory_bytes: 536870912  # 512MB
      max_cpu_seconds: 30
      allow_network: false
    tags:
      - risky
```

### Security Considerations

| Backend | Security Level | Platform | Use Case |
|---------|---------------|----------|----------|
| `none` | None | All | Development only ⚠️ |
| `timeout` | Basic | All | Trusted tools |
| `restricted` | Medium | Unix | General production |
| `bubblewrap` | High | Linux | Untrusted/isolated |

**Best Practices:**
1. Use `restricted` or `bubblewrap` for production
2. Set appropriate timeouts (30-60 seconds typical)
3. Limit memory for untrusted tools (256-512MB)
4. Disable network access for tools that don't need it
5. Use per-server sandboxing to apply least privilege

---

## Finding MCP Servers

### Official Model Context Protocol Servers

The official MCP servers maintained by Anthropic:

```bash
# Filesystem
npx -y @modelcontextprotocol/server-filesystem <allowed-directory>

# Brave Search
BRAVE_API_KEY=your_key npx -y @modelcontextprotocol/server-brave-search

# GitHub
GITHUB_PERSONAL_ACCESS_TOKEN=your_token npx -y @modelcontextprotocol/server-github

# PostgreSQL
npx -y @modelcontextprotocol/server-postgres postgresql://localhost/mydb

# SQLite
npx -y @modelcontextprotocol/server-sqlite /path/to/database.db

# Fetch (web scraping)
npx -y @modelcontextprotocol/server-fetch
```

### Awesome MCP Servers Repository

Community-maintained list of MCP servers:

- **Repository:** [github.com/modelcontextprotocol/servers](https://github.com/modelcontextprotocol/servers)
- **Categories:**
  - File systems (S3, GCS, Dropbox)
  - Databases (MySQL, MongoDB, Redis)
  - APIs (Stripe, Slack, Discord)
  - Development (Git, Docker, Kubernetes)
  - AI services (OpenAI, Anthropic, Pinecone)

### NPM Registry Search

```bash
# Search for MCP servers
npm search @modelcontextprotocol/server-

# Or search all MCP-related packages
npm search mcp-server
```

### Built-in/Popular Servers Table

| Server | Install | Auth Required | Use Case |
|--------|---------|---------------|----------|
| filesystem | `@modelcontextprotocol/server-filesystem` | No | File operations |
| brave-search | `@modelcontextprotocol/server-brave-search` | Yes (API key) | Web search |
| github | `@modelcontextprotocol/server-github` | Yes (token) | GitHub operations |
| postgres | `@modelcontextprotocol/server-postgres` | No* | Database queries |
| sqlite | `@modelcontextprotocol/server-sqlite` | No | SQLite operations |
| fetch | `@modelcontextprotocol/server-fetch` | No | Web scraping |
| slack | `@modelcontextprotocol/server-slack` | Yes | Slack integration |
| git | `@modelcontextprotocol/server-git` | No | Git operations |
| puppeteer | `@modelcontextprotocol/server-puppeteer` | No | Browser automation |

*Database connection string contains credentials

---

## Using MCP Tools in Skills

### How Skills Reference MCP Tools

Skills declare which tools they can use via the `tool_policy` field in `skill.json`:

```json
{
  "id": "web-researcher",
  "title": "Web Researcher",
  "version": "1.0.0",
  "description": "Research topics using web search",
  "inputs": {
    "type": "object",
    "properties": {
      "query": {
        "type": "string",
        "description": "Topic to research"
      }
    },
    "required": ["query"]
  },
  "entrypoint": "prompted",
  "tool_policy": {
    "allow": ["brave_search"],
    "deny": [],
    "required": ["brave_search"]
  },
  "risk_tier": "read_only"
}
```

### Tool Policy Configuration

The `tool_policy` object has three fields:

| Field | Type | Description |
|-------|------|-------------|
| `allow` | string[] | Tool patterns this skill can use (glob-style) |
| `deny` | string[] | Tool patterns explicitly forbidden |
| `required` | string[] | Tools the skill must have access to |

**Pattern matching:**
- `"*"` - All tools
- `"brave_search"` - Specific tool name
- `"filesystem/*"` - All tools from filesystem upstream
- `"*search*"` - Any tool containing "search"

### Example: Skill Using Web Search + Filesystem

**skill.json:**
```json
{
  "id": "research-writer",
  "title": "Research Writer",
  "version": "1.0.0",
  "description": "Research a topic and write findings to a file",
  "inputs": {
    "type": "object",
    "properties": {
      "topic": {
        "type": "string",
        "description": "Topic to research"
      },
      "output_file": {
        "type": "string",
        "description": "Path to write the report"
      }
    },
    "required": ["topic", "output_file"]
  },
  "entrypoint": "prompted",
  "tool_policy": {
    "allow": ["brave_search", "filesystem/*"],
    "deny": ["filesystem/delete_file"],
    "required": ["brave_search", "filesystem/write_file"]
  },
  "risk_tier": "limited_write"
}
```

**SKILL.md:**
```markdown
# Research Writer

## Purpose
Research a topic using web search and save findings to a file.

## Instructions
1. Use `brave_search` to find information about the topic
2. Read 3-5 top results to gather comprehensive information
3. Synthesize findings into a well-structured report
4. Write the report to the specified output file using `filesystem/write_file`

## Tools Used
- `brave_search` - Search the web for information
- `filesystem/read_file` - Read search results (if needed)
- `filesystem/write_file` - Save the final report

## Expected Output
A markdown file containing the research report.
```

### Complete Skill Example with Multiple MCP Tools

```bash
mkdir -p .skills/skills/project-setup
```

**skill.json:**
```json
{
  "id": "project-setup",
  "title": "Project Setup Assistant",
  "version": "1.0.0",
  "description": "Sets up a new project with README, license, and initial structure",
  "inputs": {
    "type": "object",
    "properties": {
      "project_name": {
        "type": "string",
        "description": "Name of the project"
      },
      "project_type": {
        "type": "string",
        "enum": ["python", "node", "rust"],
        "description": "Type of project"
      },
      "license": {
        "type": "string",
        "enum": ["MIT", "Apache-2.0", "GPL-3.0"],
        "default": "MIT"
      }
    },
    "required": ["project_name", "project_type"]
  },
  "entrypoint": "prompted",
  "tool_policy": {
    "allow": ["filesystem/*", "github/*"],
    "deny": ["filesystem/delete_file", "github/delete_repository"],
    "required": ["filesystem/create_directory", "filesystem/write_file"]
  },
  "risk_tier": "limited_write"
}
```

**SKILL.md:**
```markdown
# Project Setup Assistant

## Purpose
Creates a new project directory with standard files and structure.

## Instructions
1. Create the project directory using `filesystem/create_directory`
2. Create standard project files:
   - README.md with project description
   - LICENSE file (based on input)
   - .gitignore appropriate for project type
   - Basic configuration files (package.json, Cargo.toml, etc.)
3. Initialize a Git repository (optional, if git tools available)
4. Create initial source directory structure

## Project Type Structures

### Python
```
project_name/
├── README.md
├── LICENSE
├── .gitignore
├── requirements.txt
├── setup.py
└── src/
    └── project_name/
        └── __init__.py
```

### Node
```
project_name/
├── README.md
├── LICENSE
├── .gitignore
├── package.json
└── src/
    └── index.js
```

### Rust
```
project_name/
├── README.md
├── LICENSE
├── .gitignore
└── Cargo.toml
```

## Tools Used
- `filesystem/create_directory` - Create directories
- `filesystem/write_file` - Write files
- `github/create_repository` - Create GitHub repo (optional)

## Expected Output
A complete project directory ready for development.
```

---

## Troubleshooting

### Common Errors

#### "Failed to connect to upstream: <name>"

**Causes:**
- MCP server not installed
- Command path incorrect
- Missing environment variables
- Network issues (for HTTP upstreams)

**Solutions:**
```bash
# Test command manually
npx -y @modelcontextprotocol/server-filesystem .

# Check environment variables
echo $BRAVE_API_KEY
echo $GITHUB_TOKEN

# Enable debug logging
RUST_LOG=debug skills list
```

#### "Tool not found: <server>/<tool>"

**Causes:**
- Upstream not configured
- Typo in tool name
- Server not responding

**Solutions:**
```bash
# List all available tools
skills list

# Check specific server
skills list <server_name>

# Verify server connection
skills tool <server>/<any_tool>
```

#### "Permission denied"

**Causes:**
- Sandboxing too restrictive
- Filesystem paths not allowed
- Network access blocked

**Solutions:**
```yaml
# Relax sandbox for development
sandbox:
  backend: timeout  # or "none"

# Or allow specific paths
sandbox:
  backend: restricted
  allow_read: ["/home/user/projects"]
  allow_write: ["/home/user/projects"]
  allow_network: true
```

#### "Tool execution timed out"

**Causes:**
- Default timeout too short
- Slow network/operation
- Infinite loop in tool

**Solutions:**
```yaml
# Increase timeout
sandbox:
  timeout_ms: 120000  # 2 minutes

# Or disable for specific upstream
upstreams:
  - alias: slow-server
    sandbox:
      timeout_ms: 300000  # 5 minutes
```

### Debug Logging

Enable detailed logging to diagnose issues:

```bash
# Basic debug info
RUST_LOG=info skills server stdio

# Detailed debug
RUST_LOG=debug skills server stdio

# Trace everything
RUST_LOG=trace skills server stdio

# Log to file
RUST_LOG=debug skills server stdio 2> skills.log
```

### Testing Connections

Test individual upstream connections:

```bash
# Test stdio server
npx -y @modelcontextprotocol/server-filesystem . --help

# Test HTTP server
curl -H "Authorization: Bearer $TOKEN" https://api.github.com/mcp/health

# Verify in skills.rs
skills list <upstream_alias>
```

### Configuration Validation

```bash
# Check config file syntax
cat .skills/config.yaml | yq  # Requires yq installed

# Verify paths
skills paths

# Test config loading
RUST_LOG=debug skills list 2>&1 | grep -i "config\|upstream"
```

---

## Advanced Topics

### Multiple Upstreams

Configure multiple MCP servers:

```yaml
upstreams:
  # File operations
  - alias: filesystem
    transport: stdio
    command: ["npx", "-y", "@modelcontextprotocol/server-filesystem", "."]
    tags: ["filesystem", "local"]

  # Web search
  - alias: brave
    transport: stdio
    command: ["npx", "-y", "@modelcontextprotocol/server-brave-search"]
    env:
      BRAVE_API_KEY: "${BRAVE_API_KEY}"
    tags: ["search", "web"]

  # GitHub integration
  - alias: github
    transport: stdio
    command: ["npx", "-y", "@modelcontextprotocol/server-github"]
    env:
      GITHUB_PERSONAL_ACCESS_TOKEN: "${GITHUB_TOKEN}"
    tags: ["github", "git", "remote"]

  # Database access
  - alias: postgres
    transport: stdio
    command: ["npx", "-y", "@modelcontextprotocol/server-postgres", "postgresql://localhost/mydb"]
    tags: ["database", "sql"]

  # External API
  - alias: stripe
    transport: http
    url: "https://api.stripe.com/v1/mcp"
    auth:
      type: bearer
      env: "STRIPE_API_KEY"
    tags: ["payments", "external"]
```

### Environment Variables in Commands

Reference environment variables in your config:

```yaml
upstreams:
  - alias: github
    transport: stdio
    command: ["npx", "-y", "@modelcontextprotocol/server-github"]
    env:
      # Reference host environment variable
      GITHUB_PERSONAL_ACCESS_TOKEN: "${GITHUB_TOKEN}"
      
      # Hardcoded (not recommended for secrets)
      GITHUB_API_URL: "https://api.github.com"
      
      # With default fallback
      LOG_LEVEL: "${LOG_LEVEL:-info}"
```

### Network Policies

Control network access per upstream:

```yaml
upstreams:
  # No network needed
  - alias: local-fs
    transport: stdio
    command: ["npx", "-y", "@modelcontextprotocol/server-filesystem", "."]
    sandbox:
      allow_network: false

  # Network required
  - alias: brave
    transport: stdio
    command: ["npx", "-y", "@modelcontextprotocol/server-brave-search"]
    sandbox:
      allow_network: true

  # Restrict to specific hosts (implementation-dependent)
  - alias: internal-api
    transport: http
    url: "https://internal.company.com/mcp"
    sandbox:
      allow_network: true
      # Additional network restrictions may apply based on backend
```

### Security Best Practices

1. **Default to Restricted:**
```yaml
sandbox:
  backend: restricted
  timeout_ms: 30000
  max_memory_bytes: 536870912
  allow_network: false
```

2. **Use Per-Server Policies:**
```yaml
upstreams:
  - alias: trusted-local
    sandbox:
      backend: timeout  # Minimal for trusted tools
      
  - alias: untrusted-remote
    sandbox:
      backend: bubblewrap  # Full isolation
      max_memory_bytes: 268435456  # 256MB limit
      allow_network: false
```

3. **Separate Credentials:**
```bash
# Use environment files
export $(cat .env | xargs)

# Or secret management
eval $(vault env -secret=skills/credentials)
```

4. **Audit and Monitor:**
```bash
# Query execution history
sqlite3 .skills/skills.db "SELECT * FROM executions ORDER BY started_at DESC LIMIT 10"

# Check for failures
sqlite3 .skills/skills.db "SELECT * FROM executions WHERE success = 0"
```

5. **Regular Updates:**
```bash
# Update MCP servers
npx -y @modelcontextprotocol/server-filesystem  # Gets latest

# Update skills.rs
cargo install --git https://github.com/labiium/skills --force
```

### Complete Production Configuration Example

```yaml
# Server settings
server:
  transport: stdio
  log_level: info

# Default sandbox for bundled tools
sandbox:
  backend: restricted
  timeout_ms: 30000
  max_memory_bytes: 536870912
  max_cpu_seconds: 30
  allow_network: false

# Policy engine
policy:
  default_risk: read_only
  require_consent_for:
    - writes
    - destructive
  trusted_servers:
    - filesystem
    - local-tools

# Upstream MCP servers
upstreams:
  # Local filesystem (restricted access)
  - alias: filesystem
    transport: stdio
    command:
      - "npx"
      - "-y"
      - "@modelcontextprotocol/server-filesystem"
      - "/home/user/projects"
    sandbox:
      backend: timeout  # Filesystem server is trusted
    tags:
      - filesystem
      - trusted

  # Web search (requires network)
  - alias: brave
    transport: stdio
    command:
      - "npx"
      - "-y"
      - "@modelcontextprotocol/server-brave-search"
    env:
      BRAVE_API_KEY: "${BRAVE_API_KEY}"
    sandbox:
      backend: timeout
      allow_network: true
    tags:
      - search
      - web

  # GitHub (authenticated, network required)
  - alias: github
    transport: stdio
    command:
      - "npx"
      - "-y"
      - "@modelcontextprotocol/server-github"
    env:
      GITHUB_PERSONAL_ACCESS_TOKEN: "${GITHUB_TOKEN}"
    sandbox:
      backend: timeout
      allow_network: true
    tags:
      - github
      - remote
      - authenticated

# Paths
paths:
  data_dir: ".skills"
  skills_root: ".skills/skills"
  database_path: ".skills/skills.db"
```

---

## Cross-Reference

- **[TUTORIAL.md](TUTORIAL.md)** - Step-by-step beginner's guide
- **[OPERATIONS.md](OPERATIONS.md)** - Deployment and production operations
- **[SKILLS.md](SKILLS.md)** - Overview of the skills system
- **[QUICKSTART.md](QUICKSTART.md)** - 5-minute quick start
- **[config.example.yaml](config.example.yaml)** - Complete configuration reference

---

## Summary

You now have everything you need to:

1. ✅ Add MCP servers to skills.rs via `upstreams` configuration
2. ✅ Configure both stdio and HTTP transports
3. ✅ Use MCP tools in your skills via `tool_policy`
4. ✅ Apply appropriate sandboxing per upstream
5. ✅ Troubleshoot common connection and permission issues
6. ✅ Follow security best practices

**Next Steps:**
- Browse the [awesome-mcp-servers](https://github.com/modelcontextprotocol/servers) repository
- Create a skill that combines multiple MCP tools
- Experiment with different sandbox backends
- Read the [TUTORIAL.md](TUTORIAL.md) for a guided walkthrough

---

*Built with 🦀 Rust | Powered by the Model Context Protocol*
