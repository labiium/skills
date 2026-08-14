//! Policy Engine
//!
//! Enforces authorization policies for callable execution:
//! - Risk tier gating (consent requirements)
//! - Allowlist/denylist filtering
//! - Server trust levels
//! - Resource limits and quotas

use crate::{CallableRecord, RiskTier};
use glob::Pattern;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::str::FromStr;
use thiserror::Error;
use tracing::{debug, warn};

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

/// Effective authorization profile. `Open` is intended for a Mesurii workspace
/// (the workspace boundary is the trust boundary); the historical profile stays
/// the default for other consumers.
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
pub struct PolicyConfig {
    /// Authorization profile. `SKILLS_POLICY=open` selects the open profile at runtime.
    #[serde(default)]
    pub mode: PolicyMode,
    /// Default risk tier for unknown callables
    #[serde(default = "default_risk")]
    pub default_risk: String,

    /// Risk tiers that require consent
    #[serde(default)]
    pub require_consent_for: Vec<String>,

    /// Trusted server aliases
    #[serde(default)]
    pub trusted_servers: Vec<String>,

    /// Denied tags (callables with these tags are blocked)
    #[serde(default)]
    pub deny_tags: Vec<String>,

    /// Maximum calls per skill execution
    #[serde(default = "default_max_calls")]
    pub max_calls_per_skill: usize,

    /// Maximum execution time in milliseconds
    #[serde(default = "default_max_exec_ms")]
    pub max_exec_ms: u64,

    /// Allow patterns (glob-style)
    #[serde(default)]
    pub allow_patterns: Vec<String>,

    /// Deny patterns (glob-style)
    #[serde(default)]
    pub deny_patterns: Vec<String>,
}

fn default_risk() -> String {
    "unknown".to_string()
}

const fn default_max_calls() -> usize {
    30
}

const fn default_max_exec_ms() -> u64 {
    120_000
}

impl Default for PolicyConfig {
    fn default() -> Self {
        Self {
            mode: PolicyMode::Standard,
            default_risk: default_risk(),
            require_consent_for: vec![
                "writes".to_string(),
                "destructive".to_string(),
                "admin".to_string(),
            ],
            trusted_servers: vec![],
            deny_tags: vec![],
            max_calls_per_skill: default_max_calls(),
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
            require_consent_for: vec![],
            trusted_servers: vec!["*".to_string()],
            max_calls_per_skill: usize::MAX,
            max_exec_ms: u64::MAX,
            ..Self::default()
        }
    }

    /// Apply the process-level policy selector without changing non-open configs.
    #[must_use]
    pub fn from_environment(config: Self) -> Self {
        Self::from_selector(config, std::env::var("SKILLS_POLICY").ok().as_deref())
    }

    #[must_use]
    pub fn from_selector(mut config: Self, selector: Option<&str>) -> Self {
        if selector.is_some_and(|value| value.trim().eq_ignore_ascii_case("open")) {
            config.mode = PolicyMode::Open;
            config.require_consent_for.clear();
            config.trusted_servers = vec!["*".to_string()];
            config.allow_patterns = vec!["*".to_string()];
            config.deny_patterns.clear();
            config.deny_tags.clear();
            config.max_calls_per_skill = usize::MAX;
            config.max_exec_ms = u64::MAX;
        }
        config
    }
}

