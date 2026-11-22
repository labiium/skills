# Skills: format, storage, and execution

A skill is an instruction document with optional supporting files. Activating one returns instructions for an agent to follow. It is not an automatic program runner or a promise that the agent completed the workflow.

## Portable baseline

```text
review-notes/
  SKILL.md
  scripts/             # optional helpers
  references/          # optional text or other reference files
  assets/              # optional binary or text assets
```

A baseline `SKILL.md` requires `name` and `description`:

```markdown
---
name: review-notes
description: Review release notes against a supplied checklist.
---

# Review notes

Read the supplied release notes and references/checklist.txt. Report missing
items with file locations. Ask before modifying the release notes.
```

This host accepts names of 1–64 ASCII lowercase letters, digits, and single hyphens, without leading/trailing hyphens. Installed directory names must match. Unicode/NFKC naming remains [unresolved upstream](https://github.com/agentskills/agentskills/discussions/487); it is not supported here. Description must contain 1–1024 characters. An optional `compatibility` value must contain 1–500 characters.

`license`, `compatibility`, `metadata`, and `allowed-tools` are optional metadata. This host also recognizes extensions such as `version`, tags, tool usage hints, and executable bundle metadata. They are not portable baseline requirements. Unknown frontmatter fields, including nested mappings, are retained during import and updates. Body-only edits preserve the existing mapping; omitted update fields preserve existing values, while explicit empty tool/tag lists clear them.

Tool hints and `allowed-tools` describe intent. They are not an independent security boundary: the agent follows instructions, and runtime policy authorizes each real call. Host-specific plugins, hooks, and native executable extensions are not executed from frontmatter.

## Managed storage and read-only overlays

`skills init` creates a managed project store at `.skills/skills`. Otherwise the host uses the platform data directory; inspect it with `skills paths`.

The managed store wins when a skill name also exists in an overlay. Automatic read-only roots include user `~/.agents/skills` and `~/.claude/skills`. Project `.agents/skills` and `.claude/skills` roots require `--trust-project-skills`. Trust only after reviewing repository content, including scripts and instructions.

```bash
skills --trust-project-skills list
skills --global list
skills --skill-root /srv/reviewed-skills list
skills --path /srv/isolated-skills paths
```

`--skill-root` is repeatable and adds explicitly selected read-only roots. `--global` excludes project roots. `--path` and `SKILLS_PATH` disable automatic overlays so a selected data store does not silently inherit unrelated skills. An explicit root is a deliberate addition. Managed-store create/update/delete and repository sync do not write into read-only overlays.

Precedence is managed store, then explicit `--skill-root` order, then trusted project roots from the nearest ancestor outward to the Git root, then user roots. Existing configured roots are watched for file changes. Deleting a managed override exposes its remaining overlay in the live registry. Restart after adding a previously nonexistent root.

## Import, pin, check, update

Review a repository before importing its instructions. Listing/importing does not execute downloaded `SKILL.md` instructions or its helpers.

```bash
skills add owner/repo --list
skills add owner/repo --skill review-notes --git-ref "$REVISION"
skills check
skills sync
skills update
```

Set `REVISION` to the reviewed full commit ID. A branch or tag is a moving source selector; a full commit pins the source. Repository selections are recorded in `agent_skills_repos` with `repo`, optional `skills`, and optional `git_ref`.

`add --skill` extends the source's tracked selection without removing other packages. For a tracked source, both `add` and `add --list` use the configured source form: a `file://` Git source stays a Git source even if you refer to it by its local path. Omitting `--git-ref` keeps the configured ref. To narrow the selection or stop tracking a source, edit `agent_skills_repos`, run `check`, then run `sync`.

`check` previews changes without modifying installed packages or their provenance: exit `0` means up to date, `1` means changes are available, and `2` means the check failed. `sync` fetches and applies source changes, restores missing packages, and reconciles removals. `update` is an alias for `sync`. Server startup reads the installed store without fetching or applying source updates.

Imports retain source ownership and the resolved upstream commit. Sync protects locally edited content and conflicts with another source or an unmanaged skill of the same name rather than silently taking ownership. Removed upstream files and source-owned removals must not erase local work. A pinned source does not advance merely because its branch moves.

Provenance is stored in `.agent-skills-sync.json` under the managed root. Source identities are normalized independently of aliases, selected refs, and skill filters; records retain the source/ref, resolved commit, selected skills, and content/path/mode fingerprints. Legacy metadata without fingerprints does not authorize overwriting or deleting a destination; deliberate `add --force` replacement establishes current ownership.

GitHub shorthand, HTTPS/SSH Git URLs, file URLs, and local directories are supported sources. A local directory without a ref tracks its live working tree; a local source with `--git-ref` is resolved through Git. Git uses existing configured credentials; skills.rs does not provide a registry login or separate registry backend.

Source folders are installed under the declared skill name, so repository layout does not need to match the final directory name. `--list` enumerates candidate names without validating or copying every package. Installation validates the selected packages; invalid metadata in an unselected sibling does not block a filtered import. Listing is not proof that every listed package will pass validation.

Local source paths are persisted canonically, so later invocations do not reinterpret them against a different working directory. Configuration-relative paths beginning with `.` resolve against the configuration directory, or the project root for `.skills/config.yaml`. Ambiguous relative legacy provenance cannot authorize deletion or replacement; back up the installation before re-establishing ownership with an explicit source and `--force`.

`add` validates the requested packages and stages the tracking config before installing. A package conflict or validation error stops the import without changing the config. If saving the config fails after installation, `add` rolls back the package and provenance changes. This rollback does not cover process crashes or concurrent writers. Sync can apply successful packages while reporting failures for others, retaining their prior fingerprints.

`skills add owner/repo --skill review-notes --force` is an intentional clean replacement of that managed destination. It removes obsolete files as well as replacing current files. Save local modifications first. Do not use force as an automatic response to ownership conflicts.

## Read and edit

```bash
skills skill show review-notes
skills skill show review-notes --file references/checklist.txt
skills skill create review-notes --skill-md ./SKILL.md
skills skill edit review-notes --description 'Review release notes against the supplied checklist.'
skills skill edit review-notes --skill-md ./SKILL.md
skills skill edit review-notes --append 'Ask before publishing.'
skills skill delete review-notes --force
```

Create/edit/delete examples target the managed store; imported-source edits make that checkout dirty for future sync. The CLI may prompt on deletion unless `--force` is supplied. `--force` here skips confirmation; it does not authorize modifying overlays.

MCP uses `manage` operations `get`, `create`, `update`, and `delete`. Reads remain available under read policy. Mutations require trusted configuration with `policy.management_enabled: true` in Standard mode. Update only the fields that should change; do not reconstruct all metadata from a partial response. See the [MCP guide](MCP_GUIDE.md).

## Instruction-first activation

MCP `exec` on a skill without `script` returns its instructions. The presence of `scripts/` never silently changes activation into code execution. The agent reads the procedure, selects relevant references, and makes any needed tool calls individually under policy.

A workflow saying “read, transform, write” is a procedure for the agent. It is not an embedded autonomous orchestrator. Do not report completion based solely on receiving the instructions.

## Explicit helper execution

Review the helper source before selecting it. Ask the real user for approval, then use the CLI consent option for trusted interactive execution:

```bash
skills skill run review-notes --script scripts/helper.py --args '{"mode":"summary"}' --consent user_confirmed
```

MCP selects the same helper through `exec.script` with a relative path such as `scripts/helper.py`. Self-reported `consent.level` or a caller-provided token does not authenticate approval. Standard MCP requests remain subject to trusted policy; use a host-approved authorization path rather than inventing approval in JSON. Open mode deliberately bypasses these gates inside an externally trusted workspace.

Only supported script types can run, and a selected file must remain inside the skill. Python/Node/shell helpers need their interpreters installed. The JSON argument object is available to native helpers through `SKILL_ARGS_JSON`, with an argument-file path also supplied as `SKILL_ARGS_FILE`; emit useful output on stdout and diagnostics on stderr. For example, a reviewed Python helper can read arguments with:

```python
import json
import os

arguments = json.loads(os.environ.get("SKILL_ARGS_JSON", "{}"))
print(json.dumps({"mode": arguments.get("mode", "summary")}))
```

Use argument arrays rather than interpolating user text into shell commands. Validate expected types and permitted paths, and avoid printing secrets. A process timeout cannot undo writes or external side effects. See [execution boundaries](OPERATIONS.md#execution-boundaries) for enforced backends and prerequisites.

## Text and binary resources

MCP resource retrieval uses:

```json
{"operation":"get","skill_id":"review-notes","filename":"assets/logo.png"}
```

UTF-8 files are returned as text. Non-UTF-8 files use `data` with `filename`, `encoding: "base64"`, and base64 `content`; decode exactly once. Never interpret a lossy text rendering as the original binary. Binary files containing valid UTF-8 bytes may be returned as text; the byte-preserving CLI is useful when saving any asset.

CLI `--file` writes the selected file's bytes directly to stdout:

```bash
skills skill show review-notes --file assets/logo.png > logo.png
```

Resource reads do not execute files. Filenames are skill-relative; absolute paths, traversal outside the skill, and escaping symlinks are not a supported way to access host files. Resource availability is not permission to run its contents.
