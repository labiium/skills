# Skills.rs MCP Server

You have access to skills.rs, a unified MCP server that aggregates tools from multiple upstream servers and local skills into a single searchable registry.

## Core Concept: Two Types of Callables

| Type | What It Is | How It Executes |
|------|-----------|-----------------|
| **Tool** | Atomic operation from an upstream MCP server | Proxied to the original server |
| **Skill** | Reusable workflow with instructions | You read SKILL.md and orchestrate tool calls |

**Key insight:** Skills are instructions for you to follow, not automated pipelines. When you "execute" a skill, you're loading its instructions and then calling the tools it references.

---

## The Three-Step Workflow

Every interaction follows this pattern:

```
search  →  Find what you need
schema  →  Understand the parameters  
exec    →  Run it
```

### Why this matters

1. **Search returns minimal data** to save tokens (just names and descriptions)
2. **Schema gives you full parameter details** only when needed
3. **Exec validates and runs** with policy enforcement

**Never skip schema.** Even for familiar tools, schema confirms the tool exists and returns the exact ID for exec.

---

## Working with Tools

### Finding and executing a tool

```
1. search(q: "read_file")
   → Returns matches with IDs

2. schema(id: <id from search>)
   → Returns input parameters and types

3. exec(id: <id>, arguments: {path: "./file.txt"})
   → Returns the tool's output
```

### Search strategies

| Goal | Approach |
|------|----------|
| Exact tool name | `search(q: "read_file")` |
| Broader discovery | `search(q: "file", mode: "fuzzy")` |
| Only tools | `search(q: "...", kind: "tools")` |
| From specific server | `search(q: "...", filters: {server: "filesystem"})` |
| List everything | `search(q: "", limit: 50)` |

### When search returns nothing

1. Broaden your terms ("file" instead of "read_file")
2. Try fuzzy mode
3. Remove filters
4. List all available tools to see what exists

---

## Working with Skills

Skills are different from tools. A skill is a **documented procedure** that tells you which tools to use and how to use them.

### The skill execution pattern

```
1. search(q: "deploy", kind: "skills")
   → Find relevant skills

2. manage(operation: "get", skill_id: "deploy-staging")
   → Load the SKILL.md instructions

3. Read the instructions, then for each tool mentioned:
   - search for the tool
   - schema to get parameters
   - exec to run it

4. Continue following the skill's steps until complete
```

### What manage get returns

- The full SKILL.md with step-by-step instructions
- List of tools the skill uses
- Any bundled scripts or files
- Metadata about the skill

### Why skills exist

Skills solve the problem of repeatable multi-step workflows:
- Encode domain knowledge and best practices
- Reduce errors by documenting exact steps
- Enable knowledge sharing across sessions
- Build a library of your capabilities

---

## Creating Skills

Create a skill when you complete a useful multi-step workflow that could be repeated.

### When to create

- You finish a multi-step task the user might want again
- User says "remember how to do this"
- You develop a reusable pattern combining multiple tools

### When NOT to create

- One-off tasks specific to this context
- Single-tool operations
- Trivial 1-2 step workflows

### Skill creation flow

After completing a reusable task:

```
manage(
  operation: "create",
  name: "task-name",           // kebab-case, action-oriented
  description: "Brief summary",
  skill_md: "...",             // Step-by-step instructions
  uses_tools: ["server/tool"], // Tools you used
  tags: ["category"]
)
```

### Writing effective SKILL.md

Write instructions that another agent (or future you) can follow:

```markdown
# Skill Name

## Purpose
What this accomplishes in one sentence.

## Inputs
- `param` (required): What it is

## Steps
1. **Step name**
   - Tool: `server/tool_name`
   - Arguments: describe what to pass
   - On success: what to do next
   - On failure: how to recover

2. **Next step**
   ...

## Expected Output
What the user should see when complete.
```

**Principles:**
- Each step names the exact tool
- Include error handling inline
- Be specific enough to execute without guessing

---

## Updating Skills

Improve skills when you find better approaches:

```
1. manage(operation: "get", skill_id: "existing-skill")  // Review current version
2. manage(
     operation: "update",
     skill_id: "existing-skill",
     version: "1.1.0",    // Bump the version
     skill_md: "...",     // Improved instructions
     ...
   )
```

**Version semantics:**
- Patch (1.0.1): Typos, clarifications
- Minor (1.1.0): New optional steps, better error handling
- Major (2.0.0): Changed inputs, different tools required

---

## Progressive Disclosure

The system is designed to minimize token usage through three levels:

| Level | What You Get | Size |
|-------|-------------|------|
| Search | Names, descriptions, IDs | ~200 bytes each |
| Schema | Full parameter details | ~2-5 KB |
| Content | Complete SKILL.md | ~5-50 KB |

**This matters because:** Loading 100 skills' full content would be 2MB. With progressive disclosure, you load only what you need.

**Workflow implication:** Always search first, then selectively load schema/content for the specific items you'll use.

---

## Error Recovery

### "Tool not found"
- Broaden search terms
- Try `mode: "fuzzy"`
- Check for typos
- List all: `search(q: "", limit: 50)`

### "Invalid arguments"
- You skipped schema—call it first
- Check required vs optional in schema response
- Verify argument types match

### "Skill not found"
- Use the skill name, not the full ID
- Verify it exists: `search(q: "name", kind: "skills")`

### Execution timeout
- Increase `timeout_ms`
- Check if operation is blocking on input

---

## ID Format

IDs are returned by search and used in schema/exec calls:

- **Tools:** `tool:srv:<server>::<name>::sd:<digest>`
- **Skills:** `skill:<name>@<version>`

**Never construct IDs manually.** They include digests that change. Always get fresh IDs from search results.

---

## Decision Guide

```
Need to use a specific tool?
  → search → schema → exec

Starting a multi-step task?
  → First: search(kind: "skills") for existing workflow
  → If found: manage(operation: "get"), follow the steps
  → If not: complete task, consider creating skill

Just finished a useful multi-step task?
  → Would this be useful again?
  → If yes: manage(operation: "create") to save it

Found a better approach for an existing skill?
  → manage(operation: "update") the skill with improvements
```

---

## Bundled Scripts

Some skills include executable scripts (Python, Bash, Node.js). When you exec these skills, the script runs directly and returns output. You don't need to follow SKILL.md steps—the script does the work.

Check for bundled tools in the manage get response. If present, you can exec the skill directly with arguments.

---

## Best Practices

**Discovery:**
- Search before assuming a tool exists
- Use fuzzy mode for exploratory searches
- Check for existing skills before building workflows manually

**Execution:**
- Always call schema before exec
- Use dry_run for destructive operations
- Pass timeout_ms for slow operations

**Skill creation:**
- Use kebab-case names with action verbs: `deploy-staging`, `analyze-logs`
- Keep skills focused—split large workflows into composable pieces
- Document error handling in SKILL.md steps
- List all tools used in uses_tools for discoverability

**Maintenance:**
- Update skills when you find improvements
- Delete obsolete skills
- Build complex workflows by composing simpler skills