/// Machine-readable effective policy state exposed by the MCP façade.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PolicyState {
    pub mode: String,
    pub unconditional: bool,
    /// `None` denotes an intentional unbounded limit in the open workspace profile.
    pub max_calls_per_skill: Option<usize>,
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
    /// # Errors
    /// Returns an error if the operation cannot be completed.
    pub fn new(mut config: PolicyConfig) -> Result<Self> {
        if config.mode == PolicyMode::Open {
            config.max_calls_per_skill = usize::MAX;
            config.max_exec_ms = u64::MAX;
        }
        let consent_required_tiers: HashSet<RiskTier> = config
            .require_consent_for
            .iter()
            .filter_map(|s| s.parse().ok())
            .collect();

        let trusted_servers: HashSet<String> = config.trusted_servers.iter().cloned().collect();
        let deny_tags: HashSet<String> = config.deny_tags.iter().cloned().collect();

        let allow_patterns: Vec<Pattern> = config
            .allow_patterns
            .iter()
            .filter_map(|p| Pattern::new(p).ok())
            .collect();

        let deny_patterns: Vec<Pattern> = config
            .deny_patterns
            .iter()
            .filter_map(|p| Pattern::new(p).ok())
            .collect();

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
        let config = PolicyConfig {
            mode: PolicyMode::Open,
            default_risk: "read_only".to_string(),
            require_consent_for: vec![],
            trusted_servers: vec!["*".to_string()],
            deny_tags: vec![],
            max_calls_per_skill: 100,
            max_exec_ms: 300_000,
            allow_patterns: vec!["*".to_string()],
            deny_patterns: vec![],
        };

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
    /// # Errors
    /// Returns an error if the operation cannot be completed.
    #[expect(
        clippy::unused_async,
        reason = "authorization is async to preserve the public execution interface"
    )]
    pub async fn authorize(
        &self,
        callable: &CallableRecord,
        _arguments: &serde_json::Value,
        consent: ConsentLevel,
    ) -> Result<AuthorizationResult> {
        debug!("Authorizing callable: {}", callable.id.as_str());

        // Open means the workspace is the trust boundary. Deliberately bypass
        // consent, trust, allowlist, denylist, and risk-tier gates.
        if self.mode == PolicyMode::Open {
            return Ok(AuthorizationResult::allow());
        }

        // Check deny tags
        for tag in &callable.tags {
            if self.deny_tags.contains(tag) {
                warn!("Callable denied due to tag: {}", tag);
                return Ok(AuthorizationResult::deny(format!(
                    "Callable has denied tag: {tag}"
                )));
            }
        }

        // Check deny patterns
        for pattern in &self.deny_patterns {
            if pattern.matches(&callable.fq_name) {
                warn!("Callable denied by pattern: {}", pattern.as_str());
                return Ok(AuthorizationResult::deny(format!(
                    "Callable matches deny pattern: {}",
                    pattern.as_str()
                )));
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
                return Ok(AuthorizationResult::deny(
                    "Callable not in allowlist".to_string(),
                ));
            }
        }

        // Check server trust (tools only)
        if let Some(server) = &callable.server_alias {
            if !self.trusted_servers.is_empty()
                && !self.trusted_servers.contains(server)
                && !self.trusted_servers.contains("*")
            {
                warn!("Server not trusted: {}", server);
                return Ok(AuthorizationResult::deny(format!(
                    "Server not in trusted list: {server}"
                )));
            }
        }

        // Check risk tier and consent
        if self.consent_required_tiers.contains(&callable.risk_tier) {
            let required_consent = match callable.risk_tier {
                RiskTier::Admin => ConsentLevel::AdminConfirmed,
                RiskTier::Destructive | RiskTier::Writes => ConsentLevel::UserConfirmed,
                _ => ConsentLevel::None,
            };

            if consent < required_consent {
                warn!(
                    "Insufficient consent for {}: required {:?}, provided {:?}",
                    callable.fq_name, required_consent, consent
                );
                return Ok(AuthorizationResult::deny_with_consent(
                    format!(
                        "Risk tier {} requires consent level {}",
                        callable.risk_tier, required_consent
                    ),
                    required_consent,
                ));
            }
        }

        debug!("Authorization granted for: {}", callable.fq_name);
        Ok(AuthorizationResult::allow())
    }

    /// Check if execution time is within limits
    /// # Errors
    /// Returns an error if the operation cannot be completed.
    pub fn check_timeout(&self, requested_ms: Option<u64>) -> Result<u64> {
        let timeout = requested_ms.unwrap_or(self.config.max_exec_ms);
        if timeout > self.config.max_exec_ms {
            return Err(PolicyError::LimitExceeded(format!(
                "Requested timeout {}ms exceeds maximum {}ms",
                timeout, self.config.max_exec_ms
            )));
        }
        Ok(timeout)
    }

    /// Get max calls per skill
    #[must_use]
    pub const fn max_calls_per_skill(&self) -> usize {
        self.config.max_calls_per_skill
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
            max_calls_per_skill: (self.mode != PolicyMode::Open)
                .then_some(self.config.max_calls_per_skill),
            max_exec_ms: (self.mode != PolicyMode::Open).then_some(self.config.max_exec_ms),
        }
    }

    /// Check if a server is trusted
    #[must_use]
    pub fn is_server_trusted(&self, server: &str) -> bool {
        self.trusted_servers.contains(server) || self.trusted_servers.contains("*")
    }
}
