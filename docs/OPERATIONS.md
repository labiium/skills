# Operations guide

This guide covers installation, configuration, service deployment, execution boundaries, and recovery. Use [MCP integration](MCP_GUIDE.md) for upstream schemas and [skills](SKILLS.md) for source ownership and resources.

## Installation and release assets

Rust builds require Rust 1.97 or newer. On Debian/Ubuntu, install a C toolchain, pkg-config, and CA certificates before building.

```bash
cargo install --locked skillsrs
# Or build reviewed repository source:
cargo build --locked --release -p skillsrs --bin skills
```

Release assets are archives, not bare binaries. Download the archive matching your platform from [Releases](https://github.com/labiium/skills/releases) together with `SHA256SUMS`; verify its checksum before extracting/installing. Linux assets are `skills-linux-x86_64.tar.gz` and `skills-linux-aarch64.tar.gz`; macOS assets use `skills-macos-x86_64.tar.gz` and `skills-macos-aarch64.tar.gz`; Windows uses `skills-windows-x86_64.zip`. GNU Linux builds require compatible system libraries; use a source build or the container if your system cannot load them. Verify that release checksums came from the release you intended to trust; a checksum is not a separate signature.

```bash
sha256sum --check --ignore-missing SHA256SUMS
# macOS: shasum -a 256 skills-macos-aarch64.tar.gz
# Compare with the matching SHA256SUMS entry, then extract the selected archive.
tar -xzf skills-linux-x86_64.tar.gz
./skills --version
```

## Paths and configuration

```bash
skills paths
skills init
skills --global paths
skills --config /etc/skills/config.yaml --path /var/lib/skills paths
```

`skills paths` is the authority for the current installation. Platform defaults are generally:

| Platform | Data | Configuration |
|---|---|---|
| Linux | `~/.local/share/skills` | `~/.config/skills` |
| macOS | `~/Library/Application Support/skills` | `~/Library/Preferences/skills` |
| Windows | the platform application-data directory for `labiium/skills` | inspect `skills paths` |

Managed data contains `skills/`, `skills.db`, cache/log directories, and source-sync metadata. Merely creating a logs directory does not configure file logging.

Configuration selection uses explicit `--config`, then nearest project `.skills/config.yaml`, then the system/global file. `--global` skips project discovery. `use_global.enabled: true` combines global configuration/upstreams with project configuration; review collisions before enabling it. Use absolute `paths` values in long-running service configurations so their meaning does not depend on a working directory.

Root `--path` overrides the managed data root; `SKILLS_PATH` is its environment counterpart. This derives the skill root, database, cache, and logs under that root and disables automatic read-only overlays. There are no `--data-dir`, `--database`, or `SKILLS_ROOT` overrides. Use `--skill-root` for explicit read-only roots. Project `.agents/skills` and `.claude/skills` discovery requires `--trust-project-skills`; never enable it just because an unreviewed repository requested it.

The [example configuration](config.example.yaml) starts with no upstreams, Standard policy, management disabled, and a bearer-token environment selector for HTTP. Start with this small configuration rather than copying unknown fields from a different MCP client. Unsupported fields do not become enforced policy merely because they appear in YAML.

## HTTP authentication and readiness

Stdio belongs under an MCP client that keeps its input/output streams open:

```bash
skills --config /etc/skills/config.yaml server stdio
```

Use HTTP for a background network service:

```bash
# Set SKILLS_HTTP_TOKEN securely in the service environment first.
skills --config /etc/skills/config.yaml --path /var/lib/skills server http --bind 127.0.0.1:8000
curl --fail --header "Authorization: Bearer $SKILLS_HTTP_TOKEN" http://127.0.0.1:8000/health
```

Configure the variable name through:

```yaml
server:
  auth_token_env: SKILLS_HTTP_TOKEN
```

When configured, an unset or empty token is a startup error. Both `/mcp` and `/health` require a matching bearer header. Non-loopback binds require authentication, including container binds to `0.0.0.0`. An unauthenticated loopback bind is allowed only for the read-safe default policy; management or execution-enabling changes require authentication. Loopback is not a multi-user authorization boundary against other local processes.

`/health` returns HTTP 200 when the component profile is ready and 503 otherwise. Generic Standard readiness checks configured required upstreams without imposing Mesurii aliases. Open additionally requires `codiium` and `dociium`, its unrestricted backend, and an asserted source revision. A healthy process does not prove a particular user workflow succeeded.

Use a trusted TLS reverse proxy for network traffic, restrict inbound access, and keep tokens out of URLs, logs, committed files, and shell history. A bearer token authenticates a client connection; policy still controls what it may do. Rotate the service environment secret and restart the service to rotate credentials.

## Linux service

Provision a dedicated unprivileged `skills` account, `/var/lib/skills` owned by it, and a readable `/etc/skills/config.yaml`. Put `SKILLS_HTTP_TOKEN=...` in a root-owned service environment file with restrictive permissions, using your secret-management process.

```ini
[Unit]
Description=Skills MCP gateway
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=skills
Group=skills
WorkingDirectory=/var/lib/skills
EnvironmentFile=/etc/skills/server.env
Environment=RUST_LOG=info
ExecStart=/usr/local/bin/skills --config /etc/skills/config.yaml --path /var/lib/skills server http --bind 127.0.0.1:8000
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
```

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now skills
sudo systemctl status skills
sudo journalctl -u skills
```

Do not run stdio as a detached system service without an MCP client: its input stream can close. On macOS, an equivalent launchd `ProgramArguments` array must include the executable, root config/path options, `server`, `http`, `--bind`, and the address as separate entries. Set a working directory and supply the token through a protected deployment mechanism rather than committing secrets in a plist. On Windows, use a service manager that preserves those same arguments and an unprivileged account.

The HTTP server handles Ctrl+C and Unix `SIGTERM` for graceful shutdown. Give the service manager a grace period longer than the configured execution deadlines.

## Docker

```bash
docker build --build-arg RUST_VERSION=1.97 -t skills:local .
# Supply SKILLS_HTTP_TOKEN securely in your environment.
docker run -d --name skills \
  -p 127.0.0.1:8000:8000 \
  -e SKILLS_HTTP_TOKEN \
  -v skills-data:/data \
  skills:local
```

The image runs as user `skills` (UID 10001), loads `/etc/skills/config.yaml`, and isolates managed data under `/data`. The default command is `server http --bind 0.0.0.0:8000`; the example config selects `SKILLS_HTTP_TOKEN`. The healthcheck probes the authenticated `/health` endpoint instead of running `--version`.

Mount a custom config read-only at `/etc/skills/config.yaml`. Named volumes preserve managed data. For bind mounts, give UID 10001 access to the data directory without making it world-writable. Python, Node, Bash, git, curl, and Bubblewrap are installed; upstream-specific packages are not. Install reviewed pinned dependencies in a derived image if required. Do not put secrets into the image build arguments.

```yaml
services:
  skills:
    image: skills:local
    restart: unless-stopped
    ports:
      - "127.0.0.1:8000:8000"
    environment:
      SKILLS_HTTP_TOKEN: ${SKILLS_HTTP_TOKEN:?Supply a bearer token}
      RUST_LOG: info
    volumes:
      - skills-data:/data
      - ./config.yaml:/etc/skills/config.yaml:ro
volumes:
  skills-data:
```

Run with `docker compose up -d`. To update a pulled image, use `docker compose pull` followed by `docker compose up -d`; merely restarting a container does not replace its image.

For stdio under a client, override the default command with `server stdio`, use `docker run --rm -i`, and do not allocate a TTY. The image's HTTP healthcheck does not apply to stdio; disable it for that transport with `--no-healthcheck`.

Neither privileged mode nor a host Docker socket is needed for read-only skill serving or remote HTTP upstreams. Bubblewrap execution inside a container still needs usable user namespaces; many container hosts deny them. Do not add privileges or mount the host Docker socket as a generic fix for untrusted helpers. Use a host configured for isolation, or leave code execution disabled. The service should remain useful for instruction/resource reads even where nested isolation is unavailable.

## Execution boundaries

Standard mode enforces policy in CLI and MCP paths; CLI mode is not an automatic unrestricted alternative. Management mutations default off. Trusted upstream aliases, allowed callable patterns, denied tags, and unknown-risk consent gates remain independent checks. MCP caller-provided consent is advisory, not proof of user approval. Use `policy.management_enabled: true` only for a client that should be able to create/update executable skill content and change upstream configuration.

| Backend | Boundary and prerequisites |
|---|---|
| `bubblewrap` | Linux namespace/filesystem/network isolation; requires `bwrap` and kernel namespace permission. Default on Linux. |
| `docker` | Container boundary and configured limits; requires reachable Docker daemon. Default outside Linux; native stdio MCP processes are not supported by this backend. |
| `wasm` | WASI guest capabilities plus a killable Unix worker for compilation/execution, bounded compiler memory/CPU, guest memory/fuel, and deadlines. Requires the installed `skills` executable; unsupported worker platforms fail closed. |
| `timeout`, `restricted` | Unix process environment/resource/deadline hardening only; unsupported platforms fail closed. No filesystem or network containment. Explicit configurations must allow network and omit path restriction lists. |
| `none` | Unrestricted host process access, subject only to the surrounding deployment boundary. |

Enforced backends fail closed when unavailable; they do not silently fall back to `timeout` or `none`. Bubblewrap gives a read-only skill working directory and private writable temporary/home locations by default. Add only the filesystem access a reviewed helper actually needs. Memory/CPU/network controls are backend-specific; do not infer that a configured number guarantees equivalent enforcement across platforms. Remote HTTP effects happen on the upstream host and cannot be restricted by local namespaces.

Native `timeout`/`restricted` cancellation kills the original process group, not hostile descendants that deliberately detach into another session. Use these modes only for trusted native programs; Bubblewrap PID namespaces or Docker are the boundary for hostile subprocesses. A timeout cannot undo filesystem writes or remote effects.

`sandbox.max_output_bytes` bounds combined raw stdout/stderr capture (default 8 MiB). Crossing the limit is an error and terminates the helper/container instead of returning truncated success. Docker sandbox logging is disabled; output is attached before startup and captured through the bounded stream. Capture bounds also protect trusted/Open sessions.

WASM compilation never runs inside the serving process. A hidden worker invocation compiles and executes the module under process limits and is terminated on timeout/cancellation. Guest stdio stays private. Library consumers outside the `skills` executable must set `SKILLS_WASM_WORKER` to an absolute trusted `skills` executable. Zero CPU/fuel configuration does not remove the worker's 60-second safety ceiling; compilation has a separate bounded memory allowance.


`--no-sandbox`, `SKILLS_NO_SANDBOX`, and `--current-dir` intentionally remove process containment. Open policy (`SKILLS_POLICY=open` or `policy.mode: open`) also deliberately removes runtime gates and enables management. Use it only inside an externally managed, trusted workspace. Open requires `MESURII_SKILLS_SOURCE_REVISION` with an exact 40-hex revision asserted by the operator. That assertion is not checkout attestation or a signature, and does not establish that a binary was built from that revision.

Do not execute imported instructions while assessing or installing skills. Metadata such as `allowed-tools` is descriptive and does not constrain an agent that has other host tools. Review scripts, dependencies, source ownership, and the actual security boundary before granting execution.

## Monitoring, backup, and recovery

Use `RUST_LOG` or root `--log-level` for diagnostics. Capture stderr through the service manager/container logs. There is no documented `logging.file` YAML setting; redirect through your supervisor for file logging and rotation. Logs and execution history can contain arguments/results, so restrict access and retention.

Before backing up, stop writers or use SQLite's online backup facility rather than copying a live database without its WAL. Back up the managed skill directory, database, configuration, and `.agent-skills-sync.json` provenance together. Keep a separate inventory/backup of read-only overlay sources. Store secrets separately under the appropriate secret-management policy.

```bash
sudo systemctl stop skills
# Example for the explicit /var/lib/skills service layout above:
sudo tar -C /var/lib -czf /secure-backups/skills-data.tar.gz skills
sudo cp /etc/skills/config.yaml /secure-backups/skills-config.yaml
sudo systemctl start skills
```

Restore into the same configured layout while the service is stopped, restore ownership for the dedicated service account, restore secrets separately, then validate and start the service. Do not restore unreviewed archived code directly into an execution-enabled deployment. Verify readiness and a representative read after recovery.

Run `skills sync`/`skills update` explicitly to populate or refresh a store from configured sources. Server startup does not do this. Use `skills check` to preview first: `0` means up to date, `1` means changes are available, and `2` means the check failed. Save local modifications before intentional `add --force` replacement. Upgrade the program independently through a reviewed release archive, locked source build, or replacement image.

## Troubleshooting

| Symptom | Check before changing security policy |
|---|---|
| Missing skill | `skills paths`, managed directory, valid `SKILL.md`, collision precedence, project trust flag, and `--path` isolation. |
| Permission denied | Data/config ownership for the actual service UID and read-only overlay/mount status. Do not use root as a workaround. |
| Database locked | Find competing writers and use a coordinated shutdown/backup. Do not kill unrelated processes or delete the database. |
| Upstream disconnected | Reviewed executable/dependencies, endpoint, credential presence, startup stderr, and sandbox prerequisites. |
| Policy denied | Operator-configured management/trust/risk/allow rules. A self-reported consent token cannot fix authorization. |
| Timeout | Whether the operation had side effects before failure; adjust a bounded timeout only after reviewing the cause. Never blindly replay writes. |
| Namespace failure | Host support for Bubblewrap/user namespaces; do not silently switch to unrestricted execution. |
| Docker unhealthy | Token presence, configured variable name, service logs, `/health` profile gates, and required upstream status. |
| Resource looks corrupted | Use CLI `--file` bytes or decode the MCP base64 envelope exactly once. |
| Sync refuses replacement | Local edits, changed source ownership, or legacy provenance lacking fingerprints. Back up before explicit replacement. |

Before exposing a deployment, confirm authentication, TLS/network boundary, required upstreams, unprivileged ownership, executable isolation prerequisites, logs/retention, backups, and a recovery procedure appropriate to that environment. A passing health probe alone is not a production security assessment.
