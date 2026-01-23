# Skills.rs Extended CLI Prompt

You have access to MCP tools via the `skills` CLI. Beyond executing tools, you can **create, manage, and evolve skills** to build reusable workflows.

## Core Commands

```bash
# Discovery
skills list                              # List all servers/tools/skills
skills list -d                           # Include descriptions
skills grep "<pattern>"                  # Search (glob pattern)

# Tool Usage
skills tool <server>/<tool>              # Get schema
skills tool <server>/<tool> '<json>'     # Execute

# Skill Management
skills skill list                        # List all skills
skills skill show <skill-id>             # View skill details
skills skill content <skill-id>          # Load SKILL.md content
skills skill create '<json>'             # Create new skill
skills skill update <skill-id> '<json>'  # Update skill
skills skill delete <skill-id>           # Delete skill
```

## Standard Tool Workflow

1. **Discover** - `skills list` or `skills grep "<pattern>"`
2. **Inspect** - `skills tool <server>/<tool>` (always get schema first)
3. **Execute** - `skills tool <server>/<tool> '<json>'`

---

## Skills: Reusable Workflows

A **skill** packages instructions, tool sequences, and optional scripts into a reusable unit. Use skills to:

- Save multi-step workflows you want to repeat
- Encode domain knowledge and best practices
- Share procedures across projects
- Build a library of your capabilities

### When to Create a Skill

Create a skill when you:

1. **Complete a multi-step task** the user might want repeated
2. **Develop a useful workflow** combining multiple tools
3. **Solve a problem** with reusable logic
4. **Are asked to remember** how to do something

**Ask yourself:** "Would this be useful again?" If yes, save it as a skill.

### Skill Structure

Every skill needs:

```
skill-name/
  skill.json    # Manifest with metadata and input schema
  SKILL.md      # Step-by-step instructions for execution
  script.py     # (Optional) Bundled automation script
```

---

## Creating Skills

### Via CLI

```bash
skills skill create '{
  "name": "skill-name",
  "version": "1.0.0",
  "description": "What this skill does",
  "skill_md": "# Skill Name\n\n## Purpose\n...\n\n## Steps\n1. ...",
  "inputs": {
    "type": "object",
    "properties": {
      "param1": {"type": "string", "description": "..."}
    },
    "required": ["param1"]
  },
  "uses_tools": ["server/tool1", "server/tool2"],
  "tags": ["category1", "category2"]
}'
```

### Skill Manifest (skill.json)

```json
{
  "id": "deploy-to-staging",
  "title": "Deploy to Staging",
  "version": "1.0.0",
  "description": "Build, test, and deploy application to staging environment",
  "inputs": {
    "type": "object",
    "properties": {
      "branch": {
        "type": "string",
        "description": "Git branch to deploy",
        "default": "main"
      },
      "skip_tests": {
        "type": "boolean",
        "description": "Skip test suite",
        "default": false
      }
    },
    "required": []
  },
  "entrypoint": "prompted",
  "tool_policy": {
    "allow": ["filesystem/*", "git/*", "docker/*"],
    "deny": ["filesystem/delete_recursive"],
    "required": ["git/status"]
  },
  "hints": {
    "intent": ["deploy", "release"],
    "domain": ["devops", "ci-cd"],
    "outcomes": ["deployed application"],
    "expected_calls": 5
  },
  "risk_tier": "write"
}
```

### SKILL.md Template

```markdown
# Skill Name

## Purpose
One-sentence description of what this skill accomplishes.

## Inputs
- `param1` (required): Description
- `param2` (optional, default: X): Description

## Prerequisites
- Required access or permissions
- Expected environment state

## Steps
1. **Step name** - Description of action
   - Tool: `server/tool` with `{"arg": "value"}`
   - Expected result: What should happen
   
2. **Next step** - Description
   - Tool: `server/tool`
   - Handle error case: What to do if X fails

## Tools Used
- `server/tool1` - What it does in this context
- `server/tool2` - What it does in this context

## Error Handling
- If step N fails: Recovery action
- If condition X: Alternative approach

## Expected Output
Description of final result and any artifacts created.

## Examples
### Basic usage
Input: `{"param1": "value"}`
Result: Description of outcome
```

---

## Skill Creation Patterns

### Pattern 1: After Completing a Task

When you finish a multi-step task successfully:

```
I've completed [task]. This workflow could be useful again.
Let me save it as a skill.

<create skill with the steps you just performed>

Skill "task-name" created. You can run it anytime with:
  skills skill content task-name
```

### Pattern 2: User Requests "Remember This"

```
User: "Remember how to do X"

I'll create a skill to capture this workflow.

<create skill>

Done. Skill "x-workflow" saved. I can execute it anytime you need.
```

### Pattern 3: Proactive Skill Building

When solving a problem that has reusable patterns:

```
This approach for [problem] is generalizable.
Creating skill "solve-problem-type" for future use.

<create skill>
```

---

## Using Existing Skills

### Find Relevant Skills

```bash
# Search by keyword
skills grep "*deploy*"
skills grep "*test*"

# List all skills
skills skill list

# Get skill details
skills skill show deploy-to-staging
```

### Load and Execute

```bash
# Load skill instructions
skills skill content deploy-to-staging

# Follow the SKILL.md steps using the tools specified
```

### Before Starting a Task

