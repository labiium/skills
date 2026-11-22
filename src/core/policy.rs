//! Policy Engine
//!
//! Enforces authorization policies for callable execution:
//! - Risk tier gating (consent requirements)
//! - Allowlist/denylist filtering
//! - Server trust levels
//! - Execution deadlines

use crate::{CallableRecord, RiskTier};
use glob::Pattern;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::str::FromStr;
use thiserror::Error;
use tracing::warn;

#[derive(Error, Debug)]
pub enum PolicyError {
    #[error("Authorization denied: {0}")]
    Denied(String),

    #[error("Insufficient consent level: required {required}, provided {provided}")]
    InsufficientConsent { required: String, provided: String },

    #[error("Policy configuration error: {0}")]
    ConfigError(String),

    #[error("Resource limit exceeded: {0}")]
    LimitExceeded(String),
}

pub type Result<T> = std::result::Result<T, PolicyError>;

/// Consent level for execution
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsentLevel {
    None,
    UserConfirmed,
    AdminConfirmed,
}

/// `Open` trusts the Mesurii workspace boundary. Standard is conservative.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PolicyMode {
    #[default]
    Standard,
    Open,
}

impl std::fmt::Display for PolicyMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Standard => "standard",
            Self::Open => "open",
        })
    }
}

impl FromStr for ConsentLevel {
    type Err = (); // Infallible - defaults to None

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        Ok(match s.to_lowercase().as_str() {
            "user_confirmed" => Self::UserConfirmed,
            "admin_confirmed" => Self::AdminConfirmed,
            _ => Self::None,
        })
    }
}

impl std::fmt::Display for ConsentLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => write!(f, "none"),
            Self::UserConfirmed => write!(f, "user_confirmed"),
            Self::AdminConfirmed => write!(f, "admin_confirmed"),
        }
    }
}

/// Policy configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PolicyConfig {
    pub mode: PolicyMode,
    /// Enable mutating MCP management for callers inside the host's trust boundary.
    pub management_enabled: bool,
    /// Require an explicit trusted server entry for upstream reads and execution.
    pub enforce_server_trust: bool,
    /// Risk tier used for callables without authoritative risk metadata.
    pub default_risk: String,
    pub require_consent_for: Vec<String>,
    pub trusted_servers: Vec<String>,
    pub deny_tags: Vec<String>,
    pub max_exec_ms: u64,
    pub allow_patterns: Vec<String>,
    pub deny_patterns: Vec<String>,
}

fn default_risk() -> String {
    "unknown".to_string()
}

const fn default_max_exec_ms() -> u64 {
    120_000
}

impl Default for PolicyConfig {
    fn default() -> Self {
        Self {
            mode: PolicyMode::Standard,
            management_enabled: false,
            enforce_server_trust: true,
            default_risk: default_risk(),
            require_consent_for: vec![
                "writes".to_string(),
                "destructive".to_string(),
                "admin".to_string(),
                "unknown".to_string(),
            ],
            trusted_servers: vec![],
            deny_tags: vec![],
            max_exec_ms: default_max_exec_ms(),
            allow_patterns: vec!["*".to_string()],
            deny_patterns: vec![],
        }
    }
}

impl PolicyConfig {
    /// Return the explicitly open profile used by Mesurii workspaces.
    #[must_use]
    pub fn open() -> Self {
        Self {
            mode: PolicyMode::Open,
            management_enabled: true,
            enforce_server_trust: false,
            require_consent_for: vec![],
            trusted_servers: vec!["*".to_string()],
            max_exec_ms: u64::MAX,
            ..Self::default()
        }
    }

    /// Apply the process-level policy selector without changing non-open configs.
    #[must_use]
    pub fn from_environment(config: Self) -> Self {
        Self::from_selector(config, std::env::var("SKILLS_POLICY").ok().as_deref())
    }

