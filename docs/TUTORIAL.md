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

## Part 3: Adding MCP Servers (External Tools)

Let's connect to a real tool server. We'll use the official filesystem server, which lets your AI read and write files.

> **What is an MCP Server?** A Model Context Protocol (MCP) server is a program that exposes tools your AI can use — like reading files, searching the web, or accessing databases. In skills.rs, we call these **"upstreams"** because your skills connect to them for capabilities.
>
> **What is an upstream?** Think of "upstreams" as external tool providers you plug into skills.rs. Each upstream is an MCP server that skills.rs connects to and aggregates. Your skills can then request permission to use tools from these upstreams.

**Finding MCP Servers:**
- Browse [awesome-mcp-servers](https://github.com/punkpeye/awesome-mcp-servers) for community servers
- Search npm for `@modelcontextprotocol/server-*` (official servers)
- Many AI tools and services now expose MCP endpoints

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

### Step 1b: Understanding the config

Let's break down what each field means in the upstream config:

```yaml
upstreams:
  - alias: filesystem          # A short name you choose for this server
    transport: stdio           # How skills.rs talks to the server (stdio = standard input/output)
    command: ["npx", ...]      # The command to start the MCP server
```

- **`alias`**: A friendly name you pick. This becomes the prefix for tools (e.g., `filesystem/read_file`).
- **`transport`**: Usually `stdio` for local servers. Can also be `sse` for servers running over HTTP.
- **`command`**: The shell command that starts the MCP server. For Node.js servers, we use `npx` to run them without installing.

### Step 2: Verify it works

List all available tools:

```bash
skills list
```

**Expected output:**

```
filesystem/read_file      Read complete contents of a file
filesystem/write_file     Create a new file or overwrite existing
filesystem/list_directory List files and directories in a folder
filesystem/search_files   Recursively search for files
...
```

If you see tools prefixed with `filesystem/`, congratulations! Your AI now has access to file operations.

> **Troubleshooting:** If you see "connection refused" or no tools appear:
> - Make sure Node.js is installed (`node --version`)
> - Try running the command manually: `npx -y @modelcontextprotocol/server-filesystem .`
> - Check the server starts without errors
> - Run with debug logging: `RUST_LOG=debug skills list`

### Step 3: Try a tool manually

Let's read a file to make sure it works:

```bash
skills tool filesystem/read_file '{"path": "./README.md"}'
```

If you have a README.md file, you'll see its contents printed.

> **Tip:** Want to see what arguments a tool needs? Run `skills tool filesystem/read_file` (without the JSON) to see the schema.

### Connecting MCP Tools to Skills

Here's the important part: **MCP servers provide tools, and skills use those tools.**

When you create a skill, you tell it which upstream tools it's allowed to use via the `allowed-tools` field in the YAML frontmatter of `SKILL.md`.

**Example from Part 6 (Word Counter skill):**

```markdown
---
name: word-counter
description: Count words in a file
version: 1.0.0
allowed-tools: ["filesystem/read_file"]
---

# Word Counter

## Purpose
Count the number of words in a text file.

## Instructions
1. Read the file using `filesystem/read_file`
2. Count the words in the content
3. Return the word count
```

This skill is asking permission to use the `filesystem/read_file` tool from our `filesystem` upstream. The connection chain is:

```
MCP Server (filesystem) → Upstream → Tools (filesystem/read_file) → Skill (word-counter)
```

When you write a skill that needs to read files, you:
1. Add the tool to `allowed-tools` in the YAML frontmatter
2. Reference the tool in `SKILL.md` instructions (e.g., "Read the file using `filesystem/read_file`")

### Common MCP Servers

Here are some popular MCP servers you can add:

**Filesystem** (the one we just set up):
```yaml
upstreams:
  - alias: filesystem
    transport: stdio
    command: ["npx", "-y", "@modelcontextprotocol/server-filesystem", "."]
```

**Brave Search** (web search):
```yaml
upstreams:
  - alias: brave-search
    transport: stdio
    command: ["npx", "-y", "@modelcontextprotocol/server-brave-search"]
    env:
      BRAVE_API_KEY: "your-api-key-here"
```
> Requires a [Brave Search API key](https://brave.com/search/api/)

**GitHub** (repository access):
```yaml
upstreams:
  - alias: github
    transport: stdio
    command: ["npx", "-y", "@modelcontextprotocol/server-github"]
    env:
      GITHUB_PERSONAL_ACCESS_TOKEN: "your-token-here"
```
> Requires a [GitHub Personal Access Token](https://github.com/settings/tokens)

**More servers:** Check the [awesome-mcp-servers](https://github.com/punkpeye/awesome-mcp-servers) repository for databases, APIs, cloud services, and more!

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

### Step 2: Create the SKILL.md

Create `.skills/skills/greeter/SKILL.md`:

```markdown
---
name: greeter
description: Generates fun, personalized greetings
version: 1.0.0
---

# Friendly Greeter

## Purpose
Generates fun, personalized greetings for users.

## Inputs
- name (required): The name of the person to greet

## Instructions
1. Take the user's name from the input
2. Generate a fun, personalized greeting
3. Return the greeting message

## Example Output
"Hello, Alice! Welcome to skills.rs! 🎉"
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
2. Write a `SKILL.md` with YAML frontmatter and instructions
4. Optionally create a Python script for the actual counting

### Here's what the AI might create:

**.skills/skills/word-counter/SKILL.md:**
```markdown
---
name: word-counter
description: Counts words in a text file
version: 1.0.0
allowed-tools: ["filesystem/read_file"]
---

# Word Counter

## Purpose
Count the number of words in a text file.

## Parameters
- file_path: Path to the file to count words in (required)

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