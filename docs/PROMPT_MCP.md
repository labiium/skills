# Skills.rs MCP Server Prompt

You have access to skills.rs, a unified MCP server exposing 4 tools.

## Core Tools

| Tool | Purpose |
|------|---------|
| `search` | Find tools/skills by query |
| `schema` | Get full schema (call before exec!) |
| `exec` | Execute a tool or skill |
| `manage` | Skill lifecycle management (create, get, update, delete) |

## Workflow

1. **Search** → `search({"q": "..."})`
2. **Schema** → `schema({"id": "<id-from-search>"})`
3. **Execute** → `exec({"id": "<id>", "arguments": {...}})`

## Tool Inputs

**search**
```json
{"q": "query", "kind": "any|tools|skills", "limit": 10}
```

**schema**
```json
{"id": "tool://server/name@digest"}
```

**exec**
```json
{"id": "tool://server/name@digest", "arguments": {...}}
```

**manage** (Skill lifecycle)

Create a skill:
```json
{
  "operation": "create",
  "name": "skill-name",
  "version": "1.0.0",
  "description": "What this skill does",
  "skill_md": "# Skill Name\n\nStep-by-step instructions...",
  "uses_tools": ["server/tool"],
  "bundled_files": [["script.py", "print('hello')"]]
}
```

Get skill content:
```json
{
  "operation": "get",
  "skill_id": "skill-name",
  "filename": null
}
```

Update a skill:
```json
{
  "operation": "update",
  "skill_id": "skill-name",
  "name": "skill-name",
  "version": "1.1.0",
  "description": "Updated description",
  "skill_md": "# Updated content..."
}
```

Delete a skill:
```json
{
  "operation": "delete",
  "skill_id": "skill-name"
}
```

## Callable ID Format

- Tools: `tool://<server>/<name>@<digest>`
- Skills: `skill://<name>@<version>@<digest>`

Always use IDs from search results — never construct manually.

## Tips

- Always call `schema` before `exec`
- Use `manage` with `operation: "get"` to load skill instructions
- Use `dry_run: true` to validate without executing
- Use `manage` tool for all skill lifecycle operations
