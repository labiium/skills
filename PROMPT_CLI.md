# Skills.rs CLI Prompt

You have access to MCP tools via the `skills` CLI.

## Commands

```bash
skills list                    # List all servers and tools
skills list <server>           # List tools from one server
skills list -d                 # Include descriptions
skills grep "<pattern>"        # Search tools (glob pattern)
skills tool <server>/<tool>    # Get tool schema
skills tool <server>/<tool> '<json>'  # Execute tool
```

## Workflow

1. **Discover** → `skills list` or `skills grep "<pattern>"`
2. **Inspect** → `skills tool <server>/<tool>` (get schema first!)
3. **Execute** → `skills tool <server>/<tool> '<json>'`

## Examples

```bash
# Find file tools
skills grep "*file*"

# Get schema before calling
skills tool filesystem/read_file

# Execute with JSON args
skills tool filesystem/read_file '{"path": "./README.md"}'

# JSON output for parsing
skills tool filesystem/read_file '{"path": "."}' --json

# Raw text output
skills tool filesystem/read_file '{"path": "."}' --raw
```

## Tool Path Format

`<server>/<tool>` — e.g., `filesystem/read_file`, `brave/search`

## Tips

- Always inspect schema before executing
- Use `--json` when parsing output programmatically
- Use `--raw` for plain text content