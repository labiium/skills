# MCP integration guide

Skills.rs is both an MCP server for clients and a client of configured upstream servers. These are different connections: the outer server exposes `search`, `exec`, and `manage`; upstream tools remain discoverable through that facade rather than becoming extra top-level MCP tools.

## Connect a client

For a local MCP client, use stdio:

```json
{
  "mcpServers": {
    "skills": {
      "command": "skills",
      "args": ["--config", "/absolute/path/.skills/config.yaml", "server", "stdio"]
    }
  }
}
```

Use absolute configuration paths because a client's working directory may differ from your shell. `skills server` also defaults to stdio. Do not allocate a TTY or send log output into the protocol stream.

For HTTP:

```bash
skills --config /etc/skills/config.yaml --path /var/lib/skills server http --bind 127.0.0.1:8000
```

The streamable HTTP MCP endpoint is `/mcp`. `/health` returns readiness status, including profile and upstream status; unready is HTTP 503. When `server.auth_token_env` is configured, clients must send `Authorization: Bearer <token>` to both endpoints. Non-loopback binding requires this authentication; unauthenticated loopback is only allowed with the read-safe default policy. See [operations](OPERATIONS.md) for service and TLS deployment.

## Configure upstreams

Configuration is YAML with `upstreams`, not a client-style `mcpServers` object. A stdio upstream uses an argument array, not a shell command string:

```yaml
upstreams:
  - alias: local
    transport: stdio
    command: ["/usr/local/bin/reviewed-mcp-server", "--root", "/workspace/reference"]
    tags: [local, reference]
    required: true
```

Install and review the server and its dependencies separately. Skills.rs does not install arbitrary upstream packages for you. A Node-based upstream needs Node and any package manager it uses. Use pinned installed packages rather than having `npx -y` download a moving version on every launch. Only start an upstream manually for troubleshooting after reviewing its executable and arguments.

Global sandbox settings apply to native stdio process startup. `sandbox_config` on an upstream overrides those settings. Bubblewrap requires Linux namespace support; the Docker backend cannot currently launch native stdio MCP processes. Explicit `timeout`/`restricted` harden the process without filesystem/network isolation and require `allow_network: true` with no path restriction lists. No backend change is a substitute for upstream trust.

Remote HTTP example (replace the endpoint with your real server):

```yaml
upstreams:
  - alias: internal
    transport: http
    url: "https://mcp.example.com/mcp"
    auth:
      type: bearer
      env: INTERNAL_MCP_TOKEN
    tags: [internal]
    required: true
```

Supply the secret through `INTERNAL_MCP_TOKEN` in the process environment, not a committed YAML token. Upstream bearer authentication and this server's `server.auth_token_env` protect different connections. The `auth` schema supports bearer credentials; arbitrary custom-header examples are not part of this configuration contract.

Upstream HTTP and stdio both require MCP `2026-07-28` discovery via `server/discover`. They do not fall back to earlier `initialize`-based revisions. A local sandbox does not contain a remote HTTP service or revoke its credentials.

Discovery alone does not authorize execution. Standard policy requires explicitly trusted server aliases, risk authorization, and any configured allow/deny rules. For example, add `internal` to `policy.trusted_servers` only after reviewing that server. `required: true` makes a disconnected upstream a readiness failure; optional upstream failure does not by itself make generic standalone operation unready. Open mode additionally requires Mesurii's `codiium` and `dociium` dependencies.

## Search and describe

MCP tool `search`:

```json
{"q":"release review","kind":"skills","limit":10}
```

`kind` is `any`, `tools`, or `skills`. Use `q: "*"` for deterministic enumeration. When a response contains `next_cursor`, pass it as `cursor` with the same query/filter options to get the next page. Do not assume the first page is the entire registry. Do not invent a `mode: "fuzzy"` field; use the advertised input schema.

Use the ID returned by search as the `id` of MCP `exec`. To inspect a callable:

```json
{"id":"<id returned by search>","describe":true,"format":"both"}
```

`format` accepts `json_schema`, `signature`, or `both`. `include_output_schema`, `max_bytes`, and `json_pointer` control description detail. Treat returned schemas and schema digests as the contract for that version, not a guarantee that a later version accepts the same arguments. A description does not execute the callable.

## Execute and continue

MCP `exec` validates arguments, applies policy, and routes the call:

```json
{"id":"<id returned by search>","arguments":{"path":"/workspace/reference/notes.txt"},"dry_run":true}
```

A successful dry run is not proof that an external side effect succeeded, nor authorization to perform it. Remove `dry_run` only when execution is intended and authorized. Missing `arguments` normalizes to an empty object.