    /// Read the externally asserted source revision supplied by the deployment.
    /// This validates its shape, not the provenance of the running binary.
    ///
    /// # Errors
    /// Returns a configuration error when a required revision is absent or not exactly 40 hex digits.
    pub fn mesurii_source_revision(require: bool) -> Result<Option<String>> {
        let configured = std::env::var("MESURII_SKILLS_SOURCE_REVISION").ok();
        let value = exact_source_revision(configured.as_deref());
        if require && value.is_none() {
            return Err(PolicyError::ConfigError(
                "MESURII_SKILLS_SOURCE_REVISION must be a non-empty exact 40-hex revision for the open profile".to_string(),
            ));
        }
        Ok(value)
    }

    #[must_use]
    pub fn from_selector(mut config: Self, selector: Option<&str>) -> Self {
        if selector.is_some_and(|value| value.trim().eq_ignore_ascii_case("open")) {
            config.mode = PolicyMode::Open;
            config.management_enabled = true;
            config.enforce_server_trust = false;
            config.require_consent_for.clear();
            config.trusted_servers = vec!["*".to_string()];
            config.allow_patterns = vec!["*".to_string()];
            config.deny_patterns.clear();
            config.deny_tags.clear();
            config.max_exec_ms = u64::MAX;
        }
        config
    }
}

fn exact_source_revision(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .map(str::to_owned)
}

#[cfg(test)]
mod source_revision_tests {
    use super::exact_source_revision;

    #[test]
    fn exact_source_revision_accepts_only_forty_hex_characters() {
        let revision = "0123456789abcdef0123456789abcdef01234567";
        assert_eq!(
            exact_source_revision(Some(revision)).as_deref(),
            Some(revision)
        );
        assert_eq!(
            exact_source_revision(Some(&format!("  {revision}  "))).as_deref(),
            Some(revision)
        );
        assert_eq!(exact_source_revision(None), None);
        assert_eq!(exact_source_revision(Some("not-a-revision")), None);
        assert_eq!(
            exact_source_revision(Some("0123456789abcdef0123456789abcdef012345678")),
            None
        );
    }
}

/// Machine-readable effective policy state exposed by the MCP façade.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PolicyState {
    pub mode: String,
    pub unconditional: bool,
    pub management_enabled: bool,
    /// `None` denotes the open workspace's unbounded policy deadline.
    pub max_exec_ms: Option<u64>,
}

/// Authorization result
#[derive(Debug, Clone)]
pub struct AuthorizationResult {
    pub allowed: bool,
    pub reason: String,
    pub required_consent: Option<ConsentLevel>,
}

impl AuthorizationResult {
    #[must_use]
    pub fn allow() -> Self {
        Self {
            allowed: true,
            reason: "Authorized".to_string(),
            required_consent: None,
        }
    }

    #[must_use]
    pub const fn deny(reason: String) -> Self {
        Self {
            allowed: false,
            reason,
            required_consent: None,
        }
    }

    #[must_use]
    pub const fn deny_with_consent(reason: String, required: ConsentLevel) -> Self {
        Self {
            allowed: false,
            reason,
            required_consent: Some(required),
        }
    }
}

/// Policy engine
pub struct PolicyEngine {
    config: PolicyConfig,
    mode: PolicyMode,
    consent_required_tiers: HashSet<RiskTier>,
    trusted_servers: HashSet<String>,
    deny_tags: HashSet<String>,
    allow_patterns: Vec<Pattern>,
    deny_patterns: Vec<Pattern>,
}

