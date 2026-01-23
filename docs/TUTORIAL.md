# Getting Started with skills.rs — A Beginner's Tutorial

Welcome! This tutorial will walk you through using **skills.rs** to give your AI assistant (like Claude, GPT, or any LLM) access to real tools and skills. No prior experience with command-line tools is required — we'll explain everything step by step.

---

## What You'll Learn

1. What MCP servers and skills are
2. How to set up skills.rs in your project
3. How to connect your LLM to external tools
4. How to ask your LLM to create a new tool for you

By the end, you'll have a working setup where your AI can discover, use, and even create tools.

---

## Part 1: Understanding the Basics

### What is an MCP server?

An **MCP server** (Model Context Protocol server) is a program that exposes "tools" that an AI can call. Think of it like giving your AI hands to interact with the real world:

- Read and write files
- Search the web
- Query databases
- Control external APIs

Without MCP servers, your AI can only talk. With them, it can *do* things.

### What is a skill?

A **skill** is a recipe for your AI. It's a set of instructions (written in plain language) plus optional helper scripts. Skills tell the AI *how* to accomplish complex tasks using the available tools.

For example, a "Web Researcher" skill might instruct the AI to:
1. Search the web for a topic
2. Read the top results
3. Summarize the findings
4. Save the summary to a file

### What is skills.rs?

**skills.rs** is a unified gateway that:
- Connects to multiple MCP servers
- Manages your skills
- Provides a simple CLI for your AI to discover and use everything

---

## Part 2: Installation

### Step 1: Install skills.rs