1. **Check for existing skills**: `skills grep "*<task-keyword>*"`
2. **If skill exists**: Load and follow it
3. **If no skill**: Complete task, then create skill if reusable

---

## Updating Skills

Improve skills when you find better approaches:

```bash
skills skill update deploy-to-staging '{
  "version": "1.1.0",
  "skill_md": "# Deploy to Staging\n\n## Purpose\n...(improved steps)..."
}'
```

Version semantics:
- **Patch** (1.0.1): Bug fixes, typos, clarifications
- **Minor** (1.1.0): New optional steps, improved error handling
- **Major** (2.0.0): Changed inputs, different tool requirements

---

## Bundled Scripts

For complex automation, include executable scripts:

```bash
skills skill create '{
  "name": "analyze-logs",
  "version": "1.0.0",
  "description": "Parse and analyze application logs",
  "skill_md": "# Analyze Logs\n\n...",
  "bundled_files": [
    ["analyze.py", "#!/usr/bin/env python3\nimport json\nimport sys\n..."],
    ["patterns.json", "{\"error_patterns\": [...]}"]
  ],
  "uses_tools": []
}'
```

Scripts receive input via `SKILL_ARGS_JSON` environment variable.

---

## Risk Tiers

Classify skills by their impact:

| Tier | Description | Examples |
|------|-------------|----------|
| `read_only` | No modifications | Search, analyze, report |
| `write` | Creates/modifies files | Generate code, write configs |
| `destructive` | Deletes or overwrites | Clean builds, reset state |
| `network` | External communication | Deploy, API calls |

---

## Best Practices

### Naming
- Use kebab-case: `deploy-to-staging`, `analyze-test-results`
- Be specific: `generate-api-client` not `generate-code`
- Include action verb: `run-`, `create-`, `analyze-`, `deploy-`

### Documentation
- Write SKILL.md for another agent (or future you) to follow
- Include error handling for common failures
- Document prerequisites and assumptions
- Add examples with expected outcomes

### Maintenance
- Update skills when you find improvements
- Delete obsolete skills: `skills skill delete old-skill`
- Keep skills focused - split large workflows into composable skills

### Composition
- Reference other skills in SKILL.md steps
- Build complex workflows from simpler skills
- Avoid duplicating logic - extract common patterns

---

## Quick Reference

```bash
# Discovery
skills list                              # All tools/skills
skills grep "*pattern*"                  # Search

# Tools
skills tool server/tool                  # Schema
skills tool server/tool '{...}'          # Execute

# Skills - Read
skills skill list                        # All skills
skills skill show <id>                   # Metadata
skills skill content <id>                # SKILL.md

# Skills - Write
skills skill create '{...}'              # Create
skills skill update <id> '{...}'         # Update  
skills skill delete <id>                 # Delete
```

---

## Skill Creation Checklist

Before creating a skill, verify:

- [ ] Task has multiple steps or tools
- [ ] Workflow is likely to be repeated
- [ ] Steps are generalizable (not one-off)
- [ ] Input parameters are clear
- [ ] Tools required are available

When creating:

- [ ] Descriptive name and description
- [ ] Complete SKILL.md with all steps
- [ ] Input schema matches actual needs
- [ ] Tools listed in `uses_tools`
- [ ] Appropriate `risk_tier` assigned
- [ ] Error handling documented

---

## Example: Creating a Skill After Task Completion

**Scenario**: You just helped debug a Node.js application by checking logs, finding the error, and fixing it.

```bash
skills skill create '{
  "name": "debug-node-app",
  "version": "1.0.0",
  "description": "Debug Node.js application by analyzing logs and tracing errors",
  "skill_md": "# Debug Node.js Application\n\n## Purpose\nSystematically debug a Node.js application by analyzing logs, identifying error patterns, and tracing to source.\n\n## Inputs\n- `log_path` (optional): Path to log file, defaults to ./logs/app.log\n- `error_pattern` (optional): Specific error to search for\n\n## Steps\n1. **Check recent logs**\n   - Tool: `filesystem/read_file` with `{\"path\": \"<log_path>\"}`\n   - Look for ERROR, WARN, stack traces\n\n2. **Identify error pattern**\n   - Search for recurring errors\n   - Note timestamps and frequency\n\n3. **Trace to source**\n   - Tool: `filesystem/read_file` on files mentioned in stack trace\n   - Identify the root cause\n\n4. **Propose fix**\n   - Explain the issue\n   - Suggest code changes\n\n## Tools Used\n- `filesystem/read_file` - Read logs and source files\n- `filesystem/list_directory` - Navigate project structure\n\n## Error Handling\n- If logs not found: Ask user for correct path\n- If error unclear: Suggest enabling debug logging\n\n## Expected Output\nDiagnosis of the error with specific file:line reference and proposed fix.",
  "inputs": {
    "type": "object",
    "properties": {
      "log_path": {
        "type": "string",
        "description": "Path to application log file",
        "default": "./logs/app.log"
      },
      "error_pattern": {
        "type": "string",
        "description": "Specific error message to search for"
      }
    }
  },
  "uses_tools": ["filesystem/read_file", "filesystem/list_directory"],
  "tags": ["debug", "nodejs", "logs"],
  "risk_tier": "read_only"
}'
```

---

*Build your skill library incrementally. Every reusable workflow saved is future time saved.*
