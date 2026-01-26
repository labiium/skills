# Infinite Skills, Finite Context: Introducing skills.rs

**The missing piece in your AI agent toolkit has arrived.**

If you've ever built an AI agent, you know the struggle: your agent needs access to dozens of tools, but every tool description you add eats into your precious context window. You're forced to choose between giving your agent breadth (many tools) or depth (detailed instructions). 

**What if you didn't have to choose?**

Enter **skills.rs** — a unified MCP server that solves the context window crisis while unlocking unlimited tool discovery for your AI agents.

---

## The Problem: Context Window Bankruptcy

Modern AI agents operate through the Model Context Protocol (MCP), connecting to various servers that provide tools like web search, file operations, database queries, and more. The standard workflow looks like this:

1. Load all available tools into context
2. Agent reviews hundreds of lines of tool descriptions
3. Agent finally selects the right tool
4. Context window is now 90% full with tool metadata

This doesn't scale. As you add more capabilities, you quickly hit context limits. The agent spends more tokens *looking at tools* than actually solving problems.

**The metrics are startling:**
- Typical MCP setup: 10-20 tools = 2,000-5,000 tokens just for tool descriptions
- With 50+ tools: Your context is consumed before the agent even starts working
- Result: Agents become slower, less capable, and more expensive

---

## The Solution: Progressive Disclosure

skills.rs introduces a radical approach: **expose 7 focused meta-tools instead of hundreds of individual tools.**

Instead of loading every tool description into context, agents can:

1. **Search** for tools on-demand (`search`)
2. **Inspect** only the tools they need (`schema`)
3. **Execute** with validation and sandboxing (`exec`)

This achieves a **99% token reduction** in tool metadata while enabling *unlimited* tool discovery.

### How It Works

```
Traditional Approach:
┌────────────────────────────────────────┐
│ Agent Context (8K tokens)              │
│ ┌────────────────────────────────────┐ │
│ │ Tool 1: read_file (80 tokens)      │ │
│ │ Tool 2: write_file (90 tokens)     │ │
│ │ Tool 3: brave_search (120 tokens)  │ │
│ │ Tool 4: sql_query (150 tokens)     │ │
│ │ ... 50 more tools ...              │ │
│ │ (5,000+ tokens consumed)           │ │
│ └────────────────────────────────────┘ │
│ Task: "Search for latest AI news"     │
│ (Only 3K tokens remaining!)            │
└────────────────────────────────────────┘

skills.rs Approach:
┌────────────────────────────────────────┐
│ Agent Context (8K tokens)              │
│ ┌────────────────────────────────────┐ │
│ │ 7 Meta-tools (350 tokens)          │ │
│ │   - search                  │ │
│ │   - schema                  │ │
│ │   - exec                    │ │
│ │   - create                  │ │
│ │   - get_content             │ │
│ │   - update                  │ │
│ │   - delete                  │ │
│ └────────────────────────────────────┘ │
│ Task: "Search for latest AI news"     │
│ (7,650+ tokens available!)             │
└────────────────────────────────────────┘
```

---

## Two Modes of Operation

skills.rs is designed for flexibility, supporting both traditional MCP server mode and a powerful CLI mode.

### Mode 1: MCP Server (Progressive Disclosure)

Run skills.rs as an MCP server that aggregates multiple upstream servers and exposes 7 focused meta-tools:

```bash
skills stdio
```

Your AI agent connects via MCP protocol and uses progressive disclosure:

```javascript
// Step 1: Search for tools (lightweight)
{
  "tool": "search",
  "args": { "q": "search web", "kind": "tool", "limit": 5 }
}
// Returns: 5 matching tools with minimal metadata

// Step 2: Get schema for the right tool
{
  "tool": "schema",
  "args": { "id": "tool://brave_search@1.0" }
}
// Returns: Full JSON schema only for this tool

// Step 3: Execute
{
  "tool": "exec",
  "args": {
    "id": "tool://brave_search@1.0",
    "arguments": {"query": "latest AI news"}
  }
}
```

### Mode 2: CLI Agent Interface (Drop-in mcp-cli Replacement)

skills.rs includes a powerful CLI that works as a drop-in replacement for `mcp-cli`, but with production-grade features:

