# Skills.rs

Skills.rs combines local Agent Skills and upstream MCP tools in a searchable registry. It ships the `skills` CLI and an MCP server exposing three tools: `search`, `exec`, and `manage`.

Search returns compact metadata; descriptions, schemas, instructions, and resources are loaded on demand. Context savings depend on the registry and workload, not a fixed percentage.

[CI](https://github.com/labiium/skills/actions/workflows/ci.yml) · [Releases](https://github.com/labiium/skills/releases) · [Operations](docs/OPERATIONS.md)

## Install

Building requires Rust **1.97 or newer**, a C toolchain, and pkg-config on Linux.

```bash
cargo install --locked skillsrs
# Or install the single package from this repository:
cargo install --locked --git https://github.com/labiium/skills skillsrs
```

The package and library are named `skillsrs`; the executable is `skills`.

```bash
skills init
skills paths
skills validate
skills server stdio
```

`init` creates `.skills/config.yaml` and a managed `.skills/skills/` store. It exits nonzero if the project config already exists unless you pass `--force`. Commands discover the nearest project configuration by walking upward. See the [quick start](docs/QUICKSTART.md) for a local, network-free first skill.

## CLI

Put root options before the subcommand:

```bash
skills --global list
skills --config /etc/skills/config.yaml --path /var/lib/skills paths
skills list --descriptions
skills grep '*file*'
skills tool filesystem/read_file
skills tool filesystem/read_file '{"path":"/workspace/README.md"}' --json
skills skill show review-notes
skills skill show review-notes --file references/checklist.txt
```

Tool paths in these examples require a configured upstream named `filesystem`; inspect its actual schema before supplying arguments. `skills exec` is an alias for tool execution, not an MCP JSON request.

To import reviewed skills:

```bash
skills add owner/repo --list
skills add owner/repo --skill review-notes --git-ref "$REVISION"
skills check
skills update
```

Set `REVISION` to the reviewed full commit hash for a reproducible import. Repeated `add` commands keep previously selected skills. `check` leaves the managed store unchanged and exits `0` if up to date, `1` if changes are available, or `2` on error. `update` aliases `sync`; starting the server does not sync sources. `add --force` deliberately replaces the selected destination, including obsolete files; back up local work first. See [skill sources and ownership](docs/SKILLS.md).

## MCP

Configure the client to launch:

```json
{
  "mcpServers": {
    "skills": {
      "command": "skills",
      "args": ["--config", "/absolute/path/to/.skills/config.yaml", "server", "stdio"]
    }
  }
}
```

The three MCP tools have distinct jobs:

| Tool | Purpose |
|---|---|
| `search` | Find skills or upstream tools; `q: "*"` enumerates the visible registry with pagination. |
| `exec` | Describe, validate, activate skill instructions, or execute an explicitly selected script/upstream tool. |
| `manage` | Read skill content/resources and profile status; mutations require explicit management opt-in. |

Use returned callable IDs verbatim rather than constructing IDs. A normal workflow is search, describe, then read instructions or execute an authorized tool. See the [MCP guide](docs/MCP_GUIDE.md), [examples](docs/LLM_EXAMPLES.md), and [client prompt](docs/PROMPT_MCP.md).

## Skill storage and format

A portable skill has `SKILL.md` with `name` and `description` frontmatter and a Markdown instruction body. `scripts/`, `references/`, and `assets/` are optional. Version, tags, tool hints, host-specific metadata, and similar extensions are not requirements for a baseline skill. Optional and unknown frontmatter fields survive metadata and body edits.

The managed store takes precedence over read-only overlays. User `.agents/skills` and `.claude/skills` roots are discovered automatically. Project `.agents/skills` and `.claude/skills` roots require `--trust-project-skills`; review the repository first. `--global` excludes project roots. `--path` or `SKILLS_PATH` isolates automatic discovery to the selected managed store; use repeatable `--skill-root` for explicit additional read-only roots. Skills from overlays cannot be edited or deleted through managed-store operations.

Reading or activating a skill does **not** automatically run its scripts. Script execution requires selecting a relative path explicitly:

```bash
skills skill run review-notes --script scripts/helper.py --args '{"mode":"summary"}' --consent user_confirmed
```

Only use this after reviewing the script and obtaining real user approval. MCP uses `exec.script`; caller-provided MCP consent is advisory and cannot grant authorization. Binary resources are returned losslessly: CLI `skill show --file` writes bytes, and MCP `manage.get` uses a base64 JSON envelope for non-UTF-8 content. Native host plugins, hooks, and executable extensions are not loaded from skill metadata. Details are in the [skill guide](docs/SKILLS.md).

## Security boundary

Standard mode disables mutating MCP management by default. Opt in with `policy.management_enabled: true` only for a trusted client; upstream execution also requires configured server trust and risk authorization. Treat skill instructions as untrusted input, not authority to change policy or obtain consent.

Native code execution defaults to an enforced backend: Bubblewrap on Linux, Docker on other platforms. Missing isolation support is an error, not a fallback to unrestricted execution. Bubblewrap requires `bwrap` and usable kernel namespaces. Docker requires a reachable daemon; native stdio MCP processes are not supported by the Docker backend. Explicit `timeout` and `restricted` backends provide process/deadline/resource hardening, **not filesystem or network containment**. Incompatible restrictions are rejected. A local wrapper cannot restrict the machine running a remote HTTP upstream.

`--no-sandbox`, `--current-dir`, and `sandbox.backend: none` remove process isolation. The explicit Open policy (`SKILLS_POLICY=open` or `policy.mode: open`) is intentionally unrestricted and enables management. Its trust boundary is an externally controlled workspace. Open requires `MESURII_SKILLS_SOURCE_REVISION` to contain a 40-hex source revision asserted by the operator; this is not cryptographic proof of the running checkout. Mesurii's `codiium`/`dociium` readiness requirements apply to Open, not to generic standalone use.

## HTTP and Docker

```bash
skills --config /etc/skills/config.yaml --path /var/lib/skills server http --bind 127.0.0.1:8000
```

The MCP endpoint is `/mcp`; readiness is `/health`. A non-loopback bind requires `server.auth_token_env` naming an environment variable containing the bearer token. When configured, authentication protects both endpoints. Unauthenticated loopback operation is limited to the read-safe default policy. Use TLS at a trusted reverse proxy for traffic outside the local host.

The image runs as UID 10001, uses `/etc/skills/config.yaml` and `/data`, and requires `SKILLS_HTTP_TOKEN` for its default HTTP command:

```bash
docker build -t skills:local .
# Supply SKILLS_HTTP_TOKEN securely in the host environment first.
docker run --rm -p 127.0.0.1:8000:8000 -e SKILLS_HTTP_TOKEN -v skills-data:/data skills:local
curl --fail -H "Authorization: Bearer $SKILLS_HTTP_TOKEN" http://127.0.0.1:8000/health
```

No privileged container or host Docker socket is required for serving instructions and HTTP upstreams. Running isolated native helpers inside a container needs host namespace support; do not weaken container security merely to make untrusted scripts run. See [deployment, backup, and troubleshooting](docs/OPERATIONS.md).

## Development

```bash
cargo build --locked --release -p skillsrs
cargo test --locked -p skillsrs --all-features
cargo clippy --locked -p skillsrs --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
python3 scripts/audit.py
```

The audit script requires Python 3.11 or newer. It denies all RustSec warnings except [RUSTSEC-2026-0543](https://rustsec.org/advisories/RUSTSEC-2026-0543), and only while RMCP is the reason Chrono is present.

CI tests stable Rust across Linux, macOS, and Windows, plus the declared MSRV on Linux. Release automation validates the tag/package version, tests and packages the single crate, blocks on audit failures, builds native target binaries, and publishes archives with SHA-256 checksums. The benchmark workflow generates local deterministic skill fixtures and reports actual search/read/validation timings, including CLI startup; it does not make comparative performance claims.

## Guides

- [Quick start](docs/QUICKSTART.md) and [tutorial](docs/TUTORIAL.md)
- [Skill format, sources, scripts, and resources](docs/SKILLS.md)
- [MCP configuration, continuation, and management](docs/MCP_GUIDE.md)
- [Operations and security](docs/OPERATIONS.md)
- [Configuration example](docs/config.example.yaml)
- [CLI prompt](docs/PROMPT_CLI.md) and [MCP prompt](docs/PROMPT_MCP.md)

License: MIT OR Apache-2.0; see [LICENSE](LICENSE).
