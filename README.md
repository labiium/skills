# Skills.rs

**Infinite Skills. Finite Context.**

[![CI](https://github.com/labiium/skills/actions/workflows/ci.yml/badge.svg)](https://github.com/labiium/skills/actions/workflows/ci.yml)
[![Coverage](https://github.com/labiium/skills/actions/workflows/coverage.yml/badge.svg)](https://github.com/labiium/skills/actions/workflows/coverage.yml)
[![Docker](https://github.com/labiium/skills/actions/workflows/docker.yml/badge.svg)](https://github.com/labiium/skills/actions/workflows/docker.yml)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](LICENSE)
[![Rust Version](https://img.shields.io/badge/rust-1.92%2B-orange.svg)](https://www.rust-lang.org)

---

## 🎯 What is skills.rs?

skills.rs is a **unified MCP server** that aggregates multiple upstream MCP servers and high-level Skills into a single, unified registry. It exposes **4 focused MCP tools** to prevent context-window bloat while enabling unbounded tool/skill discovery and skill lifecycle management.

### Key Features

- 🔍 **Unified Discovery** - Search across all tools and skills from one interface
- 📦 **Progressive Disclosure** - Load skill content on-demand to save tokens (99% token reduction)
- 🤖 **AI Agent CLI** - Drop-in replacement for mcp-cli with enhanced features
- 🌐 **Agent Skills Compatible** - Import skills from Vercel skills.sh ecosystem (`skills add owner/repo`)
- 🛡️ **Sandboxed Execution** - Safe execution of bundled scripts with resource limits
- 💾 **Persistence** - SQLite-based storage for registry and execution history
- ✅ **Validation** - Comprehensive skill validation and dependency checking
- 🔒 **Security** - Multi-backend sandboxing (timeout, rlimits, containers)
- 🚀 **Production-Ready** - Fully tested, documented, and hardened

### Two Modes of Operation

1. **CLI Mode** (AI Agent Integration)
   - Direct tool discovery and execution from command line
   - Compatible with mcp-cli workflows
   - `skills list`, `skills tool`, `skills exec`, `skills grep`
   - `skills skill create`, `skills skill edit`, `skills skill delete`, `skills skill show`

2. **Server Mode** (MCP Protocol)
   - Run as MCP server exposing meta-tools
   - `search`, `schema`, `exec`, `manage`
   - Aggregate multiple upstream MCP servers
   - Create, update, delete skills via unified manage tool

---

## 🚀 Quick Start

### Installation

```bash
# Install directly from GitHub (recommended)
cargo install --git https://github.com/labiium/skills

# Install skills.rs from Crates.io
cargo install skillsrs

# Or clone and build from source
git clone https://github.com/labiium/skills
cd skills
cargo build --release
```

After installation, skills.rs will automatically use system-appropriate directories:
- **Linux**: `~/.local/share/skills`, `~/.config/skills`
- **macOS**: `~/Library/Application Support/skills`, `~/Library/Preferences/skills`
- **Windows**: `%APPDATA%\labiium\skills`

View your paths with:
```bash
skills paths
```

### Configuration

Skills.rs supports both **project-local** and **global** configuration.

#### Project-local (recommended)

Initialize a project-local setup in the repo root:

```bash
skills init
```

This creates:

- `.skills/config.yaml`
- `.skills/skills/`
- `.skills/skills.db`

`skills` will automatically discover the nearest `.skills/config.yaml` by walking up from the current directory.

Minimal `.skills/config.yaml`:

```yaml
paths:
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

upstreams: []
```

To disable sandboxing entirely:

```yaml
sandbox:
  backend: none
```

#### Global configuration

Global config is stored in the system config directory (varies by platform). To force using global config (ignore project `.skills/config.yaml`):

```bash
skills --global list
```

You can also point at a specific file:

```bash
skills --config /path/to/config.yaml list
```

#### Global + project combined (recommended for teams)

If you want to use **global upstreams/skills** in addition to project ones, set in `.skills/config.yaml`:

```yaml
use_global:
  enabled: true
```

This overlays project settings on top of the global config and appends project `upstreams` to the global list.

**Environment Variable Overrides:**
```bash
export SKILLS_ROOT=/custom/skills
export SKILLS_DATABASE_PATH=/custom/skills.db
skills server
```

**Command-Line Overrides:**
```bash
skills server --skills-root /custom/skills --database /custom/db/skills.db
```

To see the directories in use on your machine, run:

```bash
skills paths
```

### Run the Server

**Stdio mode (for MCP clients):**
```bash
skills server stdio
# or simply
skills server  # stdio is the default mode
```

**HTTP mode (for testing):**
```bash
skills server http --bind 127.0.0.1:8000
```

---

## 🤖 CLI Mode - mcp-cli Replacement

Skills.rs can replace mcp-cli while adding production features. Same workflow, better capabilities.

### Quick Comparison

| Feature | mcp-cli | skills.rs |
|---------|---------|-----------|
| Token Reduction | 99% | 99% |
| CLI Interface | ✓ | ✓ |
| Persistence | ✗ | ✓ |
| Sandboxing | ✗ | ✓ |
| Skills | ✗ | ✓ |
| MCP Server Mode | ✗ | ✓ |

### CLI Commands

```bash
# List all servers and tools
skills list                              # Like: mcp-cli
skills list -d                           # With descriptions: mcp-cli -d

# Search for tools
skills grep "*file*"                     # Like: mcp-cli grep "*file*"

# Get tool schema
skills tool filesystem/read_file         # Like: mcp-cli filesystem/read_file

# Execute a tool
skills tool filesystem/read_file '{"path": "./README.md"}'
# Like: mcp-cli filesystem/read_file '{"path": "./README.md"}'

# With JSON output
skills tool filesystem/read_file '{"path": "./README.md"}' --json

# Raw text only
skills tool filesystem/read_file '{"path": "./README.md"}' --raw
```

### Skill Management CLI

```bash
# Create a new skill from file
skills skill create my-skill \
  --description "My custom skill" \
  --skill-md ./SKILL.md \
  --uses-tools brave_search,grep

# Create skill with inline content
echo "# My Skill\n\nInstructions here" | skills skill create my-skill --content -

# Or provide content directly
skills skill create my-skill --content "# My Skill\n\nStep 1: Do this"

# Edit a skill - sed-like replacement
skills skill edit my-skill --replace "old text" --with "new text"

# Append to SKILL.md
skills skill edit my-skill --append "## Troubleshooting\n\nCommon issues..."

# Replace entire content from file
skills skill edit my-skill --skill-md ./updated-SKILL.md

# Or from stdin
cat ./updated-SKILL.md | skills skill edit my-skill --content -

# Show skill content
skills skill show my-skill

# Show specific file from skill
skills skill show my-skill --file helper.py

# Delete a skill (with confirmation)
skills skill delete my-skill

# Force delete without confirmation
skills skill delete my-skill --force
```

### AI Agent System Prompt

```markdown
You have access to MCP servers via the `skills` CLI.

Commands:
- `skills list` - List all servers and tools
- `skills list <server>` - Show server's tools  
- `skills list -d` - Include descriptions
- `skills tool <server>/<tool>` - Get tool schema
- `skills tool <server>/<tool> '<json>'` - Execute tool
- `skills grep "<pattern>"` - Search by pattern
- `skills skill create <name> --description "..."` - Create a skill
- `skills skill edit <skill-id>` - Edit/update a skill
- `skills skill delete <skill-id>` - Delete a skill
- `skills skill show <skill-id>` - Display skill content

Workflow:
1. Discover: `skills list` or `skills grep "<pattern>"`
2. Inspect: `skills tool <server>/<tool>` 
3. Execute: `skills tool <server>/<tool> '<json>'`
4. Manage: `skills skill create/edit/delete/show`
```

---

## 🔧 The 4 MCP Tools

### Core Discovery & Execution

#### 1. `search`
Fast discovery over unified registry (tools + skills)

```json
{
  "q": "search the web",
  "kind": "any",
  "limit": 10
}
```

#### 2. `schema`
Fetch full schema and signature for a callable

```json
{
  "id": "skill://web-researcher@1.0@abc123",
  "format": "json_schema"
}
```

#### 3. `exec`
Execute a callable with validation and policy enforcement

```json
{
  "id": "skill://web-researcher@1.0@abc123",
  "arguments": {"query": "latest AI news"},
  "timeout_ms": 30000
}
```

### Skill Lifecycle Management

#### 4. `manage`
Unified skill lifecycle management: create, get, update, delete

**Create a skill:**
```json
{
  "operation": "create",
  "name": "web-researcher",
  "version": "1.0.0",
  "description": "Research topics using web search",
  "skill_md": "# Web Researcher\n\n...",
  "uses_tools": ["brave_search"],
  "bundled_files": [["script.py", "print('hello')"]]
}
```

**Get skill content (progressive disclosure):**
```json
{
  "operation": "get",
  "skill_id": "web-researcher",
  "filename": "helper.py"
}
```

**Update a skill:**
```json
{
  "operation": "update",
  "skill_id": "web-researcher",
  "name": "web-researcher",
  "version": "1.1.0",
  "description": "Updated description",
  "skill_md": "# Updated content..."
}
```

**Delete a skill:**
```json
{
  "operation": "delete",
  "skill_id": "web-researcher"
}
```

---

## 📦 Skills System

### What is a Skill?

A **skill** is a package of agent instructions and optional bundled tools. Skills use **progressive disclosure** to minimize token usage:

**Level 1:** Metadata (name, description, tags) - always available  
**Level 2:** SKILL.md (instructions) - loaded on demand  
**Level 3:** Additional files - loaded progressively  
**Level 4:** Execution - when agent is ready

### Skill Directory Structure

```
skills/
  my-skill/
    skill.json          # Manifest (required)
    SKILL.md            # Instructions for agent (required)
    script.py           # Bundled tool (optional)
    script.py.schema.json  # Tool schema (optional)
    data.json           # Support file (optional)
    README.md           # Documentation (optional)
```

### Example: skill.json

```json
{
  "id": "web-researcher",
  "title": "Web Researcher",
  "version": "1.0.0",
  "description": "Research topics using web search and summarization",
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
    "allow": ["brave_search", "filesystem_read"],
    "deny": [],
    "required": []
  },
  "hints": {
    "domain": ["web", "research"],
    "expected_calls": 3
  },
  "risk_tier": "read_only"
}
```

### Example: SKILL.md

```markdown
# Web Researcher

## Purpose
Research topics using web search and save findings.

## Steps
1. Call `brave_search` with the query
2. Read top 3 results using `filesystem_read`
3. Summarize findings
4. Save summary to file

## Tools Used
- `brave_search` - Search the web
- `filesystem_read` - Read web content

## Expected Output
A markdown file with research summary.
```

---

## 🌐 Agent Skills (Vercel skills.sh Compatible)

skills.rs is **fully compatible** with the [Agent Skills format](https://agentskills.io/specification) pioneered by Vercel. You can import and use skills from the growing ecosystem of Agent Skills repositories.

### What is Agent Skills?

Agent Skills is an open standard for packaging AI agent instructions and tools. A skill is a directory containing:
- **SKILL.md** with YAML frontmatter (required)
- **scripts/** for bundled executables (optional)
- **references/** for supporting documents (optional)
- **assets/** for additional files (optional)

### Quick Start: Import Skills

Import skills directly from GitHub repositories:

```bash
# Import all skills from a repository
skills add vercel-labs/agent-skills

# Import specific skill(s)
skills add wshobson/agents --skill monorepo-management

# Import from full GitHub URL
skills add https://github.com/owner/repo --skill skill-name

# Import multiple skills
skills add vercel-labs/agent-skills --skill web-design-guidelines --skill vercel-react-best-practices

# Specify git ref (branch, tag, or commit)
skills add owner/repo --skill my-skill --git-ref v1.0.0

# Force overwrite existing skills
skills add owner/repo --force
```

### Auto-Sync from Configuration

Add Agent Skills repositories to your `config.yaml` for automatic synchronization:

```yaml
agent_skills_repos:
  # Import specific skills from Vercel's repository
  - repo: "vercel-labs/agent-skills"
    skills:
      - "web-design-guidelines"
      - "vercel-react-best-practices"
    # Optional: specify git ref
    # git_ref: "main"

  # Import all skills from a repository
  - repo: "wshobson/agents"
    # Omit 'skills' to import all

  # Full GitHub URL with version pinning
  - repo: "https://github.com/owner/repo"
    skills:
      - "monorepo-management"
    git_ref: "v1.0.0"
```

Skills are automatically synced on server startup and can be manually synced with:

```bash
skills sync
```

**Sync behavior**:
- ✅ **Adds** new skills from configured repositories
- 🔄 **Updates** existing skills when commit SHA changes
- 🗑️ **Removes** skills from repositories deleted from config
- 🔍 **Validates** all skills against Agent Skills specification

### Agent Skills Format Example

Here's a minimal Agent Skill (`SKILL.md`):

```markdown
---
name: pdf-processing
description: Extract text and tables from PDF files using Python tools
license: MIT
compatibility: Works best with Claude and GPT-4
metadata:
  author: your-name
  version: "1.0.0"
allowed-tools: Bash(python3) Read Write
---

# PDF Processing

Extract and process content from PDF files.

## Purpose
This skill helps you extract text, tables, and metadata from PDF documents.

## Steps
1. Use the `extract.py` script to process the PDF
2. Parse the extracted text
3. Save results to a structured format

## Tools Used
- `python3` - Run the extraction script
- File system tools for reading/writing

## Expected Output
JSON file containing extracted text and tables.
```

With optional `scripts/extract.py`:

```python
#!/usr/bin/env python3
import sys
import json

# PDF extraction logic here
```

### Compatibility Matrix

skills.rs supports all Agent Skills features:

| Feature | Supported | Notes |
|---------|-----------|-------|
| YAML frontmatter | ✅ | Full spec compliance |
| Name validation | ✅ | Lowercase, hyphens, 1-64 chars |
| Field constraints | ✅ | Description ≤1024, compatibility ≤500 |
| scripts/ | ✅ | Python, Bash, Node.js |
| references/ | ✅ | Progressive disclosure |
| assets/ | ✅ | Progressive disclosure |
| allowed-tools | ✅ | Maps to tool_policy.allow |
| Recursive discovery | ✅ | Finds skills in nested directories |
| Mixed formats | ✅ | Agent Skills + skills.rs formats coexist |

### Format Detection

skills.rs automatically detects skill format:

- **Has `SKILL.md` only**: Agent Skills format
- **Has `skill.json` + `SKILL.md`**: Traditional skills.rs format
- **Has both**: Traditional format takes precedence

Skills are seamlessly converted to the unified internal format and available through all 4 MCP tools.

### Progressive Disclosure

Agent Skills leverage progressive disclosure to minimize token usage:

**Level 1** (always loaded): Name, description, metadata  
**Level 2** (on-demand): SKILL.md content  
**Level 3** (on-demand): references/, assets/ files  
**Level 4** (execution): scripts/ bundled tools

Use `get_content` to load additional content:

```json
{
  "skill_id": "pdf-processing",
  "filename": "references/guide.md"  // optional
}
```

### Differences from skills.sh

While fully compatible, skills.rs adds enterprise features:

| Feature | skills.sh | skills.rs |
|---------|-----------|-----------|
| Skill import | ✅ | ✅ |
| GitHub shorthand | ✅ | ✅ |
| Config-based sync | ❌ | ✅ |
| Auto cleanup | ❌ | ✅ |
| MCP server mode | ❌ | ✅ |
| Sandboxing | ❌ | ✅ |
| SQLite persistence | ❌ | ✅ |
| Tool validation | ❌ | ✅ |
| Risk assessment | ❌ | ✅ |

### CLI Commands

```bash
# Import skills
skills add <owner/repo> [--skill <name>] [--git-ref <ref>] [--force]

# Sync from config
skills sync

# List all skills (including Agent Skills)
skills list

# Search for skills
skills grep "*pdf*"

# Get skill schema
skills tool local/pdf-processing

# Show system paths
skills paths
```

### Telemetry

Unlike Vercel's `skills.sh`, skills.rs does **not** send telemetry data. All skill operations are local and private.

---

## 🛡️ Security & Sandboxing

### Sandbox Backends

skills.rs supports multiple sandboxing backends:

| Backend | Security Level | Platform | Use Case |
|---------|---------------|----------|----------|
| `none` | ⚠️ None | All | Development only |
| `timeout` | 🟡 Basic | All | Basic timeout enforcement |
| `restricted` | 🟠 Medium | Unix | Resource limits, temp dir isolation, proxy-based network blocking |
| `bubblewrap` | 🟢 High | Linux | Container isolation (recommended) |
| `wasm` | 🔵 High | All | Future: WASM runtime |

### Configuration Examples

**Development:**
```yaml
sandbox:
  backend: timeout
  timeout_ms: 60000
```

**Production (Linux):**
```yaml
sandbox:
  backend: bubblewrap
  timeout_ms: 30000
  max_memory_bytes: 536870912  # 512MB
  max_cpu_seconds: 30
  allow_network: false
```

### Security Features

✅ **Resource Limits** - CPU, memory, file descriptors  
✅ **Timeout Enforcement** - Prevents runaway scripts  
✅ **Path Traversal Protection** - Validates all file paths  
✅ **Circular Dependency Detection** - Prevents infinite loops  
✅ **Environment Sanitization** - Removes dangerous env vars  
✅ **Network Blocking** - Proxy-based network blocking (use Bubblewrap for strong isolation)  
✅ **Execution Auditing** - All executions logged to database

---

## 💾 Persistence

All data is persisted to SQLite:

- **Callable Registry** - Tools and skills with metadata
- **Execution History** - Complete audit trail
- **Server State** - Configuration and runtime state

```yaml
persistence:
  enabled: true
  database: "./data/skills.db"
  prune_after_days: 30
```

Query execution history:
```rust
let history = persistence.get_execution_history(&callable_id, 100).await?;
```

---

## 🧪 Testing

### Run Tests

```bash
# All tests (30 passing)
cargo test --workspace --all-features

# Unit tests only
cargo test --workspace --lib

# Integration tests
cargo test --test integration_test

# Specific crate
cargo test -p skillsrs-skillstore
```

### Test Coverage

- ✅ **30 tests passing**
- ✅ Unit tests for all core functionality
- ✅ 7 integration tests for full lifecycle
- ✅ Sandbox backend tests
- ✅ Validation tests
- ✅ Persistence tests

---

## 📊 Performance

| Operation | Time | Notes |
|-----------|------|-------|
| Skill search | <10ms | Tantivy index |
| Registry lookup | <1ms | HashMap |
| Content loading | ~1ms | Single file read |
| Bundled tool (Python) | 50-200ms | Interpreter startup |
| Persistence save | ~2ms | SQLite insert |

**Tested at scale:**
- 100 skills: No degradation
- 1,000 callables: <1ms lookup
- 10,000 execution records: <10ms query

---

## 🚢 Deployment

### Docker

```dockerfile
FROM rust:1.70 AS builder
WORKDIR /app
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y \
    python3 bash bubblewrap ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/skills /usr/local/bin/skills
COPY config.yaml /etc/skills/config.yaml

EXPOSE 8000
CMD ["skills", "http", "--config", "/etc/skills/config.yaml"]
```

```bash
docker build -t skills:latest .
docker run -p 8000:8000 -v ./skills:/var/lib/skills skills:latest
```

### System Requirements

- **OS:** Linux (recommended), macOS, Windows
- **CPU:** 1+ cores
- **RAM:** 512MB+ (1GB+ recommended)
- **Disk:** 100MB+ for binary, varies for skills
- **Dependencies:**
  - Python 3.8+ (for .py bundled tools)
  - Bash 4.0+ (for .sh bundled tools)
  - bubblewrap (for container sandboxing on Linux)

---

## 📚 Documentation

### LLM Agent Prompts
- **[PROMPT_CLI.md](./PROMPT_CLI.md)** - System prompt for AI agents using `skills` CLI (~300 Tokens)
- **[PROMPT_MCP.md](./PROMPT_MCP.md)** - System prompt for AI agents using skills.rs as MCP server (~390 Tokens but may not be necessary)

### Guides
- **[QUICKSTART.md](./QUICKSTART.md)** - Step-by-step getting started guide
- **[OPERATIONS.md](./OPERATIONS.md)** - Complete operations guide (deployment, configuration, CLI usage)
- **[CHANGELOG.md](./CHANGELOG.md)** - Version history and changes
- **[config.example.yaml](docs/config.example.yaml)** - Full configuration reference

---

## 🏗️ Architecture

```
┌─────────────────────────────────────────┐
│        MCP Client (Agent)               │
└──────────────┬──────────────────────────┘
               │ MCP Protocol
┌──────────────▼──────────────────────────┐
│      SkillsServer (4 MCP Tools)         │
└──┬──────┬──────┬──────┬─────────────────┘
   │      │      │      │
┌──▼──┐ ┌─▼──┐ ┌▼───┐ ┌▼────────┐
│Reg. │ │Srch│ │Run │ │SkillStor│
└──┬──┘ └──┬─┘ └─┬──┘ └──┬──────┘
   │       │     │       │
   │       │  ┌──▼──┐    │
   │       │  │Sdbx │    │
   │       │  └─────┘    │
┌──▼───────▼─────────────▼──────┐
│   Persistence (SQLite)         │
└────────────────────────────────┘
```

---

## 🤝 Contributing

Contributions welcome! Please:

1. Fork the repository
2. Create a feature branch
3. Add tests for new features
4. Ensure all tests pass: `cargo test --workspace`
5. Submit a pull request

---

## 📜 License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE](LICENSE))

at your option.

---

## 🎉 Status

✅ **Production Ready**  
✅ **30 Tests Passing**  
✅ **Zero Known Blockers**  
✅ **Comprehensive Documentation**  
✅ **Security Hardened**

**Ready for deployment.**

---

## 💬 Support

- **Issues:** [GitHub Issues](https://github.com/labiium/skills/issues)
- **Discussions:** [GitHub Discussions](https://github.com/labiium/skills/discussions)
- **Documentation:** See `docs/` directory

---

*Built with 🦀 Rust | Powered by the MCP Protocol*