An ordinary skill call activates its instructions. Helpers run only when `script` explicitly selects a relative bundled file:

```json
{"id":"<skill id returned by search>","script":"scripts/helper.py","arguments":{"mode":"summary"}}
```

This example expresses script selection, not an authorization grant. Standard MCP does not trust caller-supplied `consent.level`/tokens as proof of user approval. Use trusted host configuration or an approved interactive CLI path; never invent consent to overcome a denial. Open is an explicit unrestricted deployment profile, not a workaround to enable for an untrusted request.

Upstream calls can require further user input. Preserve the upstream's `inputRequests` and opaque `requestState`, obtain the requested responses from the real user, then resume the **same upstream callable** with `inputResponses` and `requestState` at the top level of `exec`, rather than burying them inside tool `arguments`. Preserve response IDs and state unchanged. Continuation fields are not accepted for ordinary skill activation or scripts. A continuation is not a fresh unrelated tool call and must not silently choose answers for the user.

Execution responses expose `status` (`input_required` for a suspended call), preserve MCP `content`, optional `structured_content`, and `is_error`, and retain the full upstream result in `execution.raw_output`. Read `resultType`, `inputRequests`, and `requestState` from that raw result when continuing. Inspect errors and all content blocks, including non-text blocks. Do not flatten an image, resource link, or structured result into a fabricated text success. Do not blindly retry writes after timeouts or transport failures: the remote side may already have committed its work. Inspect descriptor replay metadata and the upstream's guarantees before retrying.

## Manage skills and resources

Read instructions:

```json
{"operation":"get","skill_id":"review-notes"}
```

Read one resource:

```json
{"operation":"get","skill_id":"review-notes","filename":"references/checklist.txt"}
```

Use `filename`, not `file` or an obsolete content tool. UTF-8 resource text is returned in `message`. Non-UTF-8 resource bytes are in `data` with `filename`, `encoding: "base64"`, and base64 `content`. Decode once; see [resources](SKILLS.md#text-and-binary-resources).

`manage` also supports `profile` and `list_upstreams` for read-only status inspection. Create/update/delete and upstream mutation operations require management authorization. Standard mode defaults `policy.management_enabled` to false. Enabling it trusts the connected client to modify executable content/configuration; bearer authentication is not a substitute for choosing an appropriate policy.

With explicit management authorization, a minimal creation request is:

```json
{
  "operation":"create",
  "name":"review-notes",
  "description":"Review release notes against the supplied checklist.",
  "skill_md":"# Review notes\n\nRead the supplied notes. Report missing checklist items."
}
```

Use `scripts`, `references`, or `assets` as arrays of `[relative_filename, text_content]` pairs to add UTF-8 files under those respective directories. These creation inputs are text, not an implicit base64 upload protocol.

A description-only update preserves the body, resources, and unrelated frontmatter:

```json
{"operation":"update","skill_id":"review-notes","description":"Review release notes before publication."}
```

Omit fields that should remain unchanged; empty tool/tag arrays explicitly clear them. A delete request uses `operation: "delete"` and `skill_id`. Overlay skills remain read-only regardless of management opt-in. See [skill ownership](SKILLS.md) before modifying imported skills.

## CLI counterpart and troubleshooting

```bash
skills list --descriptions
skills list internal
skills grep '*review*'
skills tool internal/read_file
skills tool internal/read_file '{"path":"/workspace/reference/notes.txt"}' --json
skills add-mcp http internal https://mcp.example.com/mcp
skills edit-mcp internal --description 'Reviewed internal reference service'
skills add-mcp stdio local -- /usr/local/bin/reviewed-mcp-server --root '/workspace/review notes'
skills edit-mcp local -- /usr/local/bin/reviewed-mcp-server --root '/workspace/new notes'
```

`add-mcp` tests discovery before saving. If the upstream is intentionally offline or needs credentials added to YAML, pass `--no-test` to save without connecting. For stdio, pass the executable and each argument separately after `--`; put CLI flags before it.

`search --kind skills` and `skill run` do not connect to upstreams. Mixed discovery warns and skips disconnected optional upstreams; commands targeting a named upstream report its connection error. CLI registration persists to the config file. MCP-managed runtime changes do not necessarily persist across restarts.

For connection failures, check the executable path, dependencies, bearer environment variable presence without printing its value, endpoint URL, and enforced backend availability. Use `RUST_LOG=debug skills --config /absolute/path/config.yaml list` for diagnostics, protecting logs as potentially sensitive. If an upstream reports no tools, check its discovery error rather than inventing schemas. For policy denials, fix trusted configuration through the operator; do not self-assert consent or switch to Open automatically.