impl PolicyEngine {
    /// Create new policy engine from config
    ///
    /// # Errors
    /// Returns a configuration error for zero deadlines, invalid risk tiers, or invalid glob patterns.
    pub fn new(mut config: PolicyConfig) -> Result<Self> {
        if config.mode == PolicyMode::Open {
            config.max_exec_ms = u64::MAX;
            config.management_enabled = true;
            config.enforce_server_trust = false;
        }
        if config.max_exec_ms == 0 {
            return Err(PolicyError::ConfigError(
                "max_exec_ms must be positive".to_string(),
            ));
        }
        config.default_risk.parse::<RiskTier>().map_err(|_| {
            PolicyError::ConfigError(format!("Invalid default_risk: {}", config.default_risk))
        })?;
        let consent_required_tiers: HashSet<RiskTier> = config
            .require_consent_for
            .iter()
            .map(|tier| {
                tier.parse().map_err(|_| {
                    PolicyError::ConfigError(format!("Invalid consent risk tier: {tier}"))
                })
            })
            .collect::<Result<_>>()?;

        let trusted_servers: HashSet<String> = config.trusted_servers.iter().cloned().collect();
        let deny_tags: HashSet<String> = config.deny_tags.iter().cloned().collect();

        let allow_patterns: Vec<Pattern> = config
            .allow_patterns
            .iter()
            .map(|p| Pattern::new(p).map_err(|error| PolicyError::ConfigError(error.to_string())))
            .collect::<Result<_>>()?;
        let deny_patterns: Vec<Pattern> = config
            .deny_patterns
            .iter()
            .map(|p| Pattern::new(p).map_err(|error| PolicyError::ConfigError(error.to_string())))
            .collect::<Result<_>>()?;

        let mode = config.mode;
        Ok(Self {
            config,
            mode,
            consent_required_tiers,
            trusted_servers,
            deny_tags,
            allow_patterns,
            deny_patterns,
        })
    }

    /// Create permissive policy engine (for testing)
    #[must_use]
    pub fn new_permissive() -> Self {
        let config = PolicyConfig::open();

        Self {
            consent_required_tiers: HashSet::new(),
            trusted_servers: HashSet::from(["*".to_string()]),
            deny_tags: HashSet::new(),
            allow_patterns: Pattern::new("*").into_iter().collect(),
            deny_patterns: Vec::new(),
            config,
            mode: PolicyMode::Open,
        }
    }

    /// Authorize a callable for execution
    ///
    /// # Errors
    /// Returns a configuration error if the default risk tier cannot be parsed.
    pub fn authorize(
        &self,
        callable: &CallableRecord,
        _arguments: &serde_json::Value,
        consent: ConsentLevel,
    ) -> Result<AuthorizationResult> {
        self.authorize_risk(callable, callable.risk_tier, consent)
    }

    /// Bundled scripts are unreviewed code regardless of the instruction skill's risk.
    ///
    /// # Errors
    /// Returns a configuration error if the default risk tier cannot be parsed.
    pub fn authorize_script(
        &self,
        callable: &CallableRecord,
        consent: ConsentLevel,
    ) -> Result<AuthorizationResult> {
        self.authorize_risk(callable, RiskTier::Unknown, consent)
    }

    fn authorize_risk(
        &self,
        callable: &CallableRecord,
        risk: RiskTier,
        consent: ConsentLevel,
    ) -> Result<AuthorizationResult> {
        let visibility = self.authorize_read(callable);
        if !visibility.allowed || self.is_open() {
            return Ok(visibility);
        }
        let risk_tier = if risk == RiskTier::Unknown {
            self.config
                .default_risk
                .parse()
                .map_err(|_| PolicyError::ConfigError("Invalid default risk tier".to_string()))?
        } else {
            risk
        };
        if self.consent_required_tiers.contains(&risk_tier) {
            let required = match risk_tier {
                RiskTier::Admin => ConsentLevel::AdminConfirmed,
                _ => ConsentLevel::UserConfirmed,
            };
            if consent < required {
                return Ok(AuthorizationResult::deny_with_consent(
                    format!("Risk tier {risk_tier} requires trusted consent level {required}"),
                    required,
                ));
            }
        }
        Ok(AuthorizationResult::allow())
    }

