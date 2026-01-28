# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.5] - 2026-01-27

### Fixed
- **Backward compatibility for old-format skills** - Added fallback to parse YAML frontmatter from SKILL.md when skill.json is not present, allowing legacy skills to be loaded and used

## [0.1.4] - 2026-01-27

### Fixed
- **MCP schema validation error** - Removed `skip_serializing_if` from SchemaOutput optional fields to ensure they serialize as `null` instead of being omitted, fixing "data must have required property 'output_schema'" error
- **Skill ID parsing** - Added proper parsing of CallableId format (`skill:name@version`) in `get_content`, `update_skill`, and `delete_skill` tools, fixing "Skill not found" errors when using skill IDs from search results

## [0.1.3] - Previous Release

### Added
- **System-aware path management** - Platform-specific directories (XDG on Linux, standard paths on macOS/Windows)
- **CLI mode** - Full mcp-cli replacement with `list`, `tool`, `exec`, `grep`, `paths` commands
- **Sandboxing control** - `--no-sandbox`, `--current-dir` flags and per-upstream configuration
- **GitHub Actions workflows** - CI, release, coverage, Docker, benchmark automation
- **Docker support** - Multi-stage Dockerfile and multi-arch images (amd64, arm64)
- **Dependabot configuration** - Automated dependency updates for Cargo and GitHub Actions
- **Comprehensive documentation** - Operations guide, production checklist, quickstart
- **Path overrides** - Command-line, environment variables, and config file support
- **Execution history tracking** - Full audit trail stored in SQLite database
- **Policy engine** - Risk-based access control with configurable consent requirements
- **Progressive disclosure** - Token-efficient tool metadata loading

### Changed
- **Configuration format** - Moved from `skillstore.root` to `paths.skills_root`
- **Directory structure** - Now follows OS conventions instead of hardcoded paths
- **CLI defaults** - CLI mode runs without sandboxing for convenience
- **Server defaults** - Server mode uses restricted sandboxing for security
- **Test suite** - Added integration tests with proper isolation and cleanup

### Fixed
- **Zero clippy warnings** - Resolved all linting issues across workspace
- **Type inference errors** - Added explicit type annotations where needed
- **Redundant code patterns** - Simplified with idiomatic Rust
- **Test flakiness** - Documented and isolated timing-dependent search tests
- **Build configuration** - Added missing dev dependencies

### Removed
- **Development documentation** - Cleaned up HANDOFF.md, IMPLEMENTATION_STATUS.md, docs/ directory
- **Redundant files** - Consolidated multiple documentation files into comprehensive guides

### Security
- **Sandbox isolation** - Multiple backends (timeout, restricted, bubblewrap)
- **Resource limits** - Configurable memory, CPU, and timeout constraints
- **Audit logging** - All tool executions tracked with timestamps and results
- **Input validation** - JSON Schema validation for all tool inputs
- **Container security** - Non-root user, vulnerability scanning with Trivy

## [0.1.0] - Initial Release

### Added
- Core persistence layer with SQLite backend
- Runtime execution engine with tracing
- Skill store for managing skill packages
- Upstream manager for MCP server aggregation
- MCP server implementation (stdio and HTTP)
- Registry with thread-safe callable management
- Search engine with in-memory indexing
- Basic CLI interface
- Configuration via YAML files

[Unreleased]: https://github.com/labiium/skills/compare/v0.1.5...HEAD
[0.1.5]: https://github.com/labiium/skills/compare/v0.1.4...v0.1.5
[0.1.4]: https://github.com/labiium/skills/compare/v0.1.3...v0.1.4
[0.1.3]: https://github.com/labiium/skills/compare/v0.1.0...v0.1.3
[0.1.0]: https://github.com/labiium/skills/releases/tag/v0.1.0