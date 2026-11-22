use anyhow::{bail, ensure, Context, Result};
use skillsrs::storage::sync::{
    canonical_source, normalize_source, AgentSkillsRepoConfig, AgentSkillsSync, SyncMetadata,
};
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

fn package(root: &Path, name: &str, body: &str) -> Result<()> {
    let directory = root.join(name);
    std::fs::create_dir_all(&directory)?;
    std::fs::write(
        directory.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: Test package\n---\n{body}\n"),
    )?;
    Ok(())
}

fn config(root: &Path) -> AgentSkillsRepoConfig {
    AgentSkillsRepoConfig {
        repo: root.to_string_lossy().into_owned(),
        git_ref: None,
        skills: None,
        alias: None,
    }
}

fn git(root: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .current_dir(root)
        .args([
            "-c",
            "user.name=Sync Tests",
            "-c",
            "user.email=sync@example.invalid",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()?;
    if !output.status.success() {
        bail!("git: {}", String::from_utf8_lossy(&output.stderr));
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

fn commit(root: &Path) -> Result<String> {
    git(root, &["add", "--all"])?;
    git(root, &["commit", "-m", "package state"])?;
    git(root, &["rev-parse", "HEAD"])
}

#[tokio::test]
async fn nested_source_folders_install_under_the_declared_skill_name() -> Result<()> {
    let source = tempfile::tempdir()?;
    let installed = tempfile::tempdir()?;
    package(source.path(), "canonical-skill", "Imported instructions")?;
    std::fs::rename(
        source.path().join("canonical-skill"),
        source.path().join("source-folder"),
    )?;
    let mut sync = AgentSkillsSync::new(installed.path()).await?;
    let report = sync.add(&config(source.path()), false).await?;
    ensure!(
        report.added == ["canonical-skill"],
        "Declared name was not installed"
    );
    let package = skillsrs::storage::agent_skills::AgentSkill::from_directory(
        &installed.path().join("canonical-skill"),
    )
    .await?;
    ensure!(
        package.frontmatter.name == "canonical-skill",
        "Installed name changed"
    );
    ensure!(
        !installed.path().join("source-folder").exists(),
        "Source folder leaked into installation"
    );
    Ok(())
}

#[tokio::test]
async fn discovery_lists_candidates_but_import_validates_only_the_selection() -> Result<()> {
    let source = tempfile::tempdir()?;
    let installed = tempfile::tempdir()?;
    package(source.path(), "valid", "Selected instructions")?;
    package(source.path(), "invalid-meta", "Unselected instructions")?;
    std::fs::write(
        source.path().join("invalid-meta/SKILL.md"),
        format!(
            "---\nname: invalid-meta\ndescription: {}\n---\nInvalid metadata.\n",
            "x".repeat(1025)
        ),
    )?;
    let mut selected = config(source.path());
    selected.skills = Some(vec!["valid".to_string()]);
    ensure!(
        AgentSkillsSync::discover_source(&selected).await? == ["invalid-meta", "valid"],
        "Discovery failed to enumerate candidate names"
    );
    let mut sync = AgentSkillsSync::new(installed.path()).await?;
    let added = sync.add(&selected, false).await?;
    ensure!(
        added.errors.is_empty() && added.added == ["valid"],
        "Unselected metadata blocked import: {:?}",
        added.errors
    );
    let rejected = sync.add(&config(source.path()), false).await?;
    ensure!(
        rejected
            .errors
            .iter()
            .any(|error| error.contains("invalid-meta") && error.contains("1024")),
        "Selecting invalid metadata did not report its package and constraint"
    );
    ensure!(
        !installed.path().join("invalid-meta").exists(),
        "Invalid package was installed"
    );
    ensure!(
        std::fs::read_to_string(installed.path().join("valid/SKILL.md"))?
            .contains("Selected instructions"),
        "Rejected add changed the valid installation"
    );
    Ok(())
}

#[tokio::test]
async fn upstream_advance_check_missing_restore_and_removed_files() -> Result<()> {
    let source = tempfile::tempdir()?;
    let installed = tempfile::tempdir()?;
    git(source.path(), &["init", "--initial-branch=main"])?;
    package(source.path(), "example", "first")?;
    std::fs::write(source.path().join("example/obsolete.txt"), "old file")?;
    commit(source.path())?;
    let mut repository = config(source.path());
    repository.repo = format!("file://{}", source.path().display());
    let mut sync = AgentSkillsSync::new(installed.path()).await?;
    let added = sync.sync_all(std::slice::from_ref(&repository)).await?;
    ensure!(added.errors.is_empty(), "{:?}", added.errors);
    ensure!(
        added.added == vec!["example"],
        "expected example to be added"
    );
    let metadata_before = std::fs::read(installed.path().join(".agent-skills-sync.json"))?;
    package(source.path(), "example", "second")?;
    std::fs::remove_file(source.path().join("example/obsolete.txt"))?;
    commit(source.path())?;
    let checked = sync.check_all(std::slice::from_ref(&repository)).await?;
    ensure!(
        checked.updated == vec!["example"],
        "expected example update in check"
    );
    ensure!(
        installed.path().join("example/obsolete.txt").exists(),
        "check removed obsolete file"
    );
    ensure!(
        metadata_before == std::fs::read(installed.path().join(".agent-skills-sync.json"))?,
        "check changed sync metadata"
    );
    let updated = sync.sync_all(std::slice::from_ref(&repository)).await?;
    ensure!(updated.errors.is_empty(), "{:?}", updated.errors);
    ensure!(
        updated.updated == vec!["example"],
        "expected example to be updated"
    );
    ensure!(
        !installed.path().join("example/obsolete.txt").exists(),
        "obsolete file remains after sync"
    );
    ensure!(
        std::fs::read_to_string(installed.path().join("example/SKILL.md"))?.contains("second"),
        "updated package lacks second content"
    );
    std::fs::remove_dir_all(installed.path().join("example"))?;
    let restored = sync.sync_all(&[repository]).await?;
    ensure!(
        restored.added == vec!["example"],
        "expected missing example to be restored"
    );
    Ok(())
}

#[tokio::test]
async fn selection_to_all_and_alias_changes_keep_source_identity() -> Result<()> {
    let source = tempfile::tempdir()?;
    let installed = tempfile::tempdir()?;
    package(source.path(), "alpha", "alpha")?;
    package(source.path(), "beta", "beta")?;
    let mut repository = config(source.path());
    repository.skills = Some(vec!["alpha".to_owned()]);
    repository.alias = Some("before".to_owned());
    let mut sync = AgentSkillsSync::new(installed.path()).await?;
    ensure!(
        sync.sync_all(std::slice::from_ref(&repository))
            .await?
            .added
            == vec!["alpha"],
        "expected selected alpha to be added"
    );
    repository.skills = None;
    repository.alias = Some("after".to_owned());
    let result = sync.sync_all(std::slice::from_ref(&repository)).await?;
    ensure!(result.errors.is_empty(), "{:?}", result.errors);
    ensure!(result.added == vec!["beta"], "expected beta to be added");
    ensure!(
        installed.path().join("alpha").exists(),
        "alpha disappeared after alias change"
    );
    repository.skills = Some(vec!["beta".to_owned()]);
    ensure!(
        sync.sync_all(&[repository]).await?.removed == vec!["alpha"],
        "expected deselected alpha to be removed"
    );
    Ok(())
}

#[tokio::test]
async fn collisions_force_transfer_and_source_removal_are_ownership_safe() -> Result<()> {
    let first = tempfile::tempdir()?;
    let second = tempfile::tempdir()?;
    let installed = tempfile::tempdir()?;
    let first_root = first.path().join("same-repo-name");
    let second_root = second.path().join("same-repo-name");
    package(&first_root, "shared", "first owner")?;
    package(&second_root, "shared", "second owner")?;
    let first_config = config(&first_root);
    let second_config = config(&second_root);
    let mut sync = AgentSkillsSync::new(installed.path()).await?;
    sync.add(&first_config, false).await?;
    let collision = sync.add(&second_config, false).await?;
    ensure!(
        !collision.errors.is_empty(),
        "ownership collision was not reported"
    );
    ensure!(
        std::fs::read_to_string(installed.path().join("shared/SKILL.md"))?.contains("first owner"),
        "collision overwrote first owner's package"
    );
    let forced = sync.add(&second_config, true).await?;
    ensure!(forced.errors.is_empty(), "{:?}", forced.errors);
    let removed_old_owner = sync.sync_all(&[second_config]).await?;
    ensure!(
        removed_old_owner.errors.is_empty(),
        "{:?}",
        removed_old_owner.errors
    );
    ensure!(
        removed_old_owner.removed.is_empty(),
        "removing old owner removed transferred package"
    );
    ensure!(
        std::fs::read_to_string(installed.path().join("shared/SKILL.md"))?.contains("second owner"),
        "transferred package lacks second owner's content"
    );
    Ok(())
}

#[tokio::test]
async fn dirty_and_unowned_packages_survive_until_explicit_force() -> Result<()> {
    let source = tempfile::tempdir()?;
    let installed = tempfile::tempdir()?;
    package(source.path(), "example", "upstream")?;
    package(installed.path(), "example", "local")?;
    std::fs::write(installed.path().join("example/local.txt"), "keep")?;
    let repository = config(source.path());
    let mut sync = AgentSkillsSync::new(installed.path()).await?;
    ensure!(
        !sync.add(&repository, false).await?.errors.is_empty(),
        "unowned package conflict was not reported"
    );
    ensure!(
        installed.path().join("example/local.txt").exists(),
        "unforced add removed local file"
    );
    let forced = sync.add(&repository, true).await?;
    ensure!(
        forced.updated == vec!["example"],
        "expected force to update example"
    );
    ensure!(
        !installed.path().join("example/local.txt").exists(),
        "force preserved unowned local file"
    );
    std::fs::write(installed.path().join("example/user.txt"), "local changes")?;
    package(source.path(), "example", "new upstream")?;
    ensure!(
        !sync
            .sync_all(std::slice::from_ref(&repository))
            .await?
            .errors
            .is_empty(),
        "dirty package conflict was not reported"
    );
    ensure!(
        std::fs::read_to_string(installed.path().join("example/SKILL.md"))?.contains("upstream"),
        "dirty sync lost original upstream content"
    );
    ensure!(
        !std::fs::read_to_string(installed.path().join("example/SKILL.md"))?
            .contains("new upstream"),
        "dirty sync installed new upstream content"
    );
    ensure!(
        !sync.sync_all(&[]).await?.errors.is_empty(),
        "dirty removal conflict was not reported"
    );
    ensure!(
        installed.path().join("example/user.txt").exists(),
        "unforced removal deleted user file"
    );
    ensure!(
        sync.add(&repository, true).await?.errors.is_empty(),
        "forced add reported errors"
    );
    ensure!(
        !installed.path().join("example/user.txt").exists(),
        "forced add preserved user file"
    );
    ensure!(
        sync.sync_all(&[]).await?.removed == vec!["example"],
        "expected clean example removal"
    );
    Ok(())
}

#[tokio::test]
async fn root_package_and_branch_tag_commit_pins() -> Result<()> {
    let source = tempfile::tempdir()?;
    let installed = tempfile::tempdir()?;
    git(source.path(), &["init", "--initial-branch=main"])?;
    std::fs::write(
        source.path().join("SKILL.md"),
        "---\nname: root-package\ndescription: Root\n---\nfirst\n",
    )?;
    let initial = commit(source.path())?;
    git(source.path(), &["tag", "release-one"])?;
    std::fs::write(
        source.path().join("SKILL.md"),
        "---\nname: root-package\ndescription: Root\n---\nsecond\n",
    )?;
    commit(source.path())?;
    let mut repository = config(source.path());
    repository.git_ref = Some(initial);
    ensure!(
        AgentSkillsSync::discover_source(&repository).await? == vec!["root-package"],
        "expected root-package discovery"
    );
    let mut sync = AgentSkillsSync::new(installed.path()).await?;
    ensure!(
        sync.add(&repository, false).await?.errors.is_empty(),
        "pinned add reported errors"
    );
    ensure!(
        std::fs::read_to_string(installed.path().join("root-package/SKILL.md"))?.contains("first"),
        "commit pin did not install first content"
    );
    repository.git_ref = Some("main".to_owned());
    ensure!(
        sync.sync_all(std::slice::from_ref(&repository))
            .await?
            .updated
            == vec!["root-package"],
        "expected branch pin to update root-package"
    );
    ensure!(
        std::fs::read_to_string(installed.path().join("root-package/SKILL.md"))?.contains("second"),
        "branch pin did not install second content"
    );
    repository.git_ref = Some("release-one".to_owned());
    ensure!(
        sync.sync_all(&[repository]).await?.updated == vec!["root-package"],
        "expected tag pin to update root-package"
    );
    ensure!(
        std::fs::read_to_string(installed.path().join("root-package/SKILL.md"))?.contains("first"),
        "tag pin did not restore first content"
    );
    ensure!(
        !installed.path().join("root-package/.git").exists(),
        "installed package includes git metadata"
    );
    Ok(())
}

#[tokio::test]
async fn legacy_metadata_never_authorizes_deletion_or_overwrite() -> Result<()> {
    let source = tempfile::tempdir()?;
    let installed = tempfile::tempdir()?;
    package(source.path(), "example", "upstream")?;
    package(installed.path(), "example", "unverified legacy copy")?;
    let metadata = serde_json::json!({"old-alias": {
        "repo": source.path().to_string_lossy(), "git_ref": null, "commit_sha": "old",
        "synced_skills": ["example", "../outside"], "last_sync": "2024-01-02T03:04:05.120Z"
    }});
    std::fs::write(
        installed.path().join(".agent-skills-sync.json"),
        serde_json::to_vec(&metadata)?,
    )?;
    let mut sync = AgentSkillsSync::new(installed.path()).await?;
    ensure!(
        !sync
            .sync_all(&[config(source.path())])
            .await?
            .errors
            .is_empty(),
        "legacy metadata overwrite conflict was not reported"
    );
    ensure!(
        !sync.sync_all(&[]).await?.errors.is_empty(),
        "legacy metadata removal conflict was not reported"
    );
    ensure!(
        std::fs::read_to_string(installed.path().join("example/SKILL.md"))?.contains("legacy"),
        "legacy package content changed"
    );
    Ok(())
}

#[tokio::test]
async fn check_reports_cross_source_collisions_without_creating_root() -> Result<()> {
    let first = tempfile::tempdir()?;
    let second = tempfile::tempdir()?;
    let parent = tempfile::tempdir()?;
    let root = parent.path().join("not-created");
    package(first.path(), "shared", "first")?;
    package(second.path(), "shared", "second")?;
    let sync = AgentSkillsSync::new(&root).await?;
    let result = sync
        .check_all(&[config(first.path()), config(second.path())])
        .await?;
    ensure!(
        !result.errors.is_empty(),
        "cross-source collision was not reported"
    );
    ensure!(!root.exists(), "check created the installation root");
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn symlinks_cannot_escape_package_and_force_replaces_destination_link() -> Result<()> {
    use std::os::unix::fs::symlink;
    let source = tempfile::tempdir()?;
    let installed = tempfile::tempdir()?;
    let outside = tempfile::tempdir()?;
    package(source.path(), "example", "package")?;
    std::fs::write(outside.path().join("secret.txt"), "secret")?;
    symlink(outside.path(), source.path().join("example/escape"))?;
    let repository = config(source.path());
    let mut sync = AgentSkillsSync::new(installed.path()).await?;
    ensure!(
        !sync.add(&repository, false).await?.errors.is_empty(),
        "escaping package symlink was not rejected"
    );
    ensure!(
        !installed.path().join("example").exists(),
        "escaping package was installed"
    );
    std::fs::remove_file(source.path().join("example/escape"))?;
    symlink(outside.path(), installed.path().join("example"))?;
    ensure!(
        !sync.add(&repository, false).await?.errors.is_empty(),
        "destination symlink was not rejected"
    );
    ensure!(
        sync.add(&repository, true).await?.errors.is_empty(),
        "forced symlink replacement reported errors"
    );
    ensure!(
        !std::fs::symlink_metadata(installed.path().join("example"))?
            .file_type()
            .is_symlink(),
        "forced replacement left destination symlink"
    );
    ensure!(
        std::fs::read_to_string(outside.path().join("secret.txt"))? == "secret",
        "outside file changed during symlink replacement"
    );
    Ok(())
}

#[test]
fn equivalent_source_spellings_share_identity() -> Result<()> {
    ensure!(
        normalize_source("owner/repository")?
            == normalize_source("https://github.com/owner/repository.git/")?,
        "equivalent GitHub spellings have different identities"
    );
    let source = tempfile::tempdir()?;
    ensure!(
        normalize_source(&source.path().to_string_lossy())?
            == normalize_source(&format!("file://{}", source.path().display()))?,
        "local path and file URL have different identities"
    );
    Ok(())
}

#[test]
fn relative_sources_remain_stable_across_working_directories() -> Result<()> {
    let workspace = tempfile::tempdir()?;
    let source = workspace.path().join("source");
    let empty = workspace.path().join("empty");
    let unrelated = workspace.path().join("unrelated");
    let installed = workspace.path().join("installed");
    std::fs::create_dir_all(&empty)?;
    package(&source, "example", "original source")?;
    package(&unrelated, "unrelated", "must not import")?;
    for (directory, operation) in [(&source, "add"), (&empty, "sync"), (&unrelated, "sync")] {
        let output = Command::new(std::env::current_exe()?)
            .args(["--exact", "cwd_identity_worker", "--nocapture"])
            .current_dir(directory)
            .env("SKILLS_SYNC_CWD_OPERATION", operation)
            .env("SKILLS_SYNC_CWD_ROOT", &installed)
            .env(
                "SKILLS_SYNC_CWD_CONFIG",
                workspace.path().join("config.json"),
            )
            .output()?;
        ensure!(
            output.status.success(),
            "worker failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    ensure!(
        std::fs::read_to_string(installed.join("example/SKILL.md"))?.contains("original source"),
        "working directory change lost original source content"
    );
    ensure!(
        !installed.join("unrelated").exists(),
        "sync imported unrelated working directory"
    );
    let persisted: AgentSkillsRepoConfig =
        serde_json::from_slice(&std::fs::read(workspace.path().join("config.json"))?)?;
    ensure!(
        persisted.repo == canonical_source(&source.to_string_lossy())?,
        "persisted source is not canonical"
    );
    Ok(())
}

#[tokio::test]
async fn cwd_identity_worker() -> Result<()> {
    let Some(operation) = std::env::var_os("SKILLS_SYNC_CWD_OPERATION") else {
        return Ok(());
    };
    let root = std::env::var_os("SKILLS_SYNC_CWD_ROOT").context("worker root is missing")?;
    let config_path =
        std::env::var_os("SKILLS_SYNC_CWD_CONFIG").context("worker config is missing")?;
    let mut sync = AgentSkillsSync::new(&root).await?;
    let repository = if operation == "add" {
        let relative = config(Path::new("."));
        let result = sync.add(&relative, false).await?;
        ensure!(result.errors.is_empty(), "{:?}", result.errors);
        let mut repository = relative;
        repository.repo = canonical_source(&repository.repo)?;
        std::fs::write(&config_path, serde_json::to_vec(&repository)?)?;
        repository
    } else {
        let repository: AgentSkillsRepoConfig =
            serde_json::from_slice(&std::fs::read(&config_path)?)?;
        let result = sync.sync_all(std::slice::from_ref(&repository)).await?;
        ensure!(result.errors.is_empty(), "{:?}", result.errors);
        ensure!(
            result.removed.is_empty(),
            "working directory change removed source packages"
        );
        repository
    };
    let metadata: HashMap<String, SyncMetadata> = serde_json::from_slice(&std::fs::read(
        Path::new(&root).join(".agent-skills-sync.json"),
    )?)?;
    ensure!(metadata.len() == 1, "expected one source metadata record");
    ensure!(
        metadata
            .get(&normalize_source(&repository.repo)?)
            .context("source metadata is missing")?
            .repo
            == repository.repo,
        "metadata source differs from configured source"
    );
    Ok(())
}

#[tokio::test]
async fn ambiguous_relative_metadata_preserves_ownership_and_files() -> Result<()> {
    for legacy_source in [".", "owner/repository"] {
        let source = tempfile::tempdir()?;
        let installed = tempfile::tempdir()?;
        package(source.path(), "example", "owned package")?;
        let repository = config(source.path());
        let mut sync = AgentSkillsSync::new(installed.path()).await?;
        ensure!(
            sync.add(&repository, false).await?.errors.is_empty(),
            "initial owned package add reported errors"
        );
        let metadata_path = installed.path().join(".agent-skills-sync.json");
        let mut metadata: HashMap<String, SyncMetadata> =
            serde_json::from_slice(&std::fs::read(&metadata_path)?)?;
        metadata
            .values_mut()
            .next()
            .context("source metadata is missing")?
            .repo = legacy_source.to_owned();
        let legacy_bytes = serde_json::to_vec(&metadata)?;
        std::fs::write(&metadata_path, &legacy_bytes)?;
        std::fs::remove_dir_all(source.path().join("example"))?;

        let mut sync = AgentSkillsSync::new(installed.path()).await?;
        let active = sync.sync_all(&[repository]).await?;
        ensure!(
            !active.errors.is_empty(),
            "ambiguous active metadata was not rejected"
        );
        ensure!(
            active.removed.is_empty(),
            "ambiguous active metadata authorized package removal"
        );
        let retired = sync.sync_all(&[]).await?;
        ensure!(
            !retired.errors.is_empty(),
            "ambiguous retired metadata was not rejected"
        );
        ensure!(
            retired.removed.is_empty(),
            "ambiguous retired metadata authorized package removal"
        );
        ensure!(
            installed.path().join("example/SKILL.md").exists(),
            "ambiguous metadata caused package deletion"
        );
        ensure!(
            std::fs::read(&metadata_path)? == legacy_bytes,
            "ambiguous metadata was changed"
        );
    }
    Ok(())
}

#[tokio::test]
async fn canonical_file_urls_keep_checkout_and_ref_semantics() -> Result<()> {
    let source = tempfile::tempdir()?;
    let working_install = tempfile::tempdir()?;
    let checkout_install = tempfile::tempdir()?;
    git(source.path(), &["init", "--initial-branch=main"])?;
    package(source.path(), "example", "reviewed commit")?;
    let reviewed = commit(source.path())?;
    package(source.path(), "example", "uncommitted working tree")?;

    let mut repository = config(source.path());
    repository.repo = canonical_source(&repository.repo)?;
    let mut working = AgentSkillsSync::new(working_install.path()).await?;
    ensure!(
        working.add(&repository, false).await?.errors.is_empty(),
        "working-tree add reported errors"
    );
    ensure!(
        std::fs::read_to_string(working_install.path().join("example/SKILL.md"))?
            .contains("uncommitted working tree"),
        "local path did not install uncommitted working tree"
    );

    repository.repo = canonical_source(&format!("file://{}", source.path().display()))?;
    ensure!(
        repository.repo.starts_with("file://"),
        "canonical file URL lost its scheme"
    );
    repository.git_ref = Some(reviewed.clone());
    let mut checkout = AgentSkillsSync::new(checkout_install.path()).await?;
    ensure!(
        checkout.add(&repository, false).await?.errors.is_empty(),
        "pinned checkout add reported errors"
    );
    ensure!(
        std::fs::read_to_string(checkout_install.path().join("example/SKILL.md"))?
            .contains("reviewed commit"),
        "pinned file URL did not install reviewed commit"
    );
    let metadata: HashMap<String, SyncMetadata> = serde_json::from_slice(&std::fs::read(
        checkout_install.path().join(".agent-skills-sync.json"),
    )?)?;
    let record = metadata
        .values()
        .next()
        .context("source metadata is missing")?;
    ensure!(
        record.repo == repository.repo,
        "metadata repository differs from canonical file URL"
    );
    ensure!(
        record.git_ref.as_deref() == Some(reviewed.as_str()),
        "metadata ref differs from reviewed commit"
    );
    ensure!(
        record.commit_sha == reviewed,
        "metadata commit SHA differs from reviewed commit"
    );

    repository.git_ref = None;
    ensure!(
        checkout.sync_all(&[repository]).await?.errors.is_empty(),
        "unpinned file URL sync reported errors"
    );
    ensure!(
        std::fs::read_to_string(checkout_install.path().join("example/SKILL.md"))?
            .contains("reviewed commit"),
        "unpinned file URL lost reviewed checkout content"
    );
    ensure!(
        canonical_source("owner/repository")? == "https://github.com/owner/repository",
        "GitHub shorthand did not canonicalize to HTTPS URL"
    );
    Ok(())
}

#[tokio::test]
async fn add_is_atomic_when_any_selected_package_conflicts() -> Result<()> {
    let source = tempfile::tempdir()?;
    let installed = tempfile::tempdir()?;
    package(source.path(), "alpha", "new package")?;
    package(source.path(), "beta", "upstream")?;
    package(installed.path(), "beta", "unowned local package")?;
    let mut sync = AgentSkillsSync::new(installed.path()).await?;
    let report = sync.add(&config(source.path()), false).await?;
    ensure!(
        !report.errors.is_empty(),
        "selected package conflict was not reported"
    );
    ensure!(
        report.added.is_empty(),
        "conflicting add installed packages"
    );
    ensure!(
        !installed.path().join("alpha").exists(),
        "conflicting add installed alpha"
    );
    ensure!(
        !installed.path().join(".agent-skills-sync.json").exists(),
        "conflicting add created metadata"
    );
    ensure!(
        std::fs::read_to_string(installed.path().join("beta/SKILL.md"))?.contains("unowned local"),
        "conflicting add overwrote unowned beta"
    );
    Ok(())
}

#[tokio::test]
async fn discovery_stops_at_packages_and_rejects_excessive_depth() -> Result<()> {
    let source = tempfile::tempdir()?;
    package(source.path(), "outer", "outer package")?;
    package(
        &source.path().join("outer"),
        "inner",
        "part of outer package",
    )?;
    ensure!(
        AgentSkillsSync::discover_source(&config(source.path())).await? == vec!["outer"],
        "discovery did not stop at outer package"
    );
    let deep = tempfile::tempdir()?;
    let mut directory = deep.path().to_path_buf();
    for _ in 0..34 {
        directory = directory.join("nested");
    }
    package(&directory, "example", "too deep")?;
    ensure!(
        AgentSkillsSync::discover_source(&config(deep.path()))
            .await
            .is_err(),
        "discovery accepted excessive package depth"
    );
    Ok(())
}
