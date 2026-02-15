# Best Practices: LLM Guide to skills.rs

## Overview

This guide covers how LLMs should effectively use skills.rs for tool and skill management. Follow these patterns for optimal discovery, creation, and execution.

---

## The 4 MCP Tools: Reference

| Tool | Purpose | When to Use |
|------|---------|-------------|
| `search` | Discovery | Find tools/skills by keywords, descriptions, or capabilities |
| `schema` | Inspection | Get parameter schemas before execution |
| `exec` | Execution | Run tools/skills with validated arguments |
| `manage` | Lifecycle | Create, read, update, delete skills |

**Golden Rule**: Always use `search` → `schema` → `exec` workflow. Never skip schema inspection.

---

## Part 1: Discovering Tools and Skills

### 1.1 Effective Search Strategies

#### Use Descriptive Keywords
```json
// ❌ Bad: Too vague
{ "q": "file" }

// ✅ Good: Specific intent
{ "q": "read text files with encoding detection" }
```

#### Filter by Kind
```json
// Find only skills (workflows)
{ "q": "data processing", "kind": "skills" }

// Find only tools (atomic operations)
{ "q": "file read", "kind": "tools" }
```

#### Use Fuzzy Matching for Exploration
```json
// When unsure of exact names
{ "q": "csv parser", "mode": "fuzzy", "limit": 10 }
```

#### Filter by Capabilities
```json
// Find tools with specific parameters
{
  "q": "file",
  "filters": {
    "requires": ["path", "encoding"]
  }
}
```

### 1.2 Interpreting Search Results

```json
{
  "matches": [{
    "id": "skill:csv-processor@1.0.0",
    "name": "csv-processor",
    "kind": "skill",
    "description_snippet": "Process CSV files with validation...",
    "inputs": ["file_path", "delimiter", "has_header"],
    "score": 95.5,
    "schema_digest": "a1b2c3d4"
  }]
}
```

**Key Fields**:
- `id`: Unique identifier for exec calls
- `kind`: "tool" (atomic) vs "skill" (workflow)
- `inputs`: Required/optional parameters
- `schema_digest`: Version fingerprint

### 1.3 Handling No Results

If search returns nothing:

1. **Try broader terms**
   ```json
   { "q": "data" } // Instead of "CSV column aggregation"
   ```

2. **Check spelling**
   ```json
   { "q": "calculator" } // Instead of "calclator"
   ```

3. **Use regex mode for patterns**
   ```json
   { "q": "csv.*process", "mode": "regex" }
   ```

---

## Part 2: Inspecting Before Execution

### 2.1 Always Get Schema First

```json
// After finding a skill/tool
{ "id": "skill:csv-processor@1.0.0", "format": "both" }
```

### 2.2 Understanding Schema Output

```json
{
  "callable": {
    "id": "skill:csv-processor@1.0.0",
    "kind": "skill",
    "name": "csv-processor",
    "fq_name": "skill.csv-processor"
  },
  "input_schema": {
    "type": "object",
    "properties": {
      "file_path": {
        "type": "string",
        "description": "Path to CSV file"
      },
      "delimiter": {
        "type": "string",
        "enum": [",", ";", "\t"],
        "default": ","
      },
      "has_header": {
        "type": "boolean",
        "default": true
      }
    },
    "required": ["file_path"]
  },
  "signature": {
    "required": ["file_path"],
    "optional": ["delimiter", "has_header"],
    "constraints": {
      "file_path": "string; Path to CSV file",
      "delimiter": "string; One of: ,, ;, \\\\t"
    }
  }
}
```

**Critical Checks**:
1. ✅ `required` fields must be provided
2. ✅ Check `enum` for valid values
3. ✅ Use `default` values when appropriate
4. ✅ Respect `type` constraints

### 2.3 Handling Schema Changes

Compare `schema_digest` between calls to detect updates:

```json
// Previous call had digest: "a1b2c3d4"
// Current call shows: "e5f6g7h8"
// → Schema changed! Re-inspect parameters.
```

---

## Part 3: Executing Tools and Skills

### 3.1 Safe Execution Pattern

