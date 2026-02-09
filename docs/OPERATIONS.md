# Operations Guide

Complete guide for deploying, configuring, and operating skills.rs in production and development environments.

## Table of Contents

1. [Installation](#installation)
2. [Directory Structure](#directory-structure)
3. [Configuration](#configuration)
4. [Running Skills.rs](#running-skillsrs)
5. [Sandboxing and Security](#sandboxing-and-security)
6. [CLI Usage (mcp-cli Replacement)](#cli-usage-mcp-cli-replacement)
7. [Docker Deployment](#docker-deployment)
8. [Monitoring and Maintenance](#monitoring-and-maintenance)
9. [Troubleshooting](#troubleshooting)

---

## Installation

### From Source (Recommended)

```bash
cargo install --git https://github.com/labiium/skills
```

This installs the `skills` binary to `~/.cargo/bin/` which should be in your PATH.

### Pre-built Binaries

Download from [GitHub Releases](https://github.com/labiium/skills/releases):

```bash
# Linux x86_64
curl -L https://github.com/labiium/skills/releases/latest/download/skills-linux-x86_64 -o skills
chmod +x skills
sudo mv skills /usr/local/bin/

# macOS (Intel)
curl -L https://github.com/labiium/skills/releases/latest/download/skills-macos-x86_64 -o skills
chmod +x skills
sudo mv skills /usr/local/bin/

# macOS (Apple Silicon)
curl -L https://github.com/labiium/skills/releases/latest/download/skills-macos-aarch64 -o skills
chmod +x skills
sudo mv skills /usr/local/bin/
```

### Docker

```bash
docker pull ghcr.io/labiium/skills:latest
```

---

## Directory Structure

Skills.rs follows platform-specific conventions for storing data, configuration, and cache files.

### Default Locations

**Linux (XDG Base Directory):**
```
~/.local/share/skills/          # Data directory
├── skills/                     # Skills storage
├── skills.db                   # SQLite database
└── logs/                       # Log files

~/.config/skills/               # Configuration directory
└── config.yaml                 # Main config file

~/.cache/skills/                # Cache directory
```

**macOS:**
```
~/Library/Application Support/skills/    # Data directory
├── skills/                              # Skills storage
├── skills.db                            # SQLite database
└── logs/                                # Log files

~/Library/Preferences/skills/            # Configuration directory
└── config.yaml                          # Main config file

~/Library/Caches/skills/                 # Cache directory
```

**Windows:**
```
%APPDATA%\labiium\skills\        # Data directory
├── skills\                      # Skills storage
├── skills.db                    # SQLite database
└── logs\                        # Log files

%APPDATA%\labiium\skills\config\ # Configuration directory
└── config.yaml                  # Main config file

%LOCALAPPDATA%\labiium\skills\cache\  # Cache directory
```

### View Current Paths

```bash
skills paths
```

Output shows all active directories and their locations.

### Custom Paths

Override default locations using command-line arguments, environment variables, or configuration:

**Command-line:**
```bash
skills --data-dir /custom/data server
skills --skills-root /custom/skills list
skills --database /custom/db/skills.db server
```

**Environment variables:**
```bash
export SKILLS_DATA_DIR=/custom/data
export SKILLS_ROOT=/custom/skills
export SKILLS_DATABASE_PATH=/custom/db/skills.db
export SKILLS_CONFIG_DIR=/etc/skills
export SKILLS_CACHE_DIR=/var/cache/skills
export SKILLS_LOGS_DIR=/var/log/skills
```

**Configuration file (`config.yaml`):**
```yaml
paths:
  data_dir: "/var/lib/skills"
  config_dir: "/etc/skills"
  cache_dir: "/var/cache/skills"
  database_path: "/var/lib/skills/skills.db"
  skills_root: "/var/lib/skills/skills"
  logs_dir: "/var/log/skills"
```

### Configuration Priority

Paths are resolved in this order (highest to lowest):
1. Command-line arguments
2. Environment variables
3. Configuration file
4. System defaults

---

## Configuration

### Configuration File Location

Skills.rs configuration is discovered in this order:
1. Path specified with `--config` flag
2. Nearest `.skills/config.yaml` found by walking up from the current directory (project-local)
3. System config directory `config.yaml` (global)

To force using global config only (ignore project `.skills/config.yaml`), use `--global`.

### Basic Configuration

Create a project-local config at `.skills/config.yaml` (recommended), or use the global config in the system config directory (varies by platform).

```yaml
# Upstream MCP servers
upstreams:
  - alias: filesystem
    transport: stdio
    command:
      - npx
      - "-y"
      - "@modelcontextprotocol/server-filesystem"
      - "."
    tags:
      - filesystem
      - local

  - alias: github
    transport: http
    url: "https://api.github.com/mcp"
    auth:
      type: bearer
      env: GITHUB_TOKEN
    tags:
      - github
      - remote

# Sandbox configuration (optional)
sandbox:
  backend: restricted  # none, timeout, restricted, bubblewrap
  timeout_ms: 30000
  max_memory_bytes: 536870912

# Policy configuration (optional)
policy:
  default_risk: read_only
  require_consent_for:
    - writes
    - destructive
  trusted_servers:
    - filesystem
```

### MCP Server Configuration (Quick Reference)

In Skills.rs, **upstreams** are MCP servers that provide tools to your application.

#### Common MCP Server Patterns

| Server Type | Transport | Example |
|-------------|-----------|---------|
| npm-based | stdio | filesystem, brave-search |
| Local binary | stdio | custom tools |
| Remote API | http | GitHub, custom APIs |

#### Configuration Examples

**npm-based MCP server (stdio):**
```yaml
upstreams:
  - alias: filesystem
    transport: stdio
    command: ["npx", "-y", "@modelcontextprotocol/server-filesystem", "."]
```

**Local binary (stdio):**
```yaml
upstreams:
  - alias: custom-tool
    transport: stdio
    command: ["/usr/local/bin/my-mcp-server"]
```

**HTTP with bearer token authentication:**
```yaml
upstreams:
  - alias: github
    transport: http
    url: "https://api.github.com/mcp"
    auth:
      type: bearer
      env: GITHUB_TOKEN
```

**HTTP with header authentication:**
```yaml
upstreams:
  - alias: api-service
    transport: http
    url: "https://api.example.com/mcp"
    auth:
      type: header
      header_name: X-API-Key
      env: API_KEY
```

#### Security Note

Configure per-server sandboxing for untrusted servers. See [Sandboxing and Security](#sandboxing-and-security) for details on:
- Timeout limits
- Memory restrictions
- Network access control
- Filesystem isolation

#### Complete Reference

For detailed configuration options, authentication methods, and advanced settings, see [MCP_GUIDE.md](MCP_GUIDE.md).

### Full Configuration Reference

See [config.example.yaml](config.example.yaml) for all available options.

---

## Running Skills.rs

Skills.rs operates in two modes:

### 1. CLI Mode (Direct Tool Access)

Use skills.rs as a command-line interface for direct tool execution:

```bash
# List all servers and tools
skills list
skills list -d  # with descriptions
skills list filesystem  # specific server

# Get tool schema
skills tool filesystem/read_file

# Execute a tool
skills tool filesystem/read_file '{"path": "./README.md"}'
skills exec filesystem/read_file '{"path": "./README.md"}'  # alias

# Search for tools
skills grep "*file*"

# Show current paths
skills paths
```

**Default behavior:** CLI mode runs **without sandboxing** by default for convenience.

### 2. Server Mode (MCP Server)

Run as an MCP server to expose skills and tools to AI agents:

```bash
# stdio transport (recommended)
skills server stdio

# HTTP transport
skills server http --bind 0.0.0.0:8000
```

**Default behavior:** Server mode runs **with sandboxing** by default for security.

### Running as a Service

**Linux (systemd):**

Create `/etc/systemd/system/skills.service`:

```ini
[Unit]
Description=Skills.rs - Infinite Skills. Finite Context.
After=network.target

[Service]
Type=simple
User=skills
Environment=SKILLS_DATA_DIR=/var/lib/skills
Environment=SKILLS_CONFIG_DIR=/etc/skills
ExecStart=/usr/local/bin/skills server stdio
Restart=on-failure
RestartSec=10

[Install]
WantedBy=multi-user.target
```

Enable and start:
```bash
sudo systemctl enable skills
sudo systemctl start skills
sudo systemctl status skills
```

**macOS (launchd):**

Create `~/Library/LaunchAgents/com.labiium.skills.plist`:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.labiium.skills</string>
    <key>ProgramArguments</key>
    <array>
        <string>/usr/local/bin/skills</string>
        <string>server</string>
        <string>stdio</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
</dict>
</plist>
```

Load:
```bash
launchctl load ~/Library/LaunchAgents/com.labiium.skills.plist
```

---

## Sandboxing and Security

### Understanding Sandboxing

Skills.rs provides multiple sandboxing backends to isolate tool execution:

- **none** - No sandboxing (full system access)
- **timeout** - Timeout enforcement only
- **restricted** - Limited filesystem/network access (default for server mode)
- **bubblewrap** - Linux container isolation (requires bubblewrap)

### Default Behavior

| Mode | Default Sandbox |
|------|----------------|
| CLI (`skills tool`, `skills list`) | **None** |
| Server (`skills server stdio`) | **Restricted** |

**Rationale:**
- CLI mode is for trusted direct interaction
- Server mode aggregates potentially untrusted upstreams

### Disabling Sandboxing

⚠️ **Security Warning:** Only disable sandboxing in trusted environments!

**CLI flag:**
```bash
skills server stdio --no-sandbox
```

**Environment variable:**
```bash
export SKILLS_NO_SANDBOX=1
skills server stdio
```

**Configuration file:**
```yaml
sandbox:
  backend: none
```

**Current directory mode:**
```bash
# Uses current directory AND disables sandbox
skills --current-dir tool filesystem/read_file '{"path": "./README.md"}'
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
```

### Security Best Practices

1. **Default to Sandboxing** - Only disable when necessary
2. **Trust Boundaries** - Use per-server sandbox configuration
3. **Least Privilege** - Allow minimum required access
4. **Audit Logs** - Monitor execution history for suspicious activity
5. **Resource Limits** - Set timeouts even without full sandboxing
6. **Network Isolation** - Disable network for tools that don't need it

### Policy Engine

Configure execution policies based on risk tiers:

```yaml
policy:
  default_risk: read_only
  require_consent_for:
    - writes
    - destructive
  trusted_servers:
    - filesystem
    - local-tool
  
  risk_overrides:
    "github/delete_repository": destructive
    "filesystem/write_file": limited_write
```

Risk tiers:
- **read_only** - No modifications allowed
- **limited_write** - Limited write operations
- **full_access** - Unrestricted access
- **destructive** - Dangerous operations (requires explicit consent)

---

## CLI Usage (mcp-cli Replacement)

Skills.rs is a **drop-in replacement** for mcp-cli with additional features.

### Command Comparison

| Task | mcp-cli | skills.rs |
|------|---------|-----------|
| List all | `mcp-cli` | `skills list` |
| List with descriptions | `mcp-cli -d` | `skills list -d` |
| List server | `mcp-cli filesystem` | `skills list filesystem` |
| Get schema | `mcp-cli filesystem/read_file` | `skills tool filesystem/read_file` |
| Execute | `mcp-cli filesystem/read_file '{"path":"..."}' ` | `skills tool filesystem/read_file '{"path":"..."}'` |
| Search | `mcp-cli grep "*file*"` | `skills grep "*file*"` |

### Migration from mcp-cli

1. **Install skills.rs:**
   ```bash
   cargo install --git https://github.com/labiium/skills
   ```

2. **Convert config** from `mcp_servers.json` to `config.yaml`:

   **Before (mcp_servers.json):**
   ```json
   {
     "mcpServers": {
       "filesystem": {
         "command": "npx",
         "args": ["-y", "@modelcontextprotocol/server-filesystem", "."]
       }
     }
   }
   ```

   **After (config.yaml):**
   ```yaml
   upstreams:
     - alias: filesystem
       transport: stdio
       command:
         - npx
         - "-y"
         - "@modelcontextprotocol/server-filesystem"
         - "."
   ```

3. **Update commands:**
   - Replace `mcp-cli` with `skills` in all scripts
   - Change `mcp-cli <server>/<tool>` to `skills tool <server>/<tool>`

### Additional Features vs mcp-cli

Skills.rs provides these features not available in mcp-cli:

1. **Persistent Storage** - Tool metadata cached across invocations
2. **Execution History** - Full audit trail of all executions
3. **Skills (Compositions)** - Create reusable tool workflows
4. **Sandboxing** - Resource limits and isolation
5. **Policy Engine** - Risk-based access control
6. **Server Mode** - Act as an MCP server itself
7. **Better Performance** - Native binary, less memory usage

### AI Agent System Prompt

Replace mcp-cli references:

```markdown
## MCP Tool Access

You have access to MCP servers via the `skills` CLI.

### Available Commands

- `skills list` - List all servers and tools
- `skills list <server>` - Show server's tools
- `skills list -d` - Include descriptions
- `skills tool <server>/<tool>` - Get tool schema
- `skills tool <server>/<tool> '<json>'` - Execute tool
- `skills grep "<pattern>"` - Search by glob pattern

### Workflow

1. **Discover**: Run `skills list` or `skills grep "<pattern>"`
2. **Inspect**: Run `skills tool <server>/<tool>` to see schema
3. **Execute**: Run `skills tool <server>/<tool> '<json>'`

### Examples

```bash
# List all tools
skills list

# Search for file-related tools
skills grep "*file*"

# Get schema
skills tool filesystem/read_file

# Execute
skills tool filesystem/read_file '{"path": "./README.md"}'

# JSON output for parsing
skills tool filesystem/read_file '{"path": "./README.md"}' --json

# Raw text output
skills tool filesystem/read_file '{"path": "./README.md"}' --raw
```
```

---

## Docker Deployment

### Using Pre-built Images

```bash
# Pull latest image
docker pull ghcr.io/labiium/skills:latest

# Run with stdio
docker run -it \
  -v skills-data:/data \
  -v skills-config:/etc/skills \
  ghcr.io/labiium/skills:latest

# Run HTTP server
docker run -d \
  -p 8000:8000 \
  -v skills-data:/data \
  -v skills-config:/etc/skills \
  ghcr.io/labiium/skills:latest \
  server http --bind 0.0.0.0:8000
```

### Docker Compose

Create `docker-compose.yml`:

```yaml
version: '3.8'

services:
  skills:
    image: ghcr.io/labiium/skills:latest
    container_name: skills
    restart: unless-stopped
    ports:
      - "8000:8000"
    volumes:
      - skills-data:/data
      - skills-config:/etc/skills
      - ./skills:/data/skills:ro
    environment:
      - SKILLS_DATA_DIR=/data
      - SKILLS_CONFIG_DIR=/etc/skills
      - RUST_LOG=info
    command: server http --bind 0.0.0.0:8000

volumes:
  skills-data:
  skills-config:
```

Start:
```bash
docker-compose up -d
```

### Building Custom Images

```bash
# Clone repository
git clone https://github.com/labiium/skills
cd skills

# Build
docker build -t skills:custom .

# Run
docker run -it skills:custom
```

---

## Monitoring and Maintenance

### Logging

**Enable structured logging:**
```bash
export RUST_LOG=info
skills server stdio
```

Log levels: `error`, `warn`, `info`, `debug`, `trace`

**File logging** (configure in `config.yaml`):
```yaml
logging:
  level: info
  file: /var/log/skills/skills.log
  format: json  # or "text"
```

### Execution History

View execution history (stored in database):

```bash
# Query the database
sqlite3 ~/.local/share/skills/skills.db
```

```sql
-- Recent executions
SELECT * FROM executions ORDER BY started_at DESC LIMIT 10;

-- Failed executions
SELECT * FROM executions WHERE success = 0 ORDER BY started_at DESC;

-- Execution statistics
SELECT
  callable_id,
  COUNT(*) as total,
  SUM(CASE WHEN success = 1 THEN 1 ELSE 0 END) as successful,
  AVG(duration_ms) as avg_duration_ms
FROM executions
GROUP BY callable_id
ORDER BY total DESC;
```

### Health Checks

**HTTP endpoint:**
```bash
curl http://localhost:8000/health
```

**Database check:**
```bash
skills list > /dev/null && echo "OK" || echo "FAIL"
```

### Backup and Restore

**Backup essentials:**
```bash
# Database
cp ~/.local/share/skills/skills.db ~/backup/

# Skills directory
tar czf ~/backup/skills.tar.gz ~/.local/share/skills/skills/

# Configuration
cp ~/.config/skills/config.yaml ~/backup/
```

**Restore:**
```bash
# Stop service if running
sudo systemctl stop skills

# Restore files
cp ~/backup/skills.db ~/.local/share/skills/
tar xzf ~/backup/skills.tar.gz -C ~/
cp ~/backup/config.yaml ~/.config/skills/

# Start service
sudo systemctl start skills
```

### Updates

**From source:**
```bash
cargo install --git https://github.com/labiium/skills --force
```

**Pre-built binaries:**
```bash
# Download latest release
curl -L https://github.com/labiium/skills/releases/latest/download/skills-linux-x86_64 -o /usr/local/bin/skills
chmod +x /usr/local/bin/skills
```

**Docker:**
```bash
docker pull ghcr.io/labiium/skills:latest
docker-compose restart
```

---

## Troubleshooting

### Permission Denied

**Symptom:**
```
Error: Failed to create directory: /var/lib/skills
```

**Solution:**
```bash
# Use user-owned directory
export SKILLS_DATA_DIR=$HOME/.local/share/skills
skills server

# Or fix permissions (system-wide)
sudo mkdir -p /var/lib/skills
sudo chown $USER:$USER /var/lib/skills
```

### Database Locked

**Symptom:**
```
Error: database is locked
```

**Solution:**
```bash
# Check for running processes
ps aux | grep skills
kill <pid>  # if needed

# Ensure only one instance
pkill -f skills
skills server
```

### Skills Not Found

**Symptom:**
```
Error: No skills found in directory
```

**Solution:**
```bash
# Check skills directory
skills paths

# Verify directory exists
ls -la $(skills paths | grep "Skills root" | awk '{print $3}')

# Create if missing
mkdir -p ~/.local/share/skills/skills
```

### Upstream Connection Failed

**Symptom:**
```
Error: Failed to connect to upstream: filesystem
```

**Solution:**
```bash
# Test upstream command manually
npx -y @modelcontextprotocol/server-filesystem .

# Check configuration
cat ~/.config/skills/config.yaml

# Enable debug logging
RUST_LOG=debug skills server stdio
```

### Tool Timeout

**Symptom:**
```
Error: Tool execution timed out after 30000ms
```

**Solution:**
```bash
# Increase timeout in config.yaml
sandbox:
  timeout_ms: 120000  # 2 minutes

# Or disable timeout
sandbox:
  backend: none

# Or use CLI mode (no sandbox by default)
skills tool filesystem/read_file '{"path": "./large-file.txt"}'
```

### Network Access Blocked

**Symptom:**
```
Error: Network access denied by sandbox
```

**Solution:**
```yaml
# Enable network in config.yaml
sandbox:
  backend: restricted
  allow_network: true

# Or disable sandbox for specific server
upstreams:
  - alias: api-server
    sandbox:
      backend: none
```

### Out of Memory

**Symptom:**
```
Error: Memory limit exceeded
```

**Solution:**
```yaml
# Increase memory limit
sandbox:
  max_memory_mb: 2048

# Or disable limit
sandbox:
  backend: timeout  # Only timeout, no memory limit
```

### Slow Performance

**Issues:**
- First run is slow (index building)
- Subsequent runs are fast (cached)

**Solution:**
```bash
# Warm up the registry
skills list > /dev/null

# Or rebuild index manually
rm ~/.cache/skills/*
skills list
```

### Docker Issues

**Can't connect to stdio:**
```bash
# Use interactive mode
docker run -it ghcr.io/labiium/skills:latest
```

**Volume permissions:**
```bash
# Use named volumes (recommended)
docker run -v skills-data:/data ghcr.io/labiium/skills:latest

# Or fix host directory permissions
sudo chown -R 1000:1000 ./skills-data
```

---

## Production Deployment Checklist

- [ ] Choose appropriate directory structure (system paths vs custom)
- [ ] Configure upstreams in `config.yaml`
- [ ] Set up sandboxing policies
- [ ] Configure resource limits
- [ ] Enable structured logging
- [ ] Set up log rotation
- [ ] Configure systemd service (or equivalent)
- [ ] Set up monitoring and alerting
- [ ] Configure backup automation
- [ ] Document custom paths and configuration
- [ ] Test disaster recovery procedures
- [ ] Set up CI/CD for skills deployment
- [ ] Review security policies
- [ ] Configure network access controls
- [ ] Set up authentication (if HTTP mode)
- [ ] Test with production workload

---

## Quick Reference

### Environment Variables

- `SKILLS_DATA_DIR` - Override data directory
- `SKILLS_CONFIG_DIR` - Override config directory
- `SKILLS_CACHE_DIR` - Override cache directory
- `SKILLS_DATABASE_PATH` - Override database path
- `SKILLS_ROOT` - Override skills directory
- `SKILLS_LOGS_DIR` - Override logs directory
- `SKILLS_NO_SANDBOX` - Disable sandboxing (`1` or `true`)
- `RUST_LOG` - Set log level (`error`, `warn`, `info`, `debug`, `trace`)

### Command-Line Options

```bash
# Global options
--config <path>         # Config file path
--data-dir <path>       # Data directory
--skills-root <path>    # Skills directory
--database <path>       # Database file
--current-dir           # Use current directory (+ disable sandbox)
--no-sandbox            # Disable sandboxing
--log-level <level>     # Log level

# Server mode
skills server stdio [--no-sandbox]
skills server http --bind <addr:port> [--no-sandbox]

# CLI mode
skills list [server] [-d] [--json]
skills tool <server>/<tool> [args] [--json|--raw]
skills exec <server>/<tool> [args]  # alias for tool
skills grep "<pattern>"
skills paths
```

### Key Files

- `config.yaml` - Main configuration
- `skills.db` - SQLite database (registry + history)
- `skills/` - Skills directory
- `logs/` - Application logs

### Useful Commands

```bash
# Show configuration
skills paths

# List everything
skills list -d

# Search tools
skills grep "*file*"

# Get tool schema
skills tool filesystem/read_file

# Execute tool
skills exec filesystem/read_file '{"path":"./README.md"}'

# Server mode
skills server stdio

# Debug mode
RUST_LOG=debug skills server stdio

# Use custom config
skills --config /etc/skills/config.yaml server stdio
```

---

## Additional Resources

- [README.md](README.md) - Project overview
- [QUICKSTART.md](QUICKSTART.md) - Getting started guide
- [PRODUCTION_CHECKLIST.md](PRODUCTION_CHECKLIST.md) - Production readiness
- [config.example.yaml](config.example.yaml) - Full configuration reference
- [GitHub Issues](https://github.com/labiium/skills/issues) - Bug reports and questions

---

## Support

For issues, questions, or contributions:
- **Repository**: https://github.com/labiium/skills
- **Issues**: https://github.com/labiium/skills/issues
- **License**: MIT OR Apache-2.0