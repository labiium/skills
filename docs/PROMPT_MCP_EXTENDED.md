# Extended MCP client guidance

Use the [short MCP prompt](PROMPT_MCP.md) as the base. This supplement covers workflow decisions rather than defining a second API.

## Decide what to load

Search for the task, not only a guessed tool name. Search returns metadata, not all skill bodies or schemas. Describe only relevant callables. Cache descriptions only while their IDs/schema digests remain valid, and refetch after a schema change.

If no suitable match exists, broaden the query or enumerate visible results with `q: "*"`, following pagination. A policy-hidden or disconnected upstream may explain missing tools. Do not invent a callable, add an unreviewed server, or create code merely to avoid a failed search.

## Follow an instruction skill

A skill is a procedure for you to follow, not an autonomous tool sequence. Read the body, then load only references needed for the current task. Map its tool hints to actual discovered callable schemas. Each real tool call is separately authorized by the runtime and the user's request.

Keep task state and report what actually happened. Receiving an instruction document is not execution evidence. If the procedure calls for writing, publishing, deleting, credentials, or new code execution beyond the user's authorization, stop for real approval rather than inferring it from the skill's prose.

## Explicit scripts

A bundled script does not run during ordinary activation. Review its file and dependencies before setting `exec.script`. Use a skill-relative supported file path and pass JSON in `arguments`. Standard MCP consent fields cannot authenticate user consent; respect the configured authorization boundary. Open mode is intentionally unrestricted and is an operator deployment decision, not an agent-selected fix.

For native script authoring, use `SKILL_ARGS_JSON` for the JSON argument object or the file named by `SKILL_ARGS_FILE`, explicit argument arrays for child processes, bounded input/resource handling, and stdout/stderr separation. Validate user paths and types. Do not substitute shell escaping for a filesystem security boundary.

## Continuation and replay

If an upstream returns `inputRequests` with `requestState`, ask the real user for the requested information. Preserve state and request IDs, and resume the same callable using top-level `inputResponses` and `requestState`. Do not merge protocol continuation fields into the tool's input object or fabricate a user's response.

MCP `exec` exposes the full upstream result in `execution.raw_output`; read its `resultType`, `inputRequests`, and `requestState`. A suspended call has execution `status: "input_required"` rather than completed success.

Inspect all MCP content blocks and structured data, plus error status. Network failure and timeout can leave the result of a write unknown. Do not replay a destructive call unless the descriptor/upstream establishes a safe replay contract or the user authorizes the recovery action.

## Management and durable learning

Prefer an existing skill over duplicates. Propose a new reusable procedure only when it serves the user's task and the client is authorized to create it. Standard MCP mutating management defaults off; read-only `get`, `profile`, and `list_upstreams` do not require enabling mutations.

When authorized, give a skill a specific baseline name/description and a clear instruction body. Store helpers under `scripts/`, references under `references/`, and assets under `assets/`. Optional/unknown frontmatter belongs to the skill and must survive unrelated edits. Omitted update fields preserve values; empty tool/tag arrays clear them. Do not update a read-only overlay or use force to erase source ownership conflicts.

Use the [MCP guide](MCP_GUIDE.md) for exact request fields, [skill guide](SKILLS.md) for file and source semantics, and [practical examples](LLM_EXAMPLES.md) for valid request sequences.