```json
// 1. Validate only first (dry_run)
{
  "id": "skill:csv-processor@1.0.0",
  "arguments": {
    "file_path": "/data/sales.csv",
    "delimiter": ","
  },
  "dry_run": true
}

// 2. If validation passes, execute
{
  "id": "skill:csv-processor@1.0.0",
  "arguments": {
    "file_path": "/data/sales.csv",
    "delimiter": ","
  }
}
```

### 3.2 Handling Timeouts

```json
// Set appropriate timeout for long operations
{
  "id": "skill:large-file-processor@1.0.0",
  "arguments": { "file_path": "/data/10gb.csv" },
  "timeout_ms": 300000  // 5 minutes
}
```

**Timeout Guidelines**:
| Operation Type | Recommended Timeout |
|----------------|---------------------|
| File read | 10-30s |
| Network request | 30-60s |
| Data processing | 60-300s |
| Complex workflow | 300-600s |

### 3.3 Error Handling

**Common Errors**:

```json
// Validation Error
{
  "error": "Missing required argument: file_path"
}
// Fix: Add the missing parameter

// Timeout Error
{
  "error": "Timeout exceeded: 30000ms"
}
// Fix: Increase timeout_ms or optimize operation

// Policy Denial
{
  "error": "Execution denied: Tool requires user consent"
}
// Fix: Request user confirmation, then retry with consent
```

### 3.4 Using Consent Tokens

For high-risk operations:

```json
{
  "id": "skill:database-cleanup@1.0.0",
  "arguments": { "confirm_delete": true },
  "consent": {
    "level": "user_confirmed",
    "token": "user-provided-token-123"
  }
}
```

---

## Part 4: Creating Custom Skills

### 4.1 When to Create a Skill

Create a skill when:
- ✅ You need reusable workflow logic
- ✅ You want to bundle helper scripts
- ✅ Complex multi-step operations are needed
- ✅ You want to share capabilities across conversations

Don't create a skill when:
- ❌ A simple one-off calculation will do
- ❌ An existing tool already handles it
- ❌ The operation is purely informational

### 4.2 Skill Design Best Practices

#### Clear Naming
```json
// ❌ Bad: Too vague
"name": "processor"

// ✅ Good: Descriptive
"name": "csv-to-json-converter"
```

#### Comprehensive Description
```json
// ❌ Bad: Too brief
"description": "Converts files"

// ✅ Good: Detailed
"description": "Converts CSV files to JSON format with automatic type detection, supports nested objects from column prefixes"
```

#### Define Input Schema
```json
"inputs": {
  "type": "object",
  "properties": {
    "input_file": {
      "type": "string",
      "description": "Path to input CSV file"
    },
    "output_file": {
      "type": "string",
      "description": "Path for output JSON file"
    },
    "pretty_print": {
      "type": "boolean",
      "description": "Format JSON with indentation",
      "default": true
    }
  },
  "required": ["input_file", "output_file"]
}
```

### 4.3 Creating Bundled Scripts

#### Template for Python Scripts

```python
#!/usr/bin/env python3
"""
Bundled tool for [skill-name]
Purpose: [brief description]
"""
import json
import os
import sys

def main():
    # Read arguments from environment
    if 'SKILL_ARGS_JSON' in os.environ:
        args = json.loads(os.environ['SKILL_ARGS_JSON'])
    else:
        with open(os.environ['SKILL_ARGS_FILE']) as f:
            args = json.load(f)
    
    # Extract parameters with defaults
    input_path = args.get('input_file')
    output_path = args.get('output_file')
    option = args.get('option', 'default_value')
    
    # Validate required parameters
    if not input_path:
        print(json.dumps({
            'success': False,
            'error': 'Missing required parameter: input_file'
        }))
        sys.exit(1)
    
    try:
        # Your logic here
        result = process_data(input_path, output_path, option)
        
        # Return structured result
        print(json.dumps({
            'success': True,
            'result': result,
            'metadata': {
                'input': input_path,
                'output': output_path
            }
        }))
        
    except Exception as e:
        print(json.dumps({
            'success': False,
            'error': str(e)
        }))
        sys.exit(1)

def process_data(input_path, output_path, option):
    # Implementation
    pass

if __name__ == '__main__':
    main()
```

#### Template for Bash Scripts

```bash
#!/bin/bash
# Bundled tool for [skill-name]

# Read arguments
if [ -n "$SKILL_ARGS_JSON" ]; then
    ARGS_JSON="$SKILL_ARGS_JSON"
else
    ARGS_JSON=$(cat "$SKILL_ARGS_FILE")
fi

# Extract values using jq (or grep/sed fallback)
INPUT_FILE=$(echo "$ARGS_JSON" | jq -r '.input_file // empty')
OUTPUT_FILE=$(echo "$ARGS_JSON" | jq -r '.output_file // empty')

# Validate
if [ -z "$INPUT_FILE" ]; then
    echo '{"success": false, "error": "Missing input_file"}'
    exit 1
fi

# Execute
if result=$(process_data "$INPUT_FILE" "$OUTPUT_FILE"); then
    echo "{\"success\": true, \"result\": \"$result\"}"
else
    echo "{\"success\": false, \"error\": \"Processing failed\"}"
    exit 1
fi
```

### 4.4 Complete Skill Creation Example

```json
{
  "operation": "create",
  "name": "log-analyzer",
  "version": "1.0.0",
  "description": "Analyze log files and generate summary reports",
  "skill_md": "# Log Analyzer\n\n## Purpose\nAnalyze application log files to extract error rates, response times, and top errors.\n\n## Steps\n1. Parse log file format (supports JSON and plain text)\n2. Extract timestamp, level, message fields\n3. Calculate statistics\n4. Generate summary report\n\n## Tools Used\n- analyzer.py - Main analysis script\n\n## Expected Output\nJSON report with error counts, response time percentiles, and top errors.",
  "bundled_files": [
    ["analyzer.py", "#!/usr/bin/env python3\nimport json\nimport os\nimport re\nfrom collections import Counter\nfrom datetime import datetime\n\ndef parse_log_line(line):\n    # Parse common log formats\n    patterns = [\n        r'(?P<timestamp>\\d{4}-\\d{2}-\\d{2}.*?)\\s+(?P<level>\\w+)\\s+(?P<message>.*)',\n        r'\\[(?P<timestamp>[^\\]]+)\\]\\s+(?P<level>\\w+).*?:\\s*(?P<message>.*)'\n    ]\n    for pattern in patterns:\n        match = re.match(pattern, line)\n        if match:\n            return match.groupdict()\n    return None\n\ndef analyze_logs(log_path):\n    levels = Counter()\n    errors = []\n    timestamps = []\n    \n    with open(log_path) as f:\n        for line in f:\n            parsed = parse_log_line(line.strip())\n            if parsed:\n                levels[parsed['level']] += 1\n                if parsed['level'].upper() == 'ERROR':\n                    errors.append(parsed['message'])\n    \n    return {\n        'total_lines': sum(levels.values()),\n        'level_counts': dict(levels),\n        'error_rate': levels['ERROR'] / sum(levels.values()) if levels else 0,\n        'top_errors': [msg for msg, _ in Counter(errors).most_common(5)]\n    }\n\nif __name__ == '__main__':\n    args = json.loads(os.environ.get('SKILL_ARGS_JSON', '{}'))\n    log_path = args.get('log_file')\n    \n    if not log_path:\n        print(json.dumps({'success': False, 'error': 'Missing log_file'}))\n        exit(1)\n    \n    try:\n        result = analyze_logs(log_path)\n        print(json.dumps({'success': True, 'result': result}))\n    except Exception as e:\n        print(json.dumps({'success': False, 'error': str(e)}))\n        exit(1)"]
  ],
  "uses_tools": ["filesystem/read_file"],
  "tags": ["logs", "analysis", "monitoring"]
}
```

---

## Part 5: Managing Skills

### 5.1 Reading Skill Content

**Get overview**:
```json
{ "operation": "get", "skill_id": "log-analyzer" }
// Returns: SKILL.md content + metadata
```

**Get specific script**:
```json
{ "operation": "get", "skill_id": "log-analyzer", "filename": "analyzer.py" }
// Returns: Full script source code
```

**Get supporting files**:
```json
{ "operation": "get", "skill_id": "log-analyzer", "filename": "config.json" }
```

### 5.2 Updating Skills

**Full update** (replace everything):
```json
{
  "operation": "update",
  "skill_id": "log-analyzer",
  "name": "log-analyzer",
  "description": "Updated log analyzer with better parsing",
  "skill_md": "# Updated content...",
  "bundled_files": [
    ["analyzer.py", "# Improved script..."]
  ]
}
```

