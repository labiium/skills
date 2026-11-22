# Tutorial: build a review workflow

Skills are written procedures. MCP servers provide callable tools. Skills.rs puts both in a registry so an agent can find a procedure, read it, and use appropriate tools without loading every schema into its initial context.

This tutorial builds on the local `review-notes` skill in the [quick start](QUICKSTART.md). Complete that walkthrough first; it needs no external service or code execution.

## Find and read before acting

```bash
skills search review-notes --kind skills --json
skills skill show review-notes
skills skill show review-notes --file references/checklist.txt
```

The first command finds metadata. The second loads instructions. The third reads a supporting file. None of them reviews the user's release notes automatically, and none runs a script.

An agent using MCP performs the same separation:

1. Call `search` with a descriptive query and `kind: "skills"`.
2. Use the returned callable ID in `exec` with `describe: true` if details are needed.
3. Read the procedure through `manage` with `operation: "get"`, or activate the skill with ordinary `exec`.
4. Follow the procedure within the user's request. Read relevant resources and inspect any tools before calling them.
5. Report actual results, distinguishing completed actions from suggested changes.

Use the [short MCP prompt](PROMPT_MCP.md) as client guidance. The JSON examples there are MCP tool inputs, not shell commands.

## Add an external tool when needed

A skill saying “read a file” does not create a file-reading tool. Your agent may already have one. Otherwise, install and review an MCP server that provides it, then configure an upstream.

For an installed local server:

```yaml
upstreams:
  - alias: reference
    transport: stdio
    command: ["/usr/local/bin/reviewed-mcp-server", "--root", "/workspace/reference"]
    tags: [reference]
```

This executable path is a placeholder for the reviewed server you installed. Do not copy it and expect a built-in executable. A `command` array passes arguments separately without a shell. On Linux, the default Bubblewrap backend must be available and the server must have the access it needs through reviewed `sandbox_config` settings. On other platforms, the default Docker backend does not support native stdio MCP processes; use an HTTP upstream or an explicitly reviewed supported native backend. See [execution boundaries](OPERATIONS.md#execution-boundaries).

Discover and inspect the actual tools:

```bash
skills list reference --descriptions
skills grep '*read*'
# Replace read_file with a name returned by discovery:
skills tool reference/read_file
```

Inspect the real schema before deciding argument names. Do not assume every upstream uses `path`, returns plain text, or can be retried safely. Standard policy requires trusted server configuration and risk authorization before execution. Add an alias to `policy.trusted_servers` only after deciding to trust it; discovery and read-only description are not authorization grants.

Remote HTTP upstreams avoid launching a local process but move the execution boundary to the remote host. Review the endpoint and its credentials. [MCP integration](MCP_GUIDE.md) explains bearer authentication and continuation requests.

## Improve the procedure without losing metadata

Edit the original `SKILL.md` to make instructions specific: what input the user provides, what to read, which checks to perform, what output to return, and where to stop for approval. Keep optional nested `metadata` and other host-specific fields when revising the body.

The local CLI supports explicit trusted operator edits without enabling MCP management:

```bash
skills skill edit review-notes --description 'Review release notes before publication.'
skills skill edit review-notes --append 'Ask before publishing or changing files.'
```

MCP `manage.update` similarly preserves omitted fields. Do not rebuild a skill from only its description or drop unknown frontmatter. Management operations cannot modify a read-only overlay; make a deliberate managed copy instead. Editing an imported managed skill makes it locally modified for synchronization purposes.

## Reuse a reviewed source

```bash
skills add owner/repo --list
skills add owner/repo --skill review-notes --git-ref "$REVISION"
skills check
skills update
```

Set `REVISION` to a reviewed full commit ID; substitute the real repository and skill names. Listing or importing is not permission to execute downloaded instructions. `check` previews source changes: `0` means up to date, `1` means changes are available, and `2` means the check failed. `update` applies clean source-owned updates. If local edits or another source conflict, resolve ownership deliberately rather than forcing away local work. See [pinning and ownership](SKILLS.md#import-pin-check-update).

For repository-local native skill directories, review the repository first and opt in with `--trust-project-skills`. Use `--global` to exclude project roots or `--path` for an isolated managed store. User roots and explicit read-only roots have their own precedence described in the [skill guide](SKILLS.md#managed-storage-and-read-only-overlays).

## Add a helper only if instructions are insufficient

A script is optional code, not a required part of a skill. Read and review it before execution. Selecting `scripts/helper.py` explicitly and obtaining real approval are separate requirements:

```bash
skills skill run review-notes --script scripts/helper.py --args '{"mode":"summary"}' --consent user_confirmed
```

This command only applies if that reviewed script exists and an enforced backend supports it. Ordinary activation remains instruction-only even after scripts are added. The helper receives JSON through `SKILL_ARGS_JSON` and an argument-file path through `SKILL_ARGS_FILE`; it should validate input, avoid shell interpolation and secrets in output, and report failures honestly. A timeout cannot roll back external effects.

For binary assets use `skills skill show review-notes --file assets/logo.png > logo.png`; do not send raw bytes through a lossy text conversion. MCP returns non-UTF-8 resource bytes in a base64 envelope.

## Operate the service

Use stdio under an MCP client. Use authenticated HTTP for a background service, and check `/health` rather than `--version` to establish readiness. Before enabling remote access or code execution, follow the [operations guide](OPERATIONS.md) for token configuration, TLS, isolation prerequisites, unprivileged ownership, backups, and troubleshooting.