```bash
# List all servers and tools
skills list

# Search for specific tools
skills grep "*file*"

# Get tool schema
skills tool filesystem/read_file

# Execute a tool
skills tool filesystem/read_file '{"path": "./README.md"}'
```

**Why use skills.rs CLI instead of mcp-cli?**

| Feature | mcp-cli | skills.rs |
|---------|---------|-----------|
| Token reduction | ✓ | ✓ |
| CLI interface | ✓ | ✓ |
| Execution persistence | ✗ | ✓ |
| Sandboxed execution | ✗ | ✓ |
| Skills system | ✗ | ✓ |
| Can also run as MCP server | ✗ | ✓ |
| Audit logging | ✗ | ✓ |

---

## The Skills System: Teaching Agents New Tricks

Beyond tool aggregation, skills.rs introduces a **skills system** — reusable packages of instructions and tools that agents can learn on-demand.

### What is a Skill?

A skill is a directory containing:

- **skill.json** - Manifest with inputs, outputs, and metadata
- **SKILL.md** - Natural language instructions for the agent
- **Optional bundled scripts** - Python, Bash, or other executables
- **Support files** - Data, schemas, documentation

### Example: Web Researcher Skill

```
skills/web-researcher/
├── skill.json             # Metadata and schema
├── SKILL.md               # Instructions for agent
├── search.py              # Bundled Python tool
└── search.py.schema.json  # Tool schema
```

**skill.json:**
```json
{
  "id": "web-researcher",
  "version": "1.0.0",
  "description": "Research topics using web search",
  "inputs": {
    "type": "object",
    "properties": {
      "query": {"type": "string"}
    }
  },
  "entrypoint": "prompted",
  "tool_policy": {
    "allow": ["brave_search"]
  }
}
```

**SKILL.md:**
```markdown
# Web Researcher

## Purpose
Research topics comprehensively using web search.

## Instructions
1. Use `brave_search` to find relevant articles
2. Read top 3 results
3. Synthesize findings into summary
4. Save summary to markdown file

## Expected Output
A markdown file with researched topic summary.
```

### Progressive Skill Loading

Skills use the same progressive disclosure pattern:

**Level 1:** Metadata only (name, description, tags)  
**Level 2:** Full instructions (`SKILL.md`) loaded on-demand  
**Level 3:** Bundled scripts loaded when needed  
**Level 4:** Execution with validation and sandboxing

This means an agent can have access to **hundreds of skills** while consuming minimal context tokens until it actually needs the details.

---

## Production-Grade Security

Unlike toy MCP servers, skills.rs is built for production with comprehensive security features:

### Multi-Backend Sandboxing

| Backend | Security | Platform | Use Case |
|---------|----------|----------|----------|
| `timeout` | Basic | All | Development |
| `restricted` | Medium | Unix | Resource limits |
| `bubblewrap` | High | Linux | Container isolation |
| `wasm` | High | All | Future WASM runtime |

### Security Features

✅ **Resource Limits** - Configurable CPU, memory, file descriptors  
✅ **Timeout Enforcement** - Prevents runaway scripts  
✅ **Path Traversal Protection** - Validates all file paths  
✅ **Network Isolation** - Optional network blocking  
✅ **Environment Sanitization** - Removes dangerous variables  
✅ **Execution Auditing** - Complete audit trail in SQLite  
✅ **Input Validation** - JSON Schema validation on all inputs  

### Example: Production Configuration

```yaml
sandbox:
  backend: bubblewrap
  timeout_ms: 30000
  max_memory_bytes: 536870912  # 512MB
  max_cpu_seconds: 30
  allow_network: false

persistence:
  enabled: true
  database: "./data/skills.db"
  prune_after_days: 30
```

---

## Built with Rust, Built for Speed

skills.rs is written in pure Rust with performance as a first-class concern:

| Operation | Time | Scale |
|-----------|------|-------|
| Skill search | <10ms | Tantivy full-text index |
| Registry lookup | <1ms | Optimized HashMap |
| Content loading | ~1ms | Single file read |
| Bundled tool execution | 50-200ms | Interpreter startup |
| Persistence save | ~2ms | SQLite insert |