You'll need Rust installed. If you don't have it, visit [rustup.rs](https://rustup.rs/) and follow the instructions.

Then install skills.rs:

```bash
cargo install --git https://github.com/labiium/skills
```

Verify it worked:

```bash
skills --version
```

You should see a version number printed.

### Step 2: Initialize your project

Navigate to your project folder (or create a new one):

```bash
mkdir my-ai-project
cd my-ai-project
```

Now initialize skills.rs:

```bash
skills init
```

This creates a `.skills/` folder containing:
- `config.yaml` — your configuration file
- `skills/` — where your custom skills live
- `skills.db` — a local database for tracking everything

---

## Part 3: Adding Your First MCP Server

Let's connect to a real tool server. We'll use the official filesystem server, which lets your AI read and write files.

### Step 1: Edit your config

Open `.skills/config.yaml` in any text editor and add an upstream server:

```yaml
paths:
  data_dir: ".skills"
  skills_root: ".skills/skills"
  database_path: ".skills/skills.db"

sandbox:
  backend: timeout
  timeout_ms: 30000
  allow_read: []
  allow_write: []
  allow_network: false
  max_memory_bytes: 536870912
  max_cpu_seconds: 30

use_global:
  enabled: false

upstreams:
  - alias: filesystem
    transport: stdio
    command: ["npx", "-y", "@modelcontextprotocol/server-filesystem", "."]
```

> **Note:** You'll need Node.js installed for this to work. The `npx` command downloads and runs the filesystem server automatically.

### Step 2: Verify it works

List all available tools:

```bash
skills list
```

You should see something like:

```
filesystem/read_file
filesystem/write_file
filesystem/list_directory
...
```

Congratulations! Your AI now has access to file operations.

### Step 3: Try a tool manually

Let's read a file to make sure it works:

```bash
skills tool filesystem/read_file '{"path": "./README.md"}'
```

If you have a README.md file, you'll see its contents printed.

---

## Part 4: Connecting Your LLM

Now let's tell your AI how to use these tools. Add this to your AI's system prompt (the instructions you give it at the start of a conversation):

```
You have access to MCP tools via the `skills` CLI.

Commands:
- `skills list` — List all available tools
- `skills tool <server>/<tool>` — Get a tool's schema (what arguments it needs)
- `skills tool <server>/<tool> '<json>'` — Execute a tool with arguments

Workflow:
1. Discover: Run `skills list` to see available tools
2. Inspect: Run `skills tool <name>` to see what arguments a tool needs
3. Execute: Run `skills tool <name> '<json>'` with the required arguments

Example:
- To read a file: `skills tool filesystem/read_file '{"path": "./example.txt"}'`
```

Now when you chat with your AI, it can discover and use tools on its own!

---

## Part 5: Your First Skill

Let's create a simple skill that helps your AI greet users in a fun way.

### Step 1: Create the skill folder

```bash
mkdir -p .skills/skills/greeter
```

### Step 2: Create the manifest

Create `.skills/skills/greeter/skill.json`:

```json
{
  "id": "greeter",
  "title": "Friendly Greeter",
  "version": "1.0.0",
  "description": "Generates fun, personalized greetings",
  "inputs": {
    "type": "object",
    "properties": {
      "name": {
        "type": "string",
        "description": "The name of the person to greet"
      }
    },
    "required": ["name"]
  },
  "entrypoint": "prompted",
  "tool_policy": {
    "allow": [],
    "deny": [],
    "required": []
  },
  "hints": {
    "domain": ["greeting", "fun"],
    "expected_calls": 0
  },
  "risk_tier": "read_only"
}
```

### Step 3: Create the instructions

Create `.skills/skills/greeter/SKILL.md`:

```markdown
# Friendly Greeter

## Purpose
Generate a fun, personalized greeting for someone.

## Instructions
1. Take the person's name from the input
2. Create a warm, friendly greeting
3. Add a fun fact or joke to make them smile
4. Keep it short and sweet (2-3 sentences max)

## Examples
- Input: {"name": "Alice"}
- Output: "Hey Alice! 🎉 Did you know octopuses have three hearts? Hope your day is three times as awesome!"

## Notes
- Be creative and vary your greetings
- Use emojis sparingly but effectively
- Keep it appropriate for all ages
```

### Step 4: Verify the skill loaded

Restart skills.rs and search for your skill:

```bash
skills grep "*greeter*"
```

You should see your skill in the results!

---

## Part 6: Getting Your LLM to Create a Tool

Here's where it gets really fun. You can ask your AI to create new tools for you!

### Example conversation:

**You:** I need a tool that can count the words in a text file. Can you create one for me?

**AI:** I'll create a word counter skill for you. Let me set that up...

*The AI would then:*
1. Create a new folder in `.skills/skills/word-counter/`
2. Write a `skill.json` manifest
3. Write a `SKILL.md` with instructions
4. Optionally create a Python script for the actual counting

### Here's what the AI might create:

**`.skills/skills/word-counter/skill.json`:**
```json
{
  "id": "word-counter",
  "title": "Word Counter",
  "version": "1.0.0",
  "description": "Counts words in a text file",
  "inputs": {
    "type": "object",
    "properties": {
      "file_path": {
        "type": "string",
        "description": "Path to the file to count words in"
      }
    },
    "required": ["file_path"]
  },
  "entrypoint": "prompted",
  "tool_policy": {
    "allow": ["filesystem/read_file"],
    "deny": [],
    "required": ["filesystem/read_file"]
  },
  "hints": {
    "domain": ["text", "analysis"],
    "expected_calls": 1
  },
  "risk_tier": "read_only"
}
```

**`.skills/skills/word-counter/SKILL.md`:**
```markdown
# Word Counter

## Purpose
Count the number of words in a text file.

## Instructions
1. Read the file using `filesystem/read_file`
2. Split the content by whitespace
3. Count the resulting words
4. Report the total word count

## Tools Used
- `filesystem/read_file` — Read the target file

## Expected Output
A message like: "The file contains 1,234 words."
```

---

## Part 7: Tips for Success

### Keep your AI informed

Always include the skills.rs commands in your system prompt. The AI can't use tools it doesn't know about!

### Start simple

Begin with basic tools like filesystem access. Once comfortable, add more complex MCP servers.

### Check the logs

If something isn't working, run with debug logging:

```bash
RUST_LOG=debug skills list
```

### Disable sandboxing for development

If you're having permission issues during development, you can temporarily disable sandboxing in `.skills/config.yaml`:

```yaml
sandbox:
  backend: none
```

> **Warning:** Only do this in development! Always use sandboxing in production.

---

## Part 8: What's Next?

Now that you have the basics down:

1. **Add more MCP servers** — Try web search, GitHub, databases, etc.
2. **Create complex skills** — Combine multiple tools into powerful workflows
3. **Share your skills** — Skills are just folders; you can share them with others!

### Useful resources:

- [README.md](../README.md) — Full feature overview
- [QUICKSTART.md](QUICKSTART.md) — Condensed setup guide
- [OPERATIONS.md](OPERATIONS.md) — Detailed configuration reference
- [config.example.yaml](config.example.yaml) — All configuration options

---

## Quick Reference

| Command | What it does |
|---------|--------------|
| `skills init` | Set up skills.rs in current directory |
| `skills list` | Show all available tools |
| `skills list -d` | Show tools with descriptions |
| `skills grep "<pattern>"` | Search for tools by name |
| `skills tool <name>` | Get a tool's schema |
| `skills tool <name> '<json>'` | Execute a tool |
| `skills paths` | Show where files are stored |
| `skills --global list` | Use global config instead of project |

---

## Need Help?

- **Issues:** [GitHub Issues](https://github.com/labiium/skills/issues)
- **Discussions:** [GitHub Discussions](https://github.com/labiium/skills/discussions)

Happy building! 🚀