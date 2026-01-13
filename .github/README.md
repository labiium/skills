# GitHub Actions CI/CD Guide

This directory contains automated workflows for the skills.rs project.

## 📋 Workflows Overview

### 1. CI Workflow (`ci.yml`)
**Triggers:** Push/PR to `main` or `develop`

**Jobs:**
- **Test** - Run tests on Ubuntu, macOS, Windows
- **Clippy** - Lint with zero warnings tolerance
- **Format** - Check code formatting
- **Build** - Release builds for all platforms
- **Security Audit** - Check for vulnerabilities

**Purpose:** Ensure code quality and compatibility across platforms.

### 2. Release Workflow (`release.yml`)
**Triggers:** 
- Push tags matching `v*` (e.g., `v0.1.0`)
- Manual workflow dispatch with version input

**Jobs:**
- **Create Release** - Generate GitHub release
- **Build Release** - Build binaries for 6 platforms:
  - Linux x86_64
  - Linux aarch64 (ARM)
  - macOS x86_64 (Intel)
  - macOS aarch64 (Apple Silicon)
  - Windows x86_64
- **Publish Crates** - Push to crates.io (requires `CRATES_IO_TOKEN`)

**Purpose:** Automated binary releases and crate publishing.

### 3. Coverage Workflow (`coverage.yml`)
**Triggers:** Push/PR to `main` or `develop`

**Jobs:**
- Generate code coverage with `cargo-llvm-cov`
- Upload to Codecov (requires `CODECOV_TOKEN`)
- Create HTML reports (available as artifacts)
- Comment on PRs with coverage stats

**Purpose:** Track and maintain test coverage.

### 4. Docker Workflow (`docker.yml`)
**Triggers:** 
- Push to `main` or `develop`
- Push tags matching `v*`
- PR to `main`

**Jobs:**
- **Build and Push** - Multi-arch images (amd64, arm64)
- **Test Image** - Verify image functionality
- **Scan Image** - Security scanning with Trivy

**Registry:** GitHub Container Registry (ghcr.io)

**Purpose:** Automated container builds and security scanning.

### 5. Benchmark Workflow (`benchmark.yml`)
**Triggers:** 
- Push to `main`
- PR to `main`
- Manual dispatch

**Jobs:**
- Run benchmarks with Criterion
- Track performance over time
- Alert on performance regressions (>150%)
- Comment on PRs with results

**Purpose:** Performance tracking and regression detection.

### 6. Dependabot (`dependabot.yml`)
**Schedule:** Weekly on Monday at 9:00 AM

**Updates:**
- Cargo dependencies
- GitHub Actions versions

**Purpose:** Automated dependency updates with security patches.

## 🔐 Required Secrets

Add these secrets in GitHub repository settings:

| Secret | Required For | Description |
|--------|--------------|-------------|
| `GITHUB_TOKEN` | All workflows | Auto-provided by GitHub |
| `CODECOV_TOKEN` | Coverage | Upload to codecov.io |
| `CRATES_IO_TOKEN` | Release | Publish to crates.io |

## 🚀 Usage

### Creating a Release

```bash
# Tag the commit
git tag v0.1.0
git push origin v0.1.0

# Or use GitHub UI to create a release
```

This will:
1. Run all CI checks
2. Build binaries for all platforms
3. Create a GitHub release with assets
4. Publish crates to crates.io (if tag starts with 'v')

### Manual Release

Use GitHub Actions UI:
1. Go to Actions → Release
2. Click "Run workflow"
3. Enter version (e.g., `0.1.0`)
4. Click "Run workflow"

### Viewing Results

- **CI Status**: Check PR/commit status badges
- **Coverage Reports**: Download from Actions artifacts
- **Docker Images**: `ghcr.io/labiium/skills:latest`
- **Releases**: GitHub Releases page

## 📊 Status Badges

Add to README:

```markdown
[![CI](https://github.com/labiium/skills/actions/workflows/ci.yml/badge.svg)](https://github.com/labiium/skills/actions/workflows/ci.yml)
[![Coverage](https://github.com/labiium/skills/actions/workflows/coverage.yml/badge.svg)](https://github.com/labiium/skills/actions/workflows/coverage.yml)
[![Docker](https://github.com/labiium/skills/actions/workflows/docker.yml/badge.svg)](https://github.com/labiium/skills/actions/workflows/docker.yml)
```

## 🔧 Maintenance

### Updating Workflows

1. Edit YAML files in this directory
2. Test with `act` locally (optional)
3. Commit and push
4. Verify in GitHub Actions UI

### Disabling Workflows

Disable in GitHub repository settings:
- Settings → Actions → General → Disable specific workflows

### Caching

All workflows use GitHub Actions cache for:
- Cargo registry
- Cargo index  
- Target directory
- Docker layers

**Cache keys** include `Cargo.lock` hash for automatic invalidation.

## 🐛 Troubleshooting

### Test Failures

Check the test job logs:
```bash
# Run tests locally
cargo test --workspace --all-features --verbose
```

### Clippy Failures

Reproduce locally:
```bash
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

### Docker Build Failures

Test locally:
```bash
docker build -t skills:test .
docker run --rm skills:test --version
```

### Release Failures

Common issues:
- Missing secrets (CRATES_IO_TOKEN)
- Version already published to crates.io
- Platform-specific build errors

## 📚 Resources

- [GitHub Actions Documentation](https://docs.github.com/en/actions)
- [Rust CI Guide](https://rust-lang.github.io/rustup/dev-guide/ci.html)
- [Docker Multi-arch Builds](https://docs.docker.com/build/building/multi-platform/)
- [Cargo Publish Guide](https://doc.rust-lang.org/cargo/reference/publishing.html)

## 🤝 Contributing

When adding new workflows:
1. Test thoroughly in a fork first
2. Document the workflow purpose
3. Add appropriate caching
4. Use semantic job names
5. Update this README

## 📞 Support

For workflow issues:
- Open an issue on GitHub
- Check Actions logs for errors
- Review workflow YAML syntax