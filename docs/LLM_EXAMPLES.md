# Practical agent examples

Each JSON block below is an input to the named MCP tool, not a shell command. IDs in angle brackets must come from actual discovery. Responses are deliberately not fabricated; inspect the real server result before advancing.

## Review a document with an instruction skill

Call `search`:

```json
{"q":"release notes review","kind":"skills","limit":10}
```

If a suitable skill appears, call `exec` to inspect it:

```json
{"id":"<skill id returned by search>","describe":true,"format":"both"}
```

Read its procedure through `manage` (substitute the actual discovered name):

```json
{"operation":"get","skill_id":"review-notes"}
```

If the procedure references a checklist, load it selectively:

```json
{"operation":"get","skill_id":"review-notes","filename":"references/checklist.txt"}
```

The agent now performs the requested review with the user's actual document. The read/activation result is not evidence that the review or any write occurred. If needed, discover the real document-reading upstream tool separately, describe it, and make an authorized call using its actual schema.

## Convert data using a reviewed helper

First search for an existing relevant skill rather than generating an unreviewed program:

```json
{"q":"csv json conversion","kind":"skills","limit":10}
```

Read its instructions and helper source through `manage.get`. For example:

```json
{"operation":"get","skill_id":"csv-convert","filename":"scripts/convert.py"}
```

Review input handling, dependencies, output path permissions, and effects. Ask for real approval. When the host's trusted policy authorizes it, select the helper through `exec`:

```json
{
  "id":"<csv-convert skill id returned by search>",
  "script":"scripts/convert.py",
  "arguments":{"input":"/workspace/input.csv","output":"/workspace/output.json"}
}
```

This example does not imply that this helper exists or accepts these arguments. Its reviewed implementation defines that contract. A caller-supplied `consent` object does not grant Standard MCP approval. For trusted interactive CLI use after review and real approval:

```bash
skills skill run csv-convert --script scripts/convert.py --args '{"input":"/workspace/input.csv","output":"/workspace/output.json"}' --consent user_confirmed
```

The selected backend must permit the reviewed input/output access. Do not change to unrestricted mode merely because a filesystem or namespace operation was denied. Inspect the actual output and failure status; do not announce conversion before it succeeds.

## Continue an upstream request for user input

Search and describe the upstream callable as usual. If its response contains `inputRequests` and opaque `requestState`, show the requested choice/form to the real user. Construct `inputResponses` using the upstream's actual request IDs and advertised schema.

For MCP `exec`, inspect `execution.status` and the full result under `execution.raw_output`; continuation fields retain their upstream spellings `resultType`, `inputRequests`, and `requestState`. `input_required` means the operation is suspended, not completed.

Resume the same callable with the original validated arguments plus top-level `inputResponses` and `requestState`. The structure of those values comes from the real upstream response, so this guide does not invent response objects or state tokens. Do not pass protocol state as an arbitrary tool argument, silently answer for the user, or send a continuation to a different callable.

CLI `tool` and `exec` expose the same explicit continuation fields:

```bash
skills tool internal/action '{}' --input-responses "$INPUT_RESPONSES_JSON" --request-state "$REQUEST_STATE_JSON" --json
```

Here `internal/action` and `{}` must be replaced with the actual described callable and arguments, and both environment values must contain the real JSON continuation values. Consent and policy are still checked. A continuation request is not permission to replay unrelated writes.

## Save a binary asset without corruption

MCP `manage`:

```json
{"operation":"get","skill_id":"review-notes","filename":"assets/logo.png"}
```

If the response contains `data.encoding: "base64"`, decode `data.content` exactly once to bytes. Never save the explanatory `message` as the image or re-encode the already encoded payload. UTF-8 files can instead return text.

The CLI writes raw resource bytes:

```bash
skills skill show review-notes --file assets/logo.png > logo.png
```

This is a resource read, not script execution.

## Preserve metadata while improving a reusable workflow

With operator-authorized MCP management enabled, a targeted update is:

```json
{
  "operation":"update",
  "skill_id":"review-notes",
  "description":"Review release notes against the team's publication checklist."
}
```

The body, resources, and unrelated frontmatter should remain unchanged. To revise the procedure, supply only the intended new `skill_md` body and other fields that should change. Do not reconstruct all metadata from a search result, which intentionally omits most detail. Explicit empty tool/tag arrays clear those values; omission preserves them.

For an imported source, saving changes makes the destination locally modified. Preview upstream updates with `skills check`; source sync must not silently replace this local work. Resolve deliberately through the [ownership procedure](SKILLS.md#import-pin-check-update). Read-only overlay content cannot be changed by a managed update.

## Turn a completed workflow into instructions

Only when the user wants durable reuse and management is authorized, create a procedure describing verified steps. Keep the core document instruction-first:

```json
{
  "operation":"create",
  "name":"log-review",
  "description":"Review supplied application logs and report recurring failures.",
  "skill_md":"# Log review\n\nRead the supplied logs. Group recurring failures by message and location. Report counts from the actual data. Ask before changing configuration or deleting files."
}
```

This creates instructions, not an automatic log-processing program. Avoid invented statistics, successful test reports, or claims that a new skill has already been exercised. Add code only if the user needs it, with review, explicit selection, and the real execution boundary described in [operations](OPERATIONS.md).
