# Quick Start Guide

Get skills.rs running in 5 minutes.

---

## 1. Install

```bash
# Clone repository
git clone https://github.com/labiium/skills
cd skills

# Build release binary
cargo build --release

# Verify installation
./target/release/skills --version
```

---

## 2. Initialize Project Config

Initialize a project-local setup (run in the repo root):

```bash
./target/release/skills init
```

This creates:

- `.skills/config.yaml`
- `.skills/skills/`
- `.skills/skills.db`

Edit `.skills/config.yaml` as needed. Example:

```yaml
paths:
  data_dir: ".skills"
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

upstreams:
  - alias: brave
    transport: stdio
    command: ["npx", "-y", "@modelcontextprotocol/server-brave-search"]
    tags: ["search", "web"]
```

To disable sandboxing entirely:

```yaml
sandbox:
  backend: none
```

---

## 3. Create Your First Skill

```bash
# Create skills directory
mkdir -p .skills/skills/my-first-skill

# Create SKILL.md with YAML frontmatter
cat > .skills/skills/my-first-skill/SKILL.md << 'EOF'
---
name: my-first-skill
description: A simple test skill
version: 1.0.0
allowed-tools: ["brave_search"]
tags: ["demo", "test"]
---

# My First Skill

## Purpose
A simple test skill that demonstrates the SKILL.md format.

## Instructions
1. Read the input message
2. Optionally search the web for related information
3. Return a response

## Tools Used
- `brave_search` (optional) - Search the web for context

## Expected Output
A text response acknowledging the message.
EOF
```

---

## 4. Run the Server

**Stdio mode (for MCP clients):**
```bash
./target/release/skills server stdio
```

**HTTP mode (for testing):**
```bash
./target/release/skills server http --bind 127.0.0.1:8000
```

The server will:
- Load all skills from `./skills/`
- Connect to upstream MCP servers
- Expose 4 MCP tools

---

## 5. Test with MCP Tools

### Search for your skill

**Tool:** `search`

**Input:**
```json
{
  "q": "first",
  "kind": "skill",
  "limit": 10
}
```

**Expected:** Should find "my-first-skill"

### Get skill content

**Tool:** `manage`

**Input:**
```json
{
  "operation": "get",
  "skill_id": "my-first-skill"
}
```

**Expected:** Returns SKILL.md content

### Execute the skill

**Tool:** `exec`

**Input:**
```json
{
  "id": "skill://my-first-skill@1.0.0@<digest>",
  "arguments": {
    "message": "Hello, world!"
  },
  "timeout_ms": 5000
}
```

**Expected:** Skill executes successfully

---

## 6. Add a Bundled Script (Optional)

Create a Python script in your skill:

```bash
cat > skills/my-first-skill/process.py << 'EOF'
#!/usr/bin/env python3
import json
import os

# Read arguments
args = json.loads(os.environ.get('SKILL_ARGS_JSON', '{}'))
message = args.get('message', 'No message')

# Process
result = {
    "processed": True,
    "original_message": message,
    "response": f"Processed: {message.upper()}"
}

# Output as JSON
print(json.dumps(result))
EOF

chmod +x skills/my-first-skill/process.py
```

Restart the server - it will auto-detect the bundled tool.

---

## 7. Verify Everything Works

```bash
# Run all tests
cargo test --workspace --all-features

# Should see: "30 tests passing"
```

---

## Next Steps

- **Add more skills** - Create additional skill directories
- **Configure upstreams** - Connect to more MCP servers
- **Enable sandboxing** - Use `restricted` or `bubblewrap` backend
- **Enable persistence** - Add database configuration
- **Read the docs:**
  - [README.md](README.md) - Full feature overview
  - [PRODUCTION_READY.md](PRODUCTION_READY.md) - Deployment guide
  - [HANDOFF.md](HANDOFF.md) - Architecture details

---

## Troubleshooting

**Skills not loading?**
```bash
# Check directory
ls -la ./skills

# Run with debug logging
RUST_LOG=debug ./target/release/skills server stdio --config config.yaml
```

**Upstream connection failed?**
```bash
# Test upstream manually
npx -y @modelcontextprotocol/server-brave-search

# Check logs for connection errors
```

**Bundled script not executing?**
```bash
# Test script directly
cd .skills/skills/my-first-skill
export SKILL_ARGS_JSON='{"message":"test"}'
python3 process.py

# Check sandbox configuration
# Use backend: timeout for development
```

---

## Configuration Quick Reference

| Setting | Development | Production |
|---------|-------------|------------|
| `sandbox.backend` | `timeout` | `bubblewrap` |
| `sandbox.timeout_ms` | `60000` | `30000` |
| `server.log_level` | `debug` | `info` |
| `persistence.enabled` | `false` | `true` |

---

**You're all set! 🎉**

For more details, see [README.md](README.md) or [PRODUCTION_READY.md](PRODUCTION_READY.md).