**Tested at scale:**
- ✓ 100 skills: No degradation
- ✓ 1,000 callables: <1ms lookup
- ✓ 10,000 execution records: <10ms query

---

## Getting Started in 5 Minutes

### Install

```bash
# Install from crates.io
cargo install skillsrs

# Or install from GitHub
cargo install --git https://github.com/labiium/skills

# Or build from source
git clone https://github.com/labiium/skills
cd skills
cargo build --release
```

### Configure

Create `~/.config/skills/config.yaml`:

```yaml
server:
  transport: stdio
  log_level: info

sandbox:
  backend: timeout
  timeout_ms: 30000

upstreams:
  - alias: brave
    transport: stdio
    command: ["npx", "-y", "@modelcontextprotocol/server-brave-search"]
```

### Run

```bash
# As MCP server
skills stdio

# Or as CLI
skills list
skills grep "search"
skills tool brave_search/search '{"query": "rust MCP"}'
```

---

## Real-World Use Cases

### 1. Multi-Tool AI Agents

**Problem:** Your agent needs access to 50+ tools across multiple MCP servers.

**Solution:** skills.rs aggregates all upstream servers into one unified interface with progressive tool discovery.

```yaml
upstreams:
  - alias: filesystem
    command: ["mcp-server-filesystem"]
  - alias: brave
    command: ["mcp-server-brave-search"]
  - alias: database
    command: ["mcp-server-postgres"]
  - alias: git
    command: ["mcp-server-git"]
```

Agent now has access to 50+ tools while consuming only 350 tokens.

### 2. Reusable Agent Workflows

**Problem:** You've trained your agent on a complex workflow, but can't reuse it efficiently.

**Solution:** Package the workflow as a skill that any agent can load on-demand.

```bash
skills create \
  --name code-reviewer \
  --description "Review code changes for bugs and style issues" \
  --entrypoint prompted
```

### 3. Secure Code Execution

**Problem:** Your agent needs to run user-provided code, but you can't trust it.

**Solution:** skills.rs sandboxes all script execution with configurable resource limits.

```yaml
sandbox:
  backend: bubblewrap
  max_memory_bytes: 536870912
  max_cpu_seconds: 10
  allow_network: false
```

### 4. Team Skill Libraries

**Problem:** Your team has built many custom tools, but there's no centralized way to share them.

**Solution:** Build a shared skills repository that all team agents can access.

```
skills/
  data-analysis/
  code-generation/
  documentation/
  testing/
  deployment/
```

---

## Why skills.rs Matters

The Model Context Protocol is revolutionizing how AI agents access tools and data. But as the ecosystem grows, we face a new challenge: **tool sprawl**.

skills.rs solves this by introducing a **meta-layer** that makes infinite tool access practical while respecting context window constraints.

This unlocks new possibilities:

🚀 **Agents with 100+ tools** without context bloat  
🔍 **Dynamic tool discovery** instead of static configurations  
📦 **Reusable skills** that agents can learn on-demand  
🛡️ **Production-grade security** with sandboxing and auditing  
⚡ **Blazing fast performance** thanks to Rust  

---

## Join the Movement

skills.rs is production-ready and battle-tested:

✅ **30 comprehensive tests passing**  
✅ **Zero known blockers**  
✅ **Complete documentation**  
✅ **MIT/Apache-2.0 licensed**  

### Get Started Today

- **Repository:** [github.com/labiium/skills](https://github.com/labiium/skills)
- **Documentation:** Full guides in repo
- **Installation:** `cargo install skillsrs`

### Contribute

We welcome contributions! Check out our:
- [GitHub Issues](https://github.com/labiium/skills/issues)
- [GitHub Discussions](https://github.com/labiium/skills/discussions)

---

## The Future is Modular

AI agents are getting smarter, but they're also getting hungrier for context. skills.rs ensures your agents can scale to hundreds of tools and skills without hitting context limits.

**Progressive disclosure isn't just an optimization — it's the future of agent architectures.**

Ready to give your AI agents infinite skills with finite context?

```bash
cargo install skillsrs
skills stdio
```

---

*Built with 🦀 Rust | Powered by the Model Context Protocol*

**skills.rs** — Infinite Skills. Finite Context.