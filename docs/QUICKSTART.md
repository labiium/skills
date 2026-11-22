# Quick start

This walkthrough creates a local instruction-only skill. It does not download skill instructions, connect to an upstream, or execute a helper.

## Install and initialize

Use Rust 1.97 or newer with the system build dependencies described in [operations](OPERATIONS.md).

```bash
cargo install --locked skillsrs
mkdir skill-demo
cd skill-demo
skills init
skills paths
```

`init` creates `.skills/config.yaml` and the managed `.skills/skills/` root. Commands in this project discover that configuration. Inspect the generated paths rather than guessing platform-global directories.

## Write a skill

```bash
mkdir -p .skills/skills/review-notes/references
cat > .skills/skills/review-notes/SKILL.md <<'EOF'
---
name: review-notes
description: Review supplied release notes against a local checklist.
---

# Review notes

Read the user's release notes and references/checklist.txt. Report missing
items with their file locations. Ask before modifying any files.
EOF
printf 'Check version, migration notes, and breaking changes.\n' > .skills/skills/review-notes/references/checklist.txt
skills validate
skills search review-notes --kind skills --json
skills skill show review-notes
skills skill show review-notes --file references/checklist.txt
```

This is a valid baseline skill without version or tool metadata. Reading it is not proof that an agent performed the review.

## Connect an MCP client

Configure your client to run the absolute installed binary path if `skills` is not on its PATH, and use an absolute configuration path:

```json
{
  "mcpServers": {
    "skills": {
      "command": "skills",
      "args": ["--config", "/absolute/path/skill-demo/.skills/config.yaml", "server", "stdio"]
    }
  }
}
```

In the connected client, call MCP `search`:

```json
{"q":"review-notes","kind":"skills","limit":10}
```

Use the returned ID with `exec` and `describe: true` to inspect it. Load the instructions with `manage`:

```json
{"operation":"get","skill_id":"review-notes"}
```

A normal `exec` on a skill also activates instructions; scripts run only with explicit `script` selection. Standard mutating MCP management is disabled by default. Do not enable it simply to read instructions.

## Choose the next step

- Add reviewed upstream tools using the [MCP guide](MCP_GUIDE.md).
- Import a reviewed repository with `skills add owner/repo --list`, then `skills add owner/repo --skill name --git-ref "$REVISION"`, where `REVISION` is the reviewed full commit ID.
- Read [skill sources and scripts](SKILLS.md) before enabling code execution or project overlays.
- Use [operations](OPERATIONS.md) for authenticated HTTP and Docker deployment.

No helper is needed for this example. If you add one later, inspect its source and use explicit `skills skill run ... --script ... --consent user_confirmed` only after real user approval and with a usable enforced execution backend.
