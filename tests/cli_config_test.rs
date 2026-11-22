#![cfg(target_os = "linux")]

use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

fn command(project: &Path, home: &Path, managed: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_skills"));
    command
        .current_dir(project)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env_remove("SKILLS_PATH")
        .env_remove("SKILLS_NO_SANDBOX")
        .env_remove("SKILLS_POLICY")
        .env_remove("MESURII_SKILLS_SOURCE_REVISION")
        .env("NO_PROXY", "127.0.0.1")
        .env("RUST_LOG", "error")
        .arg("--path")
        .arg(managed);
    command
}

fn run(project: &Path, home: &Path, managed: &Path, arguments: &[&str]) -> Result<()> {
    let result = command(project, home, managed).args(arguments).output()?;
    ensure!(
        result.status.success(),
        "{arguments:?}: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    Ok(())
}

#[test]
fn project_sources_survive_global_overlay_persistence_and_directory_changes() -> Result<()> {
    let fixture = tempfile::tempdir()?;
    let project = fixture.path().join("project");
    let home = fixture.path().join("home");
    let managed = fixture.path().join("managed");
    std::fs::create_dir_all(project.join(".skills"))?;
    std::fs::create_dir_all(project.join("source"))?;
    std::fs::create_dir_all(project.join("nested"))?;
    std::fs::create_dir_all(home.join("config/skills"))?;
    std::fs::write(
        home.join("config/skills/config.yaml"),
        "sandbox:\n  docker:\n    image: inherited-only:fixed\n",
    )?;
    let configuration = project.join(".skills/config.yaml");
    std::fs::write(&configuration, "use_global:\n  enabled: true\n")?;
    std::fs::write(project.join("source/SKILL.md"), "---\nname: example\ndescription: Preserve project source ownership\n---\n\nOriginal instructions\n")?;
    run(&project, &home, &managed, &["add", "./source"])?;
    let persisted: serde_yaml::Value =
        serde_yaml::from_str(&std::fs::read_to_string(&configuration)?)?;
    ensure!(
        persisted.get("sandbox").is_none(),
        "Global execution settings leaked into project configuration"
    );
    run(&project.join("nested"), &home, &managed, &["sync"])?;
    let installed = managed.join("skills/example/SKILL.md");
    ensure!(
        std::fs::read_to_string(&installed)?.contains("Original instructions"),
        "Source disappeared after overlay reconciliation"
    );
    let configuration = configuration.to_string_lossy();
    run(
        fixture.path(),
        &home,
        &managed,
        &["--config", &configuration, "sync"],
    )?;
    ensure!(
        installed.is_file(),
        "Source disappeared after changing directories"
    );
    Ok(())
}

fn fixture_command(root: &Path) -> Command {
    command(root, root, &root.join("managed"))
}

fn invoke(root: &Path, arguments: &[&str], status: i32) -> Result<Output> {
    let output = fixture_command(root).args(arguments).output()?;
    ensure!(
        output.status.code() == Some(status),
        "{arguments:?}: expected exit {status}, got {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(output)
}

fn write_package(root: &Path, name: &str, instructions: &str) -> Result<()> {
    let package = root.join(name);
    std::fs::create_dir_all(&package)?;
    std::fs::write(
        package.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: Review notes\n---\n\n{instructions}\n"),
    )?;
    Ok(())
}

fn git(root: &Path, args: &[&str]) -> Result<()> {
    let output = Command::new("git")
        .current_dir(root)
        .args([
            "-c",
            "user.name=CLI Tests",
            "-c",
            "user.email=cli@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(args)
        .output()?;
    ensure!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

#[test]
fn rejected_add_leaves_tracking_config_packages_and_ownership_unchanged() -> Result<()> {
    let fixture = tempfile::tempdir()?;
    let root = fixture.path();
    let source = root.join("source");
    let replacement = root.join("replacement");
    let config = root.join("config/skills/config.yaml");
    let skills = root.join("managed/skills");
    let ownership = skills.join(".agent-skills-sync.json");
    let installed = skills.join("example/SKILL.md");
    write_package(&source, "example", "Original instructions")?;
    write_package(&replacement, "example", "Replacement instructions")?;
    write_package(&replacement, "extra", "New instructions")?;

    invoke(root, &["add", "./source", "--skill", "absent"], 1)?;
    ensure!(!config.exists() && !ownership.exists() && !installed.exists());
    invoke(root, &["add", "./source"], 0)?;
    let config_before = std::fs::read(&config)?;
    let ownership_before = std::fs::read(&ownership)?;
    let installed_before = std::fs::read(&installed)?;

    invoke(root, &["add", "./replacement"], 1)?;
    ensure!(std::fs::read(&config)? == config_before);

    std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o444))?;
    let failure = invoke(root, &["add", "./replacement", "--force"], 1)?;
    ensure!(String::from_utf8_lossy(&failure.stderr).contains("Tracking config is read-only"));
    ensure!(std::fs::read(&config)? == config_before);
    ensure!(std::fs::read(&ownership)? == ownership_before);
    ensure!(std::fs::read(&installed)? == installed_before);
    ensure!(!skills.join("extra").exists());
    invoke(root, &["sync"], 0)?;
    ensure!(std::fs::read(&installed)? == installed_before);
    Ok(())
}

#[test]
fn add_rolls_back_replacements_if_tracking_finalization_fails() -> Result<()> {
    let fixture = tempfile::tempdir()?;
    let root = fixture.path();
    write_package(&root.join("source"), "example", "Original instructions")?;
    invoke(root, &["add", "./source"], 0)?;

    let config = root.join("config/skills/config.yaml");
    let skills = root.join("managed/skills");
    let ownership = skills.join(".agent-skills-sync.json");
    let installed = skills.join("example/SKILL.md");
    let config_before = std::fs::read(&config)?;
    let ownership_before = std::fs::read(&ownership)?;
    let installed_before = std::fs::read(&installed)?;
    let replacement = root.join("replacement");
    write_package(&replacement, "example", "Replacement instructions")?;
    write_package(&replacement, "extra", "New instructions")?;
    git(&replacement, &["init", "--initial-branch=main"])?;
    git(&replacement, &["add", "."])?;
    git(&replacement, &["commit", "-m", "Replacement"])?;

    // Revoke write permission after config preflight, while the source is being fetched.
    let bin = root.join("bin");
    std::fs::create_dir(&bin)?;
    let shim = bin.join("git");
    std::fs::write(
        &shim,
        "#!/bin/sh\n\
         if [ \"$3\" = clone ]; then chmod a-w \"$TRACKING_CONFIG\" || exit 1; fi\n\
         PATH=\"$REAL_PATH\" exec git \"$@\"\n",
    )?;
    std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755))?;
    let real_path = std::env::var_os("PATH").context("PATH is not set")?;
    let shim_path =
        std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(&real_path)))?;
    let output = fixture_command(root)
        .env("PATH", shim_path)
        .env("REAL_PATH", real_path)
        .env("TRACKING_CONFIG", &config)
        .args(["add", "./replacement", "--git-ref", "main", "--force"])
        .output()?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    ensure!(
        output.status.code() == Some(1) && stderr.contains("Cannot write tracking config"),
        "expected finalization to fail: {stderr}"
    );
    ensure!(std::fs::read(&config)? == config_before);
    ensure!(std::fs::read(&ownership)? == ownership_before);
    ensure!(std::fs::read(&installed)? == installed_before);
    ensure!(!skills.join("extra").exists());
    Ok(())
}

#[test]
fn add_and_list_use_the_configured_source_location_and_ref() -> Result<()> {
    let fixture = tempfile::tempdir()?;
    let source = fixture.path().join("source");
    write_package(&source, "example", "Committed instructions")?;
    git(&source, &["init", "--initial-branch=main"])?;
    git(&source, &["add", "."])?;
    git(&source, &["commit", "-m", "Initial packages"])?;
    git(&source, &["tag", "pinned"])?;
    write_package(&source, "main-only", "Added on main")?;
    git(&source, &["add", "."])?;
    git(&source, &["commit", "-m", "Add main-only package"])?;
    write_package(&source, "example", "Working tree instructions")?;
    write_package(&source, "working-only", "Not committed")?;

    let local = source.to_str().context("Non-UTF-8 fixture path")?;
    let url = reqwest::Url::from_file_path(&source)
        .map_err(|()| anyhow::anyhow!("Invalid fixture path"))?;
    for (case, configured, argument, git_ref, listing, instructions) in [
        (
            "checkout",
            url.as_str(),
            local,
            None,
            "example\nmain-only\n",
            "Committed instructions",
        ),
        (
            "working",
            local,
            url.as_str(),
            None,
            "example\nmain-only\nworking-only\n",
            "Working tree instructions",
        ),
        (
            "pinned",
            url.as_str(),
            local,
            Some("pinned"),
            "example\n",
            "Committed instructions",
        ),
    ] {
        let root = fixture.path().join(case);
        let global = root.join("config/skills");
        std::fs::create_dir_all(&global)?;
        std::fs::write(
            global.join("config.yaml"),
            serde_yaml::to_string(&json!({
                "agent_skills_repos": [{
                    "repo": configured, "git_ref": git_ref, "skills": [],
                }],
            }))?,
        )?;
        std::fs::create_dir(root.join(".skills"))?;
        let project = root.join(".skills/config.yaml");
        let initial = "use_global:\n  enabled: true\n";
        std::fs::write(&project, initial)?;
        std::fs::set_permissions(&project, std::fs::Permissions::from_mode(0o640))?;

        let output = invoke(&root, &["add", argument, "--list"], 0)?;
        ensure!(
            output.stdout == listing.as_bytes(),
            "{case}: unexpected package listing"
        );
        ensure!(std::fs::read_to_string(&project)? == initial);
        invoke(&root, &["add", argument, "--skill", "example"], 0)?;
        let installed = root.join("managed/skills/example/SKILL.md");
        ensure!(std::fs::read_to_string(&installed)?.contains(instructions));
        let tracked: Value = serde_yaml::from_slice(&std::fs::read(&project)?)?;
        let tracked = tracked
            .pointer("/agent_skills_repos/0")
            .context("Missing tracked source")?;
        ensure!(
            tracked.get("repo").and_then(Value::as_str) == Some(configured)
                && tracked.get("git_ref").and_then(Value::as_str) == git_ref
        );
        ensure!(std::fs::metadata(&project)?.permissions().mode() & 0o777 == 0o640);
        invoke(&root, &["check"], 0)?;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unavailable_optional_upstream_does_not_break_local_or_targeted_commands() -> Result<()> {
    use axum::{routing::post, Json, Router};
    use skillsrs::execution::upstream::MCP_CURRENT_PROTOCOL_VERSION;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let rejected_requests = Arc::new(AtomicUsize::new(0));
    let rejected = Arc::clone(&rejected_requests);
    let app = Router::new()
        .route(
            "/older/mcp",
            post(move |Json(request): Json<Value>| async move {
                rejected.fetch_add(1, Ordering::Relaxed);
                Json(json!({
                    "jsonrpc": "2.0",
                    "id": request.get("id"),
                    "error": { "code": -32601, "message": "Method not found" }
                }))
            }),
        )
        .route(
            "/ready/mcp",
            post(|Json(request): Json<Value>| async move {
                let result = match request.get("method").and_then(Value::as_str) {
                    Some("server/discover") => json!({
                        "resultType": "complete",
                        "supportedVersions": [MCP_CURRENT_PROTOCOL_VERSION],
                        "capabilities": { "tools": {} }
                    }),
                    Some("tools/list") => json!({
                        "resultType": "complete",
                        "tools": [{ "name": "lookup", "inputSchema": { "type": "object" } }]
                    }),
                    _ => {
                        return Json(json!({
                            "jsonrpc": "2.0",
                            "id": request.get("id"),
                            "error": { "code": -32601, "message": "Method not found" }
                        }))
                    }
                };
                Json(json!({ "jsonrpc": "2.0", "id": request.get("id"), "result": result }))
            }),
        );
    tokio::spawn(async move { axum::serve(listener, app).await });

    let fixture = tempfile::tempdir()?;
    let root = fixture.path();
    invoke(root, &["init"], 0)?;
    invoke(
        root,
        &[
            "skill",
            "create",
            "example",
            "--description",
            "Review notes",
            "--content",
            "Review the notes",
        ],
        0,
    )?;
    let config = root.join(".skills/config.yaml");
    let mut before = std::fs::read_to_string(&config)?;
    before.push_str("policy:\n  trusted_servers: [ready]\n");
    std::fs::write(&config, &before)?;
    let older = format!("http://{address}/older");
    let result = invoke(root, &["add-mcp", "http", "older", &older], 1)?;
    ensure!(String::from_utf8_lossy(&result.stderr).contains(MCP_CURRENT_PROTOCOL_VERSION));
    ensure!(
        std::fs::read_to_string(&config)? == before,
        "failed discovery changed config"
    );
    invoke(root, &["add-mcp", "http", "older", &older, "--no-test"], 0)?;
    let ready = format!("http://{address}/ready");
    invoke(root, &["add-mcp", "http", "ready", &ready], 0)?;
    ensure!(rejected_requests.load(Ordering::Relaxed) == 1);

    let search = invoke(root, &["search", "*", "--kind", "skills", "--json"], 0)?;
    let search: Value = serde_json::from_slice(&search.stdout)?;
    let id = search
        .pointer("/matches/0/id")
        .and_then(Value::as_str)
        .context("missing local skill")?;
    let run = invoke(root, &["skill", "run", id], 0)?;
    ensure!(String::from_utf8_lossy(&run.stdout).contains("Review the notes"));
    let tool = invoke(root, &["tool", "ready/lookup"], 0)?;
    let tool: Value = serde_json::from_slice(&tool.stdout)?;
    ensure!(tool.get("name").and_then(Value::as_str) == Some("lookup"));
    ensure!(
        rejected_requests.load(Ordering::Relaxed) == 1,
        "unrelated upstream was contacted"
    );

    let mixed = invoke(root, &["list", "--json"], 0)?;
    let names = String::from_utf8_lossy(&mixed.stdout);
    ensure!(names.contains("lookup") && names.contains("example"));
    ensure!(String::from_utf8_lossy(&mixed.stderr).contains("optional upstream older"));
    invoke(root, &["list", "older"], 1)?;
    Ok(())
}

#[test]
fn stdio_mcp_registration_preserves_arguments_verbatim() -> Result<()> {
    use skillsrs::execution::upstream::UpstreamConfig;

    #[derive(serde::Deserialize)]
    struct Upstreams {
        upstreams: Vec<UpstreamConfig>,
    }

    let fixture = tempfile::tempdir()?;
    let root = fixture.path();
    invoke(root, &["init"], 0)?;
    let command_args = [
        "/path to/server",
        "--root",
        "/path with spaces",
        "",
        "--no-test",
    ];
    let mut add = vec!["add-mcp", "stdio", "reference", "--no-test", "--"];
    add.extend(command_args);
    invoke(root, &add, 0)?;
    let config = root.join(".skills/config.yaml");
    let read = || -> Result<Vec<String>> {
        let saved: Upstreams = serde_yaml::from_slice(&std::fs::read(&config)?)?;
        saved
            .upstreams
            .into_iter()
            .next()
            .and_then(|upstream| upstream.command)
            .context("missing stdio command")
    };
    ensure!(
        read()? == command_args,
        "registration changed argv boundaries"
    );

    let edited = ["/some other/server", "--root", "/other path"];
    let mut edit = vec!["edit-mcp", "reference", "--"];
    edit.extend(edited);
    invoke(root, &edit, 0)?;
    ensure!(read()? == edited, "editing changed argv boundaries");
    Ok(())
}

#[tokio::test]
async fn additive_imports_survive_explicit_sync_without_startup_updates() -> Result<()> {
    let fixture = tempfile::tempdir()?;
    let root = fixture.path();
    invoke(root, &["check"], 0)?;
    ensure!(!root.join("managed").exists() && !root.join("config").exists());
    invoke(root, &["init"], 0)?;
    let config = root.join(".skills/config.yaml");
    let initialized = std::fs::read(&config)?;
    invoke(root, &["init"], 1)?;
    ensure!(
        std::fs::read(&config)? == initialized,
        "repeated init overwrote config"
    );
    for name in ["alpha", "beta"] {
        let path = root.join("source").join(name);
        std::fs::create_dir_all(&path)?;
        std::fs::write(
            path.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: Review notes\n---\n\nInitial instructions\n"),
        )?;
        invoke(root, &["add", "./source", "--skill", name], 0)?;
    }
    let installed = root.join("managed/skills");
    ensure!(
        installed.join("alpha/SKILL.md").is_file(),
        "second add removed alpha"
    );
    invoke(root, &["check"], 0)?;

    let source = root.join("source/beta/SKILL.md");
    let original = std::fs::read_to_string(&source)?;
    let updated = original.replace("Initial", "Updated");
    std::fs::write(&source, &updated)?;
    invoke(root, &["check"], 1)?;

    let mut server = tokio::process::Command::from(fixture_command(root))
        .args(["server", "stdio"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    server.stdin.as_mut().context("missing server stdin")?.write_all(
        b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-11-25\",\"capabilities\":{},\"clientInfo\":{\"name\":\"test\",\"version\":\"1\"}}}\n",
    ).await?;
    let mut stdout = BufReader::new(server.stdout.take().context("missing server stdout")?);
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(10), stdout.read_line(&mut line)).await??;
    let response: Value = serde_json::from_str(&line)?;
    ensure!(
        response.get("result").is_some(),
        "MCP handshake failed: {response}"
    );
    let installed_beta = installed.join("beta/SKILL.md");
    ensure!(
        std::fs::read_to_string(&installed_beta)? == original,
        "server startup applied update"
    );
    server.kill().await?;

    invoke(root, &["update"], 0)?;
    ensure!(std::fs::read_to_string(installed_beta)? == updated);
    ensure!(
        installed.join("alpha/SKILL.md").is_file(),
        "sync removed alpha"
    );
    std::fs::rename(root.join("source"), root.join("missing-source"))?;
    invoke(root, &["check"], 2)?;
    invoke(root, &["--config", "nonexistent.yaml", "check"], 2)?;
    Ok(())
}
