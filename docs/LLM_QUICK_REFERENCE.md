# Agent quick reference

For a ready-to-use client instruction, use [MCP prompt](PROMPT_MCP.md) or [CLI prompt](PROMPT_CLI.md). The [MCP guide](MCP_GUIDE.md) is the detailed API reference.

| Task | MCP tool input |
|---|---|
| Find skills | `search` with `{"q":"release review","kind":"skills","limit":10}` |
| Enumerate | `search` with `{"q":"*","limit":50}`; pass `next_cursor` as `cursor` for more pages |
| Describe | `exec` with `{"id":"<returned id>","describe":true,"format":"both"}` |
| Validate | `exec` with `{"id":"<returned id>","arguments":{},"dry_run":true}` |
| Activate instructions | `exec` with a returned skill ID and no `script` |
| Read instructions | `manage` with `{"operation":"get","skill_id":"review-notes"}` |
| Read resource | `manage` with `{"operation":"get","skill_id":"review-notes","filename":"references/checklist.txt"}` |
| Profile | `manage` with `{"operation":"profile"}` |
| Upstream status | `manage` with `{"operation":"list_upstreams"}` |

Use IDs returned by discovery, not guessed legacy ID shapes. Describe schemas are loaded on demand; schema metadata is not a separate MCP `schema` tool. Resource reads are `manage.get`, not a separate content tool.

## Execution details

Inspect `execution.status` and `execution.raw_output`; raw output retains upstream `resultType`, `inputRequests`, and `requestState`. Status `input_required` means suspended for input, not complete.

- Missing `arguments` means `{}`. Validate against the actual described schema.
- `describe` supports `format`, `include_output_schema`, `max_bytes`, and `json_pointer`.
- `dry_run` is validation, not an external success or approval grant.
- `script: "scripts/helper.py"` explicitly selects a reviewed bundled helper. Ordinary activation remains instruction-only.
- Top-level `inputResponses` and `requestState` continue an upstream input request; do not put them inside `arguments`. Get real user responses and preserve opaque state.
- Preserve content blocks, structured output, and error status. Do not blindly replay unknown side effects after timeouts.
- Standard MCP caller-provided consent cannot authenticate approval. Local CLI consent is a trusted explicit operator action, still subject to policy.

## Resources and mutations

Non-UTF-8 resource responses use `data.filename`, `data.encoding: "base64"`, and base64 `data.content`. Decode exactly once. CLI `skill show --file` writes bytes directly.

Standard MCP mutations require `policy.management_enabled: true`. Only after operator opt-in, use `manage.create/update/delete` or upstream management operations. Omitted update fields preserve existing values, including unknown frontmatter; empty tool/tag arrays clear them. Read-only overlays remain read-only. Do not erase local source edits or change policy merely to satisfy a request.

## CLI equivalents

```bash
skills search 'release review' --kind skills --json
skills list --descriptions
skills grep '*file*'
skills tool filesystem/read_file
skills tool filesystem/read_file '{"path":"/workspace/reference/notes.txt"}' --json
skills skill show review-notes
skills skill show review-notes --file assets/logo.png > logo.png
skills skill run review-notes
skills skill run review-notes --script scripts/helper.py --args '{}' --consent user_confirmed
skills check
skills update
```

These tool/script names are examples requiring a configured upstream or existing reviewed helper. See [skills](SKILLS.md) for baseline format and ownership, and [operations](OPERATIONS.md#execution-boundaries) for the actual process boundary.
