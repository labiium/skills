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
skills skill show <skill-id>             # View skill content (SKILL.md)
skills skill show <skill-id> --file <path>  # View specific file
skills skill create <name> [options]     # Create new skill
skills skill edit <skill-id> [options]   # Update skill
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
# Create from file
skills skill create skill-name \
  --description "What this skill does" \
  --skill-md ./SKILL.md \
  --uses-tools server/tool1,server/tool2

# Create with inline content
skills skill create skill-name \
  --description "Debug Node.js apps" \
  --content "# Debug App\n\n1. Check logs\n2. Find errors" \
  --uses-tools filesystem/read_file,grep

# Create from stdin (useful for agents)
echo "# Skill Name\n\nInstructions..." | skills skill create skill-name --content -

# Or pipe a heredoc
cat << 'EOF' | skills skill create skill-name --content -
# Skill Name

## Purpose
Debug Node.js application issues

## Steps
1. Check logs
2. Find error patterns
3. Trace to source
EOF
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
  skills skill show task-name
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
skills list | grep skill

# Get skill details
skills skill show deploy-to-staging
```

### Load and Execute

```bash
# Load skill instructions
skills skill show deploy-to-staging

# Follow the SKILL.md steps using the tools specified
```

### Before Starting a Task

1. **Check for existing skills**: `skills grep "*<task-keyword>*"`
2. **If skill exists**: Load and follow it
3. **If no skill**: Complete task, then create skill if reusable

---

## Updating Skills

Improve skills when you find better approaches:

### Full Content Replacement
```bash
# Replace from file
skills skill edit deploy-to-staging \
  --version "1.1.0" \
  --skill-md ./updated-SKILL.md

# Replace from inline content
skills skill edit deploy-to-staging \
  --content "# Updated Skill\n\nNew instructions..."

# Replace from stdin
cat ./updated-SKILL.md | skills skill edit deploy-to-staging --content -
```

### Incremental Edits (sed-like)
```bash
# Replace text patterns
skills skill edit deploy-to-staging \
  --replace "old server name" --with "new server name"

# Append to end of SKILL.md
skills skill edit deploy-to-staging \
  --append "## Troubleshooting\n\nCommon issues and solutions..."

# Prepend to beginning
skills skill edit deploy-to-staging \
  --prepend "⚠️ DEPRECATED: Use new-deployment skill instead\n\n"

# Combine operations (applied in order: replace, prepend, append)
skills skill edit deploy-to-staging \
  --replace "step 1" --with "step 1 (updated)" \
  --append "## Notes\n\nAdditional context..."
```

Version semantics:
- **Patch** (1.0.1): Bug fixes, typos, clarifications
- **Minor** (1.1.0): New optional steps, improved error handling
- **Major** (2.0.0): Changed inputs, different tool requirements

---

## Bundled Scripts

For complex automation with bundled scripts, create the skill first, then add scripts to the skill directory:

```bash
# Create the skill
skills skill create analyze-logs \
  --description "Parse and analyze application logs" \
  --skill-md ./SKILL.md

# Add bundled scripts directly to the skill directory
cp analyze.py patterns.json ~/.local/share/skills/skills/analyze-logs/

# Or create them inline
cat > ~/.local/share/skills/skills/analyze-logs/analyze.py << 'EOF'
#!/usr/bin/env python3
import json
import sys
# Script logic here
EOF
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
skills list | grep skill                 # All skills
skills skill show <id>                   # SKILL.md content
skills skill show <id> --file <path>     # Specific file

# Skills - Create
skills skill create <name> \
  --description "..." \
  --skill-md ./SKILL.md                  # From file
skills skill create <name> \
  --content "# Skill\n\n..."              # Inline
skills skill create <name> --content -   # From stdin

# Skills - Edit
skills skill edit <id> \
  --skill-md ./updated.md                # Replace from file
skills skill edit <id> \
  --content "# Updated\n\n..."            # Replace inline
skills skill edit <id> \
  --replace "old" --with "new"           # Sed-like replace
skills skill edit <id> \
  --append "## Notes..."                 # Append to end
skills skill edit <id> \
  --prepend "⚠️ Note..."                 # Prepend to start

# Skills - Delete
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

First, create a SKILL.md file:

```markdown
# Debug Node.js Application

## Purpose
Systematically debug a Node.js application by analyzing logs, identifying error patterns, and tracing to source.

## Inputs
- `log_path` (optional): Path to log file, defaults to ./logs/app.log
- `error_pattern` (optional): Specific error to search for

## Steps
1. **Check recent logs**
   - Tool: `filesystem/read_file` with `{"path": "<log_path>"}`
   - Look for ERROR, WARN, stack traces

2. **Identify error pattern**
   - Search for recurring errors
   - Note timestamps and frequency

3. **Trace to source**
   - Tool: `filesystem/read_file` on files mentioned in stack trace
   - Identify the root cause

4. **Propose fix**
   - Explain the issue
   - Suggest code changes

## Tools Used
- `filesystem/read_file` - Read logs and source files
- `filesystem/list_directory` - Navigate project structure

## Error Handling
- If logs not found: Ask user for correct path
- If error unclear: Suggest enabling debug logging

## Expected Output
Diagnosis of the error with specific file:line reference and proposed fix.
```

Then create the skill:

```bash
skills skill create debug-node-app \
  --description "Debug Node.js application by analyzing logs and tracing errors" \
  --skill-md ./debug-node-app-SKILL.md \
  --uses-tools filesystem/read_file,filesystem/list_directory
```

---

*Build your skill library incrementally. Every reusable workflow saved is future time saved.*