    /// Authorize metadata and instruction reads without granting code execution.
    #[must_use]
    pub fn authorize_read(&self, callable: &CallableRecord) -> AuthorizationResult {
        // Open means the workspace is the trust boundary. Deliberately bypass
        // consent, trust, allowlist, denylist, and risk-tier gates.
        if self.mode == PolicyMode::Open {
            return AuthorizationResult::allow();
        }

        // Check deny tags
        for tag in &callable.tags {
            if self.deny_tags.contains(tag) {
                warn!("Callable denied due to tag: {}", tag);
                return AuthorizationResult::deny(format!("Callable has denied tag: {tag}"));
            }
        }

        // Check deny patterns
        for pattern in &self.deny_patterns {
            if pattern.matches(&callable.fq_name) {
                warn!("Callable denied by pattern: {}", pattern.as_str());
                return AuthorizationResult::deny(format!(
                    "Callable matches deny pattern: {}",
                    pattern.as_str()
                ));
            }
        }

        // Check allow patterns (must match at least one)
        if !self.allow_patterns.is_empty() {
            let matched = self
                .allow_patterns
                .iter()
                .any(|p| p.matches(&callable.fq_name));

            if !matched {
                warn!("Callable not in allowlist: {}", callable.fq_name);
                return AuthorizationResult::deny("Callable not in allowlist".to_string());
            }
        }

        // Check server trust (tools only)
        if let Some(server) = &callable.server_alias {
            if self.config.enforce_server_trust
                && !self.trusted_servers.contains(server)
                && !self.trusted_servers.contains("*")
            {
                warn!("Server not trusted: {}", server);
                return AuthorizationResult::deny(format!("Server not in trusted list: {server}"));
            }
        }
        AuthorizationResult::allow()
    }

    /// Check if execution time is within limits
    ///
    /// # Errors
    /// Returns a resource-limit error when the requested timeout is zero.
    pub fn check_timeout(&self, requested_ms: Option<u64>) -> Result<u64> {
        let timeout = requested_ms.unwrap_or(self.config.max_exec_ms);
        if timeout == 0 {
            return Err(PolicyError::LimitExceeded(
                "Execution timeout must be positive".to_string(),
            ));
        }
        Ok(timeout.min(self.config.max_exec_ms))
    }

    /// Require a host-configured management trust boundary before any mutation.
    ///
    /// # Errors
    /// Returns an authorization error when the host has not enabled management.
    pub fn authorize_management(&self) -> Result<()> {
        if self.is_open() || self.config.management_enabled {
            Ok(())
        } else {
            Err(PolicyError::Denied(
                "Mutating MCP management is disabled; configure policy.management_enabled explicitly".to_string(),
            ))
        }
    }

    #[must_use]
    pub fn gates(&self) -> Vec<String> {
        if self.is_open() {
            return Vec::new();
        }
        let mut gates = vec!["timeout".to_string()];
        if !self.config.management_enabled {
            gates.push("management".to_string());
        }
        if self.config.enforce_server_trust {
            gates.push("trust".to_string());
        }
        if !self.consent_required_tiers.is_empty() {
            gates.push("consent".to_string());
        }
        if !self.deny_patterns.is_empty() || !self.deny_tags.is_empty() {
            gates.push("deny".to_string());
        }
        if !self.allow_patterns.is_empty()
            && !self
                .allow_patterns
                .iter()
                .any(|pattern| pattern.as_str() == "*")
        {
            gates.push("allowlist".to_string());
        }
        gates
    }

    #[must_use]
    pub fn is_open(&self) -> bool {
        self.mode == PolicyMode::Open
    }

    #[must_use]
    pub fn state(&self) -> PolicyState {
        PolicyState {
            mode: self.mode.to_string(),
            unconditional: self.is_open(),
            management_enabled: self.is_open() || self.config.management_enabled,
            max_exec_ms: (self.mode != PolicyMode::Open).then_some(self.config.max_exec_ms),
        }
    }

    /// Check if a server is trusted
    #[must_use]
    pub fn is_server_trusted(&self, server: &str) -> bool {
        self.is_open()
            || !self.config.enforce_server_trust
            || self.trusted_servers.contains(server)
            || self.trusted_servers.contains("*")
    }
}
