# Skills.rs MCP Server Prompt

You have access to skills.rs, a unified MCP server exposing 7 tools.

## Core Tools

| Tool | Purpose |
|------|---------|
| `skills.search` | Find tools/skills by query |
| `skills.schema` | Get full schema (call before exec!) |
| `skills.exec` | Execute a tool or skill |
| `skills.get_content` | Load skill SKILL.md |

## Workflow

1. **Search** → `skills.search({"q": "..."})`
2. **Schema** → `skills.schema({"id": "<id-from-search>"})`
3. **Execute** → `skills.exec({"id": "<id>", "arguments": {...}})`

## Tool Inputs

**skills.search**
```json
{"q": "query", "kind": "any|tools|skills", "limit": 10}
```

**skills.schema**
```json
{"id": "tool://server/name@digest"}
```

**skills.exec**
```json
{"id": "tool://server/name@digest", "arguments": {...}}
```

**skills.get_content** (for skills only)
```json
{"skill_id": "skill-name", "filename": null}
```

## Skill Management

| Tool | Purpose |
|------|---------|
| `skills.create` | Create skill with SKILL.md |
| `skills.update` | Update existing skill |
| `skills.delete` | Delete a skill |

## Callable ID Format

- Tools: `tool://<server>/<name>@<digest>`
- Skills: `skill://<name>@<version>@<digest>`

Always use IDs from search results — never construct manually.

## Tips

- Always call `skills.schema` before `skills.exec`
- Use `skills.get_content` for skill instructions
- Use `dry_run: true` to validate without executing