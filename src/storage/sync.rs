//! Shared, ownership-aware import and synchronization of Agent Skills sources.

use crate::storage::agent_skills::AgentSkill;
use anyhow::{bail, ensure, Context, Result};
use cap_std::{ambient_authority, fs::Dir};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use tempfile::{NamedTempFile, TempDir};
use tokio::process::Command;

const MAX_DEPTH: usize = 32;
const MAX_ENTRIES: usize = 100_000;
const METADATA_FILE: &str = ".agent-skills-sync.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSkillsRepoConfig {
    /// Local directory, Git URL, or GitHub owner/repository shorthand.
    pub repo: String,
    /// Branch, tag, or full commit. Local directories without a ref use their working tree.
    #[serde(default)]
    pub git_ref: Option<String>,
    /// None imports every package; an empty list imports none.
    #[serde(default)]
    pub skills: Option<Vec<String>>,
    /// Display label only; ownership does not depend on this label.
    #[serde(default)]
    pub alias: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncMetadata {
    pub repo: String,
    pub git_ref: Option<String>,
    pub commit_sha: String,
    pub synced_skills: Vec<String>,
    pub last_sync: jiff::Timestamp,
    /// Absent in legacy metadata: never infer ownership of existing files.
    #[serde(default)]
    pub fingerprints: BTreeMap<String, String>,
    #[serde(default)]
    pub selection: Option<Vec<String>>,
}

pub struct AgentSkillsSync {
    skills_root: PathBuf,
    metadata_path: PathBuf,
    metadata: HashMap<String, SyncMetadata>,
}

struct Source {
    identity: String,
    location: String,
    local: Option<PathBuf>,
}

struct Prepared {
    _checkout: TempDir,
    revision: String,
    packages: BTreeMap<String, Dir>,
}

struct Change {
    name: String,
    staged: Option<PathBuf>,
}

struct TrackingWrite<'a> {
    contents: &'a [u8],
    path: PathBuf,
    replace: bool,
}

impl<'a> TrackingWrite<'a> {
    fn preflight(path: &Path, contents: &'a [u8]) -> Result<Self> {
        let replace = match std::fs::symlink_metadata(path) {
            Ok(_) => true,
            Err(error) if error.kind() == ErrorKind::NotFound => false,
            Err(error) => return Err(error.into()),
        };
        let path = if replace {
            // Save through a config symlink rather than replacing the link itself.
            path.canonicalize()?
        } else {
            let name = path
                .file_name()
                .context("Tracking config has no file name")?;
            let parent = path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new("."));
            std::fs::create_dir_all(parent)?;
            parent.canonicalize()?.join(name)
        };
        if replace {
            Self::writable_permissions(&path)?;
        }
        Ok(Self {
            contents,
            path,
            replace,
        })
    }

    fn stage(&self) -> Result<NamedTempFile> {
        let parent = self
            .path
            .parent()
            .context("Tracking config has no parent")?;
        let mut file = NamedTempFile::new_in(parent)?;
        file.write_all(self.contents)?;
        file.as_file().sync_all()?;
        Ok(file)
    }

    fn writable_permissions(path: &Path) -> Result<std::fs::Permissions> {
        let metadata = std::fs::symlink_metadata(path)?;
        ensure!(metadata.is_file(), "Tracking config is not a regular file");
        let permissions = metadata.permissions();
        ensure!(!permissions.readonly(), "Tracking config is read-only");
        // Renaming only needs directory access; also require permission to write the file.
        std::fs::OpenOptions::new().write(true).open(path)?;
        Ok(permissions)
    }

    fn persist(self, file: NamedTempFile) -> Result<()> {
        let result = (|| -> Result<()> {
            if self.replace {
                file.as_file()
                    .set_permissions(Self::writable_permissions(&self.path)?)?;
                file.persist(&self.path)?;
            } else {
                file.persist_noclobber(&self.path)?;
            }
            Ok(())
        })();
        result.with_context(|| format!("Cannot write tracking config {}", self.path.display()))
    }
}

#[derive(Clone, Copy)]
enum ImportMode {
    Add { force: bool },
    Sync,
    Check,
}

impl AgentSkillsSync {
    /// Load ownership records. Invalid metadata is an error, not permission to overwrite.
    ///
    /// # Errors
    /// Returns filesystem, metadata, or source-normalization errors.
    pub async fn new(skills_root: impl AsRef<Path>) -> Result<Self> {
        let skills_root = skills_root.as_ref().to_path_buf();
        let metadata_path = skills_root.join(METADATA_FILE);
        let metadata: HashMap<String, SyncMetadata> = match tokio::fs::read(&metadata_path).await {
            Ok(bytes) => {
                serde_json::from_slice(&bytes).context("Invalid sync ownership metadata")?
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => HashMap::new(),
            Err(error) => return Err(error.into()),
        };
        let mut migrated: HashMap<String, SyncMetadata> = HashMap::new();
        for (old_key, mut record) in metadata {
            // Relative legacy spellings do not identify their original working directory.
            let key = metadata_source(&record.repo).map_or(old_key, |value| value.identity);
            record.synced_skills.retain(|name| valid_name(name));
            record.fingerprints.retain(|name, _| valid_name(name));
            if let Some(existing) = migrated.get_mut(&key) {
                existing.synced_skills.extend(record.synced_skills);
                existing.synced_skills.sort();
                existing.synced_skills.dedup();
                // Ambiguous old aliases are not proof of ownership.
                existing.fingerprints.clear();
            } else {
                migrated.insert(key, record);
            }
        }
        Ok(Self {
            skills_root,
            metadata_path,
            metadata: migrated,
        })
    }

    /// Import selected packages without retiring previously installed packages from the source.
    /// Only explicit force may replace another source's package, an unowned package, or local
    /// edits. Any package error aborts the entire add without changing installs or ownership.
    ///
    /// # Errors
    /// Returns source-fetch, staging, transaction, or metadata errors. Package conflicts
    /// and invalid packages are returned in the report.
    pub async fn add(&mut self, config: &AgentSkillsRepoConfig, force: bool) -> Result<SyncReport> {
        self.import(config, ImportMode::Add { force }, None).await
    }

    /// Import packages and atomically replace the tracking file only after the install
    /// and ownership updates succeed. A tracking write failure rolls both back.
    /// This does not isolate concurrent writers or guarantee recovery after a process crash.
    ///
    /// # Errors
    /// Returns the same errors as [`Self::add`], plus errors staging or persisting tracking.
    /// If rollback itself fails, the error identifies the retained backups.
    pub async fn add_with_tracking(
        &mut self,
        config: &AgentSkillsRepoConfig,
        force: bool,
        tracking_path: &Path,
        tracking_contents: &[u8],
    ) -> Result<SyncReport> {
        let tracking =
            TrackingWrite::preflight(tracking_path, tracking_contents).with_context(|| {
                format!("Cannot prepare tracking config {}", tracking_path.display())
            })?;
        self.import(config, ImportMode::Add { force }, Some(tracking))
            .await
    }

    /// Fetch a source and list candidate names without validating or installing packages.
    ///
    /// # Errors
    /// Returns fetching, discovery, malformed frontmatter, or invalid name errors.
    pub async fn discover_source(config: &AgentSkillsRepoConfig) -> Result<Vec<String>> {
        let prepared = prepare(config).await?;
        Ok(prepared.packages.into_keys().collect())
    }

    /// Refresh upstream state and reconcile configured sources; never overwrite local edits.
    ///
    /// # Errors
    /// Returns metadata transaction errors. Per-source errors appear in the report.
    pub async fn sync_all(&mut self, repos: &[AgentSkillsRepoConfig]) -> Result<SyncReport> {
        self.reconcile(repos, false).await
    }

    /// Fetch upstream state and report proposed changes, without changing installs or metadata.
    /// Pins are resolved exactly like sync; changing `git_ref` then checking previews an update.
    ///
    /// # Errors
    /// Returns filesystem errors. Per-source errors appear in the report.
    pub async fn check_all(&self, repos: &[AgentSkillsRepoConfig]) -> Result<SyncReport> {
        let mut preview = Self {
            skills_root: self.skills_root.clone(),
            metadata_path: self.metadata_path.clone(),
            metadata: self.metadata.clone(),
        };
        preview.reconcile(repos, true).await
    }

    async fn reconcile(
        &mut self,
        repos: &[AgentSkillsRepoConfig],
        check: bool,
    ) -> Result<SyncReport> {
        let mut report = SyncReport::default();
        let mut active = HashSet::new();
        // A malformed config must not make a previously managed source look retired.
        for config in repos {
            active.insert(source(&config.repo)?.identity);
        }
        let removed: Vec<_> = self
            .metadata
            .keys()
            .filter(|key| !active.contains(*key))
            .cloned()
            .collect();
        let mut seen = HashSet::new();
        for config in repos {
            let identity = source(&config.repo)?.identity;
            if !seen.insert(identity) {
                report
                    .errors
                    .push(format!("{}: source configured more than once", config.repo));
                continue;
            }
            let mode = if check {
                ImportMode::Check
            } else {
                ImportMode::Sync
            };
            match self.import(config, mode, None).await {
                Ok(result) => report.merge(result),
                Err(error) => report.errors.push(format!("{}: {error:#}", config.repo)),
            }
        }
        for key in removed {
            report.merge(self.retire(&key, check).await?);
        }
        Ok(report)
    }

    async fn retire(&mut self, key: &str, check: bool) -> Result<SyncReport> {
        let mut report = SyncReport::default();
        let Some(record) = self.metadata.get(key).cloned() else {
            return Ok(report);
        };
        if let Err(error) = metadata_source(&record.repo) {
            report.errors.push(format!(
                "{}: source identity is unverified; packages preserved: {error:#}",
                record.repo
            ));
            return Ok(report);
        }
        let mut next = record.clone();
        let mut changes = Vec::new();
        for name in &record.synced_skills {
            match self.removable(key, name).await {
                Ok(exists) => {
                    if exists {
                        changes.push(Change {
                            name: name.clone(),
                            staged: None,
                        });
                        report.removed.push(name.clone());
                    }
                    next.synced_skills.retain(|value| value != name);
                    next.fingerprints.remove(name);
                }
                Err(error) => report.errors.push(format!("{name}: {error:#}")),
            }
        }
        let mut metadata = self.metadata.clone();
        if next.synced_skills.is_empty() {
            metadata.remove(key);
        } else {
            metadata.insert(key.to_owned(), next);
        }
        if check {
            self.metadata = metadata;
        } else {
            self.commit(changes, metadata, None).await?;
        }
        Ok(report)
    }

    #[expect(
        clippy::too_many_lines,
        reason = "source import keeps validation, ownership planning, and commit in one transaction"
    )]
    async fn import(
        &mut self,
        config: &AgentSkillsRepoConfig,
        mode: ImportMode,
        tracking: Option<TrackingWrite<'_>>,
    ) -> Result<SyncReport> {
        let force = matches!(mode, ImportMode::Add { force: true });
        let check = matches!(mode, ImportMode::Check);
        let additive = matches!(mode, ImportMode::Add { .. });
        let source = source(&config.repo)?;
        let identity = source.identity;
        if !force {
            if let Some(record) = self.metadata.get(&identity) {
                metadata_source(&record.repo)
                    .context("Stored source identity is unverified; use add --force to replace")?;
            }
        }
        // Always fetch before deciding whether an upstream package changed.
        let prepared = prepare(config).await?;
        let staging = if check {
            tempfile::tempdir()?
        } else {
            tokio::fs::create_dir_all(&self.skills_root).await?;
            tempfile::tempdir_in(&self.skills_root)?
        };
        let mut report = SyncReport::default();
        let mut next = self
            .metadata
            .get(&identity)
            .cloned()
            .unwrap_or_else(|| SyncMetadata {
                repo: String::new(),
                git_ref: None,
                commit_sha: String::new(),
                synced_skills: Vec::new(),
                last_sync: jiff::Timestamp::now(),
                fingerprints: BTreeMap::new(),
                selection: Some(Vec::new()),
            });
        let selected: BTreeMap<_, _> = prepared
            .packages
            .iter()
            .filter(|(name, _)| {
                config
                    .skills
                    .as_ref()
                    .is_none_or(|filter| filter.contains(name))
            })
            .collect();
        if let Some(filter) = &config.skills {
            for name in filter {
                if !prepared.packages.contains_key(name) {
                    report
                        .errors
                        .push(format!("{name}: requested package not found"));
                }
            }
        }
        let mut changes = Vec::new();
        let mut successful = HashSet::new();
        for (name, package) in selected {
            let staged = staging.path().join(name);
            let result: Result<(String, bool)> = async {
                let fingerprint = stage_package(package, &staged).await?;
                let destination = self.skills_root.join(name);
                let exists = path_exists(&destination).await?;
                let actual = if exists {
                    Some(tree_hash_async(destination).await)
                } else {
                    None
                };
                if !force {
                    if self.other_owner(&identity, name) {
                        bail!("package is owned by another source; use add --force to replace");
                    }
                    if let Some(actual) = &actual {
                        let expected = next.fingerprints.get(name).context(
                            "existing package has no ownership baseline; use add --force",
                        )?;
                        if actual
                            .as_ref()
                            .map_err(|error| anyhow::anyhow!("{error:#}"))?
                            != expected
                        {
                            bail!("local modifications preserved; use add --force to replace");
                        }
                    }
                }
                let unchanged = !force
                    && actual.is_some_and(|value| value.is_ok_and(|value| value == fingerprint));
                Ok((fingerprint, !unchanged))
            }
            .await;
            match result {
                Ok((fingerprint, needs_replacement)) => {
                    if needs_replacement {
                        if path_exists(&self.skills_root.join(name)).await? {
                            report.updated.push(name.clone());
                        } else {
                            report.added.push(name.clone());
                        }
                        changes.push(Change {
                            name: name.clone(),
                            staged: Some(staged),
                        });
                    }
                    next.fingerprints.insert(name.clone(), fingerprint);
                    if !next.synced_skills.contains(name) {
                        next.synced_skills.push(name.clone());
                    }
                    successful.insert(name.clone());
                }
                Err(error) => report.errors.push(format!("{name}: {error:#}")),
            }
        }
        let obsolete: Vec<_> = next
            .synced_skills
            .iter()
            .filter(|name| {
                !additive
                    && (!prepared.packages.contains_key(*name)
                        || config
                            .skills
                            .as_ref()
                            .is_some_and(|filter| !filter.contains(*name)))
            })
            .cloned()
            .collect();
        for name in obsolete {
            match self.removable(&identity, &name).await {
                Ok(exists) => {
                    if exists {
                        changes.push(Change {
                            name: name.clone(),
                            staged: None,
                        });
                        report.removed.push(name.clone());
                    }
                    next.synced_skills.retain(|value| value != &name);
                    next.fingerprints.remove(&name);
                }
                Err(error) => report.errors.push(format!("{name}: {error:#}")),
            }
        }
        if additive && !report.errors.is_empty() {
            report.added.clear();
            report.updated.clear();
            report.removed.clear();
            return Ok(report);
        }
        next.repo = source.location;
        next.git_ref.clone_from(&config.git_ref);
        if additive && config.skills.is_some() {
            if let (Some(previous), Some(selected)) = (&mut next.selection, &config.skills) {
                previous.extend(selected.iter().cloned());
                previous.sort_unstable();
                previous.dedup();
            }
        } else {
            next.selection.clone_from(&config.skills);
        }
        next.commit_sha = prepared.revision;
        next.last_sync = jiff::Timestamp::now();
        next.synced_skills.sort();
        let mut metadata = self.metadata.clone();
        if force {
            for (key, record) in &mut metadata {
                if key != &identity {
                    record
                        .synced_skills
                        .retain(|name| !successful.contains(name));
                    record
                        .fingerprints
                        .retain(|name, _| !successful.contains(name));
                }
            }
        }
        metadata.insert(identity, next);
        if check {
            self.metadata = metadata;
        } else {
            self.commit(changes, metadata, tracking).await?;
        }
        Ok(report)
    }

    fn other_owner(&self, identity: &str, name: &str) -> bool {
        self.metadata.iter().any(|(key, record)| {
            key != identity && record.synced_skills.iter().any(|value| value == name)
        })
    }

    async fn removable(&self, identity: &str, name: &str) -> Result<bool> {
        let path = self.skills_root.join(name);
        if !path_exists(&path).await? {
            return Ok(false);
        }
        if self.other_owner(identity, name) {
            bail!("ambiguous source ownership; package preserved");
        }
        let record = self
            .metadata
            .get(identity)
            .context("Source ownership record is missing")?;
        metadata_source(&record.repo).context("Unverified source identity; package preserved")?;
        let baseline = record
            .fingerprints
            .get(name)
            .context("legacy/unowned package preserved (no ownership baseline)")?;
        if &tree_hash_async(path).await? != baseline {
            bail!("locally modified package preserved");
        }
        Ok(true)
    }

    async fn commit(
        &mut self,
        changes: Vec<Change>,
        metadata: HashMap<String, SyncMetadata>,
        tracking: Option<TrackingWrite<'_>>,
    ) -> Result<()> {
        tokio::fs::create_dir_all(&self.skills_root).await?;
        let tracking = if let Some(tracking) = tracking {
            let root = self.skills_root.canonicalize()?;
            ensure!(
                tracking.path != root.join(METADATA_FILE)
                    && !changes
                        .iter()
                        .any(|change| tracking.path.starts_with(root.join(&change.name))),
                "Tracking config overlaps files changed by the import"
            );
            let file = tracking.stage().with_context(|| {
                format!("Cannot stage tracking config {}", tracking.path.display())
            })?;
            Some((tracking, file))
        } else {
            None
        };
        let temporary = NamedTempFile::new_in(&self.skills_root)?;
        tokio::fs::write(temporary.path(), serde_json::to_vec_pretty(&metadata)?).await?;
        temporary.as_file().sync_all()?;
        let backup = tempfile::tempdir_in(&self.skills_root)?;
        let metadata_backup = backup.path().join(METADATA_FILE);
        let mut backed_up_metadata = false;
        let mut installed_metadata = false;
        let mut applied: Vec<(String, bool, bool)> = Vec::new();
        let result: Result<()> = async {
            for change in changes {
                let destination = self.skills_root.join(&change.name);
                let existed = path_exists(&destination).await?;
                if existed {
                    tokio::fs::rename(&destination, backup.path().join(&change.name)).await?;
                }
                applied.push((change.name, existed, false));
                if let Some(staged) = change.staged {
                    tokio::fs::rename(staged, &destination).await?;
                    if let Some(last) = applied.last_mut() {
                        last.2 = true;
                    }
                }
            }
            if tracking.is_some() && path_exists(&self.metadata_path).await? {
                tokio::fs::rename(&self.metadata_path, &metadata_backup).await?;
                backed_up_metadata = true;
            }
            tokio::fs::rename(temporary.path(), &self.metadata_path).await?;
            installed_metadata = true;
            if let Some((tracking, file)) = tracking {
                tracking.persist(file)?;
            }
            Ok(())
        }
        .await;
        if let Err(error) = result {
            let mut failures = Vec::new();
            let restore_metadata = if backed_up_metadata {
                tokio::fs::rename(metadata_backup, &self.metadata_path).await
            } else if installed_metadata {
                tokio::fs::remove_file(&self.metadata_path).await
            } else {
                Ok(())
            };
            if let Err(error) = restore_metadata {
                failures.push(format!("ownership metadata: {error}"));
            }
            for (name, existed, installed) in applied.into_iter().rev() {
                let destination = self.skills_root.join(&name);
                let result = async {
                    if installed {
                        tokio::fs::remove_dir_all(&destination).await?;
                    }
                    if existed {
                        tokio::fs::rename(backup.path().join(&name), destination).await?;
                    }
                    Ok::<_, std::io::Error>(())
                }
                .await;
                if let Err(error) = result {
                    failures.push(format!("{name}: {error}"));
                }
            }
            if !failures.is_empty() {
                let recovery = backup.keep();
                bail!(
                    "{error:#}; rollback failed: {}; backups retained at {}",
                    failures.join("; "),
                    recovery.display()
                );
            }
            return Err(error);
        }
        self.metadata = metadata;
        Ok(())
    }
}

/// Return the stable source identity used for config deduplication and ownership.
/// Aliases, selected packages, and Git refs do not change this identity.
///
/// # Errors
/// Returns an error for invalid sources or unavailable local paths.
pub fn normalize_source(raw: &str) -> Result<String> {
    Ok(source(raw)?.identity)
}

/// Return a persistent source spelling independent of the current working directory.
/// File URLs remain Git sources rather than becoming working-tree directories.
///
/// # Errors
/// Returns an error for invalid sources or unavailable local paths.
pub fn canonical_source(raw: &str) -> Result<String> {
    Ok(source(raw)?.location)
}

fn metadata_source(raw: &str) -> Result<Source> {
    if !Path::new(raw).is_absolute() && !raw.starts_with("file://") && !is_git_source(raw) {
        bail!("relative legacy source has no recorded base directory");
    }
    source(raw)
}

fn is_git_source(raw: &str) -> bool {
    ["https://", "http://", "ssh://", "git://"]
        .iter()
        .any(|prefix| raw.starts_with(prefix))
        || (raw.contains('@') && raw.contains(':') && !raw.contains("://"))
}

fn source(raw: &str) -> Result<Source> {
    let raw = raw.trim();
    if raw.is_empty() {
        bail!("empty source");
    }
    let direct = Path::new(raw);
    let file_path = raw
        .strip_prefix("file://")
        .map(|_| {
            reqwest::Url::parse(raw)
                .context("Invalid file URL")?
                .to_file_path()
                .map_err(|()| anyhow::anyhow!("File URL must be local"))
        })
        .transpose()?;
    let (location, local, canonical) = if let Some(path) = file_path {
        let path = std::fs::canonicalize(path)?;
        (
            reqwest::Url::from_file_path(&path)
                .map_err(|()| anyhow::anyhow!("File URL must be absolute"))?
                .to_string(),
            None,
            format!("path:{}", path.display()),
        )
    } else if direct.exists()
        || raw.starts_with('.')
        || direct.is_absolute()
        || raw.starts_with('~')
    {
        let expanded = if let Some(rest) = raw.strip_prefix("~/") {
            PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?).join(rest)
        } else {
            direct.to_path_buf()
        };
        let path = std::fs::canonicalize(expanded).context("Local source not found")?;
        if !path.is_dir() {
            bail!("Local source must be a directory");
        }
        (
            path.to_string_lossy().into_owned(),
            Some(path.clone()),
            format!("path:{}", path.display()),
        )
    } else if is_git_source(raw) {
        (
            raw.to_owned(),
            None,
            format!("git:{}", raw.trim_end_matches('/').trim_end_matches(".git")),
        )
    } else {
        let parts: Vec<_> = raw.split('/').collect();
        if parts.len() != 2
            || parts.iter().any(|part| {
                part.is_empty()
                    || !part
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
            })
        {
            bail!("Expected a local path, Git URL, or owner/repository shorthand");
        }
        let url = format!("https://github.com/{raw}");
        (
            url.clone(),
            None,
            format!("git:{}", url.trim_end_matches(".git")),
        )
    };
    Ok(Source {
        identity: blake3::hash(canonical.as_bytes()).to_hex().to_string(),
        location,
        local,
    })
}

async fn git(path: Option<&Path>, args: &[&str]) -> Result<String> {
    let mut command = Command::new("git");
    command
        .args(["-c", "core.hooksPath=/dev/null"])
        .args(args)
        .kill_on_drop(true);
    if let Some(path) = path {
        command.current_dir(path);
    }
    let output = command.output().await.context("Running git")?;
    if !output.status.success() {
        bail!(
            "git failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

async fn prepare(config: &AgentSkillsRepoConfig) -> Result<Prepared> {
    let source = source(&config.repo)?;
    let checkout = tempfile::tempdir()?;
    let (root, revision) = if let Some(path) = source.local.filter(|_| config.git_ref.is_none()) {
        (path, String::new())
    } else {
        let root = checkout.path().join("repository");
        git(
            None,
            &[
                "clone",
                "--no-checkout",
                "--",
                &source.location,
                &root.to_string_lossy(),
            ],
        )
        .await?;
        let revision = if let Some(reference) = &config.git_ref {
            let branch = reference.strip_prefix("refs/heads/").unwrap_or(reference);
            let candidates = [
                format!("refs/remotes/origin/{branch}^{{commit}}"),
                format!("refs/tags/{reference}^{{commit}}"),
                format!("{reference}^{{commit}}"),
            ];
            let mut resolved = None;
            for candidate in &candidates {
                if let Ok(value) = git(
                    Some(&root),
                    &["rev-parse", "--verify", "--end-of-options", candidate],
                )
                .await
                {
                    resolved = Some(value);
                    break;
                }
            }
            resolved.context("Requested branch, tag, or commit was not found")?
        } else {
            git(Some(&root), &["rev-parse", "HEAD"]).await?
        };
        git(Some(&root), &["checkout", "--detach", &revision, "--"]).await?;
        (root, revision)
    };
    let discovery_root = Dir::open_ambient_dir(root, ambient_authority())?;
    let packages = tokio::task::spawn_blocking(move || discover(discovery_root)).await??;
    Ok(Prepared {
        _checkout: checkout,
        revision,
        packages,
    })
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('-')
        && !name.ends_with('-')
        && !name.contains("--")
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn discover(root: Dir) -> Result<BTreeMap<String, Dir>> {
    let mut pending = vec![(root, 0_usize)];
    let mut count = 0_usize;
    let mut packages = BTreeMap::new();
    while let Some((directory, depth)) = pending.pop() {
        if depth > MAX_DEPTH {
            bail!("Source discovery exceeds depth limit {MAX_DEPTH}");
        }
        match directory.symlink_metadata("SKILL.md") {
            Ok(metadata) => {
                if !metadata.is_file() || metadata.file_type().is_symlink() {
                    bail!("SKILL.md must be a regular file");
                }
                let (mut input, _) =
                    open_regular_file(&directory, std::ffi::OsStr::new("SKILL.md"))?;
                let mut content = String::new();
                input.read_to_string(&mut content)?;
                let name = AgentSkill::source_name(&content)?;
                match packages.entry(name) {
                    std::collections::btree_map::Entry::Vacant(entry) => {
                        entry.insert(directory);
                    }
                    std::collections::btree_map::Entry::Occupied(entry) => {
                        bail!("Duplicate package name: {}", entry.key());
                    }
                }
                continue;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        for entry in directory.entries()? {
            let entry = entry?;
            count = count.saturating_add(1);
            if count > MAX_ENTRIES {
                bail!("Source discovery exceeds entry limit {MAX_ENTRIES}");
            }
            let name = entry.file_name();
            if name == ".git" {
                continue;
            }
            let kind = entry.file_type()?;
            if kind.is_symlink() {
                bail!("Source contains a symlink: {}", Path::new(&name).display());
            }
            if kind.is_dir() {
                pending.push((directory.open_dir(name)?, depth.saturating_add(1)));
            }
        }
    }
    Ok(packages)
}

async fn stage_package(source: &Dir, destination: &Path) -> Result<String> {
    let source = source.try_clone()?;
    let destination_copy = destination.to_path_buf();
    tokio::task::spawn_blocking(move || copy_tree(source, &destination_copy)).await??;
    AgentSkill::from_directory(destination)
        .await
        .with_context(|| format!("Invalid package {}", destination.display()))?;
    tree_hash_async(destination.to_path_buf()).await
}

fn copy_tree(source: Dir, destination: &Path) -> Result<()> {
    let mut pending = vec![(source, destination.to_path_buf(), 0_usize)];
    let mut count = 0_usize;
    while let Some((source, destination, depth)) = pending.pop() {
        if depth > MAX_DEPTH {
            bail!("Package exceeds depth limit {MAX_DEPTH}");
        }
        std::fs::create_dir(&destination)?;
        for entry in source.entries()? {
            let entry = entry?;
            count = count.saturating_add(1);
            if count > MAX_ENTRIES {
                bail!("Package exceeds entry limit {MAX_ENTRIES}");
            }
            let name = entry.file_name();
            if name == ".git" {
                continue;
            }
            let kind = entry.file_type()?;
            if kind.is_symlink() {
                bail!("Package symlink rejected: {}", Path::new(&name).display());
            }
            let target = destination.join(&name);
            if kind.is_dir() {
                pending.push((source.open_dir(name)?, target, depth.saturating_add(1)));
            } else if kind.is_file() {
                let (input, metadata) = open_regular_file(&source, &name)?;
                copy_regular_file(input, &metadata, &target)?;
            } else {
                bail!("Package contains a non-regular file");
            }
        }
    }
    Ok(())
}

fn open_regular_file(
    directory: &Dir,
    name: &std::ffi::OsStr,
) -> Result<(std::fs::File, std::fs::Metadata)> {
    let mut options = cap_std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    let input = directory.open_with(name, &options)?.into_std();
    let metadata = input.metadata()?;
    if !metadata.is_file() {
        bail!("Package file changed to a non-regular file");
    }
    Ok((input, metadata))
}

fn copy_regular_file(
    mut input: std::fs::File,
    metadata: &std::fs::Metadata,
    destination: &Path,
) -> Result<()> {
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    std::io::copy(&mut input, &mut output)?;
    output.set_permissions(metadata.permissions())?;
    Ok(())
}

async fn tree_hash_async(path: PathBuf) -> Result<String> {
    tokio::task::spawn_blocking(move || tree_hash(&path)).await?
}

fn tree_hash(root: &Path) -> Result<String> {
    if !std::fs::symlink_metadata(root)?.is_dir() {
        bail!("Package is not a regular directory");
    }
    let root = Dir::open_ambient_dir(root, ambient_authority())?;
    let mut hasher = blake3::Hasher::new();
    let mut pending = vec![(root, PathBuf::new(), 0_usize)];
    let mut count = 0_usize;
    let mut buffer = [0_u8; 16_384];
    while let Some((directory, relative, depth)) = pending.pop() {
        if depth > MAX_DEPTH {
            bail!("Installed package exceeds depth limit");
        }
        let mut entries = Vec::new();
        for entry in directory.entries()? {
            count = count.saturating_add(1);
            if count > MAX_ENTRIES {
                bail!("Installed package exceeds entry limit");
            }
            entries.push(entry?);
        }
        entries.sort_by_key(cap_std::fs::DirEntry::file_name);
        for entry in entries {
            let kind = entry.file_type()?;
            let basename = entry.file_name();
            let path = relative.join(&basename);
            let name = path.to_str().context("Package paths must be UTF-8")?;
            hasher.update(&u64::try_from(name.len())?.to_le_bytes());
            hasher.update(name.as_bytes());
            if kind.is_dir() {
                hasher.update(b"directory");
                pending.push((directory.open_dir(basename)?, path, depth.saturating_add(1)));
            } else if kind.is_file() {
                hasher.update(b"file");
                let (mut file, metadata) = open_regular_file(&directory, &basename)?;
                hasher.update(&metadata.len().to_le_bytes());
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    hasher.update(&metadata.permissions().mode().to_le_bytes());
                }
                loop {
                    let read = file.read(&mut buffer)?;
                    if read == 0 {
                        break;
                    }
                    hasher.update(buffer.get(..read).context("Invalid read size")?);
                }
            } else {
                bail!("Package contains a symlink or non-regular file");
            }
        }
    }
    Ok(hasher.finalize().to_hex().to_string())
}

async fn path_exists(path: &Path) -> Result<bool> {
    match tokio::fs::symlink_metadata(path).await {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

#[derive(Debug, Default, Clone)]
pub struct SyncReport {
    pub added: Vec<String>,
    pub updated: Vec<String>,
    pub removed: Vec<String>,
    pub errors: Vec<String>,
}

impl SyncReport {
    pub fn merge(&mut self, other: Self) {
        self.added.extend(other.added);
        self.updated.extend(other.updated);
        self.removed.extend(other.removed);
        self.errors.extend(other.errors);
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.added.is_empty()
            && self.updated.is_empty()
            && self.removed.is_empty()
            && self.errors.is_empty()
    }

    #[must_use]
    pub const fn total_changes(&self) -> usize {
        self.added
            .len()
            .saturating_add(self.updated.len())
            .saturating_add(self.removed.len())
    }
}
