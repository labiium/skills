# Skills.rs MCP Server Prompt

You have access to skills.rs, a unified MCP server exposing 7 tools.

## Core Tools

| Tool | Purpose |
|------|---------|
| `search` | Find tools/skills by query |
| `schema` | Get full schema (call before exec!) |
| `exec` | Execute a tool or skill |
| `get_content` | Load skill SKILL.md |

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

**get_content** (for skills only)
```json
{"skill_id": "skill-name", "filename": null}
```

## Skill Management

| Tool | Purpose |
|------|---------|
| `create` | Create skill with SKILL.md |
| `update` | Update existing skill |
| `delete` | Delete a skill |

## Callable ID Format

- Tools: `tool://<server>/<name>@<digest>`
- Skills: `skill://<name>@<version>@<digest>`

Always use IDs from search results — never construct manually.

## Tips

- Always call `schema` before `exec`
- Use `get_content` for skill instructions
- Use `dry_run: true` to validate without executing