# LLM Quick Reference: skills.rs

**Purpose**: Drop-in system prompt reference for LLMs using skills.rs

---

## The 4 Tools

```
search  → Find tools/skills by keywords
schema  → Get parameter requirements
exec    → Execute with validation
manage  → Create/read/update/delete skills
```

**Always follow**: `search` → `schema` → `exec`

---

## Search Patterns

```json
// Basic search
{ "q": "file reader" }

// Filter by type
{ "q": "csv", "kind": "skills" }

// Fuzzy matching
{ "q": "calclator", "mode": "fuzzy" }

// Regex patterns
{ "q": "csv.*process", "mode": "regex" }

// With filters
{ "q": "file", "filters": { "requires": ["path", "encoding"] } }
```

---

## Schema Inspection

```json
// Get full schema
{ "id": "skill:name@1.0.0", "format": "both" }

// Get only signature
{ "id": "skill:name@1.0.0", "format": "signature" }

// Get JSON schema only
{ "id": "skill:name@1.0.0", "format": "json_schema" }
```

**Key fields to check**:
- `input_schema.properties` - Available parameters
- `input_schema.required` - Required parameters
- `signature.optional` - Optional parameters with defaults

---

## Execution

```json
// Basic execution
{
  "id": "skill:name@1.0.0",
  "arguments": {
    "param1": "value1",
    "param2": "value2"
  }
}

// With timeout
{
  "id": "skill:name@1.0.0",
  "arguments": {...},
  "timeout_ms": 60000
}

// Dry run (validate only)
{
  "id": "skill:name@1.0.0",
  "arguments": {...},
  "dry_run": true
}

// With tracing
{
  "id": "skill:name@1.0.0",
  "arguments": {...},
  "trace": {
    "include_timing": true,
    "include_steps": true
  }
}
```

---

## Skill Management

### Create Skill
```json
{
  "operation": "create",
  "name": "my-skill",
  "version": "1.0.0",
  "description": "What this skill does",
  "skill_md": "# My Skill\n\nInstructions here...",
  "bundled_files": [
    ["script.py", "#!/usr/bin/env python3\nimport json..."]
  ],
  "uses_tools": ["filesystem/read_file"],
  "tags": ["category1", "category2"]
}
```

### Get Skill Info
```json
// Get overview
{ "operation": "get", "skill_id": "my-skill" }

// Get specific file
{ "operation": "get", "skill_id": "my-skill", "filename": "script.py" }
```

### Update Skill
```json
{
  "operation": "update",
  "skill_id": "my-skill",
  "name": "my-skill",
  "description": "Updated description",
  "skill_md": "# Updated...",
  "bundled_files": [
    ["script.py", "#!/usr/bin/env python3..."]
  ]
}
```

### Delete Skill
```json
{ "operation": "delete", "skill_id": "my-skill" }
```

---

## Bundled Script Template (Python)

```python
#!/usr/bin/env python3
"""Bundled tool for [skill-name]"""
import json
import os
import sys

def main():
    # Read arguments
    if 'SKILL_ARGS_JSON' in os.environ:
        args = json.loads(os.environ['SKILL_ARGS_JSON'])
    else:
        with open(os.environ['SKILL_ARGS_FILE']) as f:
            args = json.load(f)
    
    # Extract parameters
    required_param = args.get('required_param')
    optional_param = args.get('optional_param', 'default')
    
    # Validate
    if not required_param:
        print(json.dumps({
            'success': False,
            'error': 'Missing required_param'
        }))
        sys.exit(1)
    
    try:
        # Your logic here
        result = process(required_param, optional_param)
        
        print(json.dumps({
            'success': True,
            'result': result
        }))
        
    except Exception as e:
        print(json.dumps({
            'success': False,
            'error': str(e)
        }))
        sys.exit(1)

if __name__ == '__main__':
    main()
```

---

## Common Errors & Fixes

| Error | Cause | Fix |
|-------|-------|-----|
| `Callable not found` | Wrong ID | Check `search` results for exact ID |
| `Missing required argument` | Schema mismatch | Re-check `schema` output |
| `Timeout exceeded` | Too slow | Increase `timeout_ms` |
| `Permission denied` | Sandbox | Check sandbox_config |
| `Path traversal` | Invalid path | Use relative paths only |

---

## Workflow Patterns

### Pattern 1: Discover and Execute
```
1. search → Find tool
2. schema → Check parameters
3. exec → Run with args
```

### Pattern 2: Create Custom Tool
```
1. search → Check if exists
2. manage(create) → Create skill with bundled script
3. schema → Verify schema
4. exec → Test execution
```

### Pattern 3: Debug Existing Skill
```
1. manage(get) → Read SKILL.md
2. manage(get, filename=script.py) → Read source
3. schema → Check expected parameters
4. exec(dry_run=true) → Validate
5. exec → Execute
```

---

## Input Schema Best Practices

```json
{
  "type": "object",
  "properties": {
    "file_path": {
      "type": "string",
      "description": "Path to input file"
    },
    "mode": {
      "type": "string",
      "enum": ["fast", "thorough"],
      "default": "fast",
      "description": "Processing mode"
    },
    "verbose": {
      "type": "boolean",
      "default": false
    }
  },
  "required": ["file_path"]
}
```

**Guidelines**:
- Always provide descriptions
- Use enums for constrained choices
- Set sensible defaults
- Mark truly required fields only

---

## Security Checklist

- [ ] Validate file paths (no `..`)
- [ ] Escape shell commands
- [ ] Sanitize user input
- [ ] Check file sizes before reading
- [ ] Never log secrets
- [ ] Use timeouts for network ops
- [ ] Handle exceptions gracefully

---

## Example: Complete Session

```
User: "Analyze this log file"

LLM:
1. search → { "q": "log analyzer", "kind": "skills" }
   → Found: log-analyzer@1.0.0

2. schema → { "id": "skill:log-analyzer@1.0.0" }
   → Required: log_file
   → Optional: format, top_n

3. exec → {
     "id": "skill:log-analyzer@1.0.0",
     "arguments": {
       "log_file": "/var/log/app.log",
       "format": "json",
       "top_n": 10
     }
   }
   → Returns analysis results

LLM: "Here are the top 10 errors..."
```

---

## Tips

1. **Progressive Disclosure**: Start with `search`, get details only when needed
2. **Caching**: Remember schema_digest to avoid re-fetching unchanged schemas
3. **Batch Operations**: Group related exec calls when possible
4. **Error Recovery**: Always have fallback strategies
5. **Documentation**: Write clear SKILL.md for custom skills

---

## CLI Equivalents (for reference)

```bash
# Search
skills list
grep

# Schema
skills tool <id>

# Execute
skills tool <id> '<json>'

# Manage
skills skill create <name> --description "..." --skill-md <file>
skills skill show <name>
skills skill show <name> --file <filename>
skills skill edit <name> --replace "old" --with "new"
skills skill delete <name>
```

---

*Version: 1.0 | For skills.rs MCP Server*