**Best Practice**: Always read current content before updating to avoid losing data.

### 5.3 Version Management

```json
// Create v2 of existing skill
{
  "operation": "create",
  "name": "log-analyzer-v2",
  "version": "2.0.0",
  "description": "Enhanced version with new features",
  ...
}
```

**Version Guidelines**:
- Use semantic versioning (MAJOR.MINOR.PATCH)
- Major changes → new MAJOR version
- New features → new MINOR version
- Bug fixes → new PATCH version

---

## Part 6: Security Best Practices

### 6.1 Input Validation

**Always validate user-provided paths**:
```python
# In your bundled script
import os

user_path = args.get('file_path')
allowed_base = os.environ.get('SKILL_WORKDIR', '/tmp')

# Resolve and validate
full_path = os.path.abspath(os.path.join(allowed_base, user_path))
if not full_path.startswith(allowed_base):
    raise ValueError("Path traversal detected")
```

### 6.2 Sanitization

**Escape shell commands**:
```python
import shlex

user_input = args.get('query')
safe_input = shlex.quote(user_input)
# Use safe_input in shell commands
```

**Validate regex patterns**:
```python
import re

pattern = args.get('pattern')
try:
    re.compile(pattern)
except re.error:
    raise ValueError("Invalid regex pattern")
```

### 6.3 Resource Limits

**Handle large files**:
```python
MAX_FILE_SIZE = 100 * 1024 * 1024  # 100MB
MAX_LINES = 1000000

file_size = os.path.getsize(file_path)
if file_size > MAX_FILE_SIZE:
    raise ValueError(f"File too large: {file_size} bytes")

line_count = sum(1 for _ in open(file_path))
if line_count > MAX_LINES:
    raise ValueError(f"Too many lines: {line_count}")
```

### 6.4 Sensitive Data

**Never log secrets**:
```python
# ❌ Bad
logger.info(f"Connecting with password: {password}")

# ✅ Good
logger.info("Connecting to database...")
```

**Use environment variables**:
```python
# Read API keys from env, not args
api_key = os.environ.get('API_KEY')
if not api_key:
    raise ValueError("API_KEY not set")
```

---

## Part 7: Common Patterns

### 7.1 Multi-Step Workflow

When a skill needs to call multiple tools:

```json
// SKILL.md for multi-step skill
"""
# Data Pipeline

## Steps
1. Read source file using filesystem/read_file
2. Transform data using transform.py bundled script
3. Write result using filesystem/write_file
4. Validate output using validate.py bundled script

## Parameters
- input_path: Source file path
- output_path: Destination file path
- transform_type: Type of transformation to apply
"""
```

### 7.2 Conditional Execution

```python
# In bundled script
args = json.loads(os.environ.get('SKILL_ARGS_JSON', '{}'))
mode = args.get('mode', 'standard')

if mode == 'fast':
    result = quick_analysis()
elif mode == 'thorough':
    result = deep_analysis()
else:
    result = standard_analysis()
```

### 7.3 Progress Reporting

For long operations, emit progress:

```python
import sys

total = len(items)
for i, item in enumerate(items):
    process(item)
    progress = (i + 1) / total * 100
    # Emit progress to stderr (won't interfere with JSON output)
    print(f"Progress: {progress:.1f}%", file=sys.stderr)
```

### 7.4 Error Recovery

```python
def process_with_retry(item, max_retries=3):
    for attempt in range(max_retries):
        try:
            return process(item)
        except TransientError as e:
            if attempt < max_retries - 1:
                time.sleep(2 ** attempt)  # Exponential backoff
            else:
                raise
```

---

## Part 8: Testing and Debugging

### 8.1 Testing Skills

**Test pattern**:
```bash
# 1. Create skill
skills skill create test-skill --description "Test" --skill-md ./SKILL.md

# 2. List to verify registration
skills list

# 3. Get schema
skills tool local/test-skill

# 4. Execute with test data
skills tool local/test-skill '{"test_param": "value"}'

# 5. Check script content
skills skill show test-skill --file script.py
```

### 8.2 Debugging Failed Executions

**Enable tracing**:
```json
{
  "id": "skill:my-skill@1.0.0",
  "arguments": {...},
  "trace": {
    "include_timing": true,
    "include_steps": true
  }
}
```

**Check logs**:
```bash
# View execution history
skills logs --skill my-skill --last 10
```

### 8.3 Dry Runs

Always validate before executing:
```json
{
  "id": "skill:risky-operation@1.0.0",
  "arguments": {...},
  "dry_run": true
}
```

---

## Part 9: Performance Optimization

### 9.1 Caching Results

```python
import hashlib
import json
import os

def cached_process(args):
    # Create cache key from arguments
    cache_key = hashlib.md5(json.dumps(args, sort_keys=True).encode()).hexdigest()
    cache_file = f"/tmp/cache/{cache_key}.json"
    
    if os.path.exists(cache_file):
        with open(cache_file) as f:
            return json.load(f)
    
    result = expensive_operation(args)
    
    with open(cache_file, 'w') as f:
        json.dump(result, f)
    
    return result
```

### 9.2 Lazy Loading

Only load heavy resources when needed:

```python
def load_model():
    # Expensive operation
    return heavy_ml_model()

# Don't load at module level
# model = load_model()  # ❌ Bad

# Load on first use
_model = None
def get_model():
    global _model
    if _model is None:
        _model = load_model()
    return _model
```

### 9.3 Streaming Large Outputs

For very large results, consider chunking:

```python
def stream_results(items):
    buffer = []
    for item in items:
        buffer.append(process(item))
        if len(buffer) >= 1000:
            yield buffer
            buffer = []
    if buffer:
        yield buffer
```

---

## Part 10: Troubleshooting Guide

### Problem: Skill Not Found

**Symptoms**: `Callable not found: skill:name@1.0.0`

**Solutions**:
1. Check exact name: `skills list | grep name`
2. Verify version: Some calls need `@version`, some don't
3. Try without version suffix: `skill:name` instead of `skill:name@1.0.0`

### Problem: Script Permission Denied

**Symptoms**: `Permission denied` when executing bundled script

**Solutions**:
1. Ensure script has shebang: `#!/usr/bin/env python3`
2. Check interpreter exists: `which python3`
3. Verify sandbox allows execution

### Problem: Missing Dependencies

**Symptoms**: `ModuleNotFoundError: No module named 'xyz'`

**Solutions**:
1. Use standard library only when possible
2. Document required packages in SKILL.md
3. Use inline bundling for small dependencies

### Problem: Schema Mismatch

**Symptoms**: `Missing required argument: xyz`

**Solutions**:
1. Re-fetch schema: `schema` tool
2. Check required vs optional fields
3. Verify parameter types match

### Problem: Timeout Errors

**Symptoms**: `Timeout exceeded: 30000ms`

**Solutions**:
1. Increase timeout: `"timeout_ms": 60000`
2. Optimize script performance
3. Process data in chunks
4. Use async operations

---

## Quick Reference Card

### Search
```json
{ "q": "keywords", "kind": "skills|tools|any", "mode": "literal|regex|fuzzy", "limit": 10 }
```

### Schema
```json
{ "id": "skill:name@version", "format": "json_schema|signature|both" }
```

### Execute
```json
{ "id": "skill:name@version", "arguments": {...}, "timeout_ms": 30000, "dry_run": false }
```

### Create Skill
```json
{ "operation": "create", "name": "skill-name", "description": "...", "skill_md": "...", "bundled_files": [["file.py", "content"]] }
```

### Get Skill
```json
{ "operation": "get", "skill_id": "skill-name", "filename": "optional-specific-file" }
```

### Update Skill
```json
{ "operation": "update", "skill_id": "skill-name", "name": "skill-name", "description": "...", "skill_md": "..." }
```

### Delete Skill
```json
{ "operation": "delete", "skill_id": "skill-name" }
```

---

## Summary Checklist

- [ ] Always search before creating (avoid duplicates)
- [ ] Always get schema before executing
- [ ] Use dry_run for risky operations
- [ ] Set appropriate timeouts
- [ ] Handle errors gracefully
- [ ] Validate all inputs
- [ ] Use structured JSON output
- [ ] Document skills thoroughly
- [ ] Test before sharing
- [ ] Version appropriately

---

*This guide follows the principle: "Infinite Skills. Finite Context." Use tools wisely, create skills thoughtfully, and always prioritize safety and clarity.*
