//! Policy Engine
//!
//! Enforces authorization policies for callable execution:
//! - Risk tier gating (consent requirements)
//! - Allowlist/denylist filtering
//! - Server trust levels
//! - Resource limits and quotas

use crate::{CallableRecord, RiskTier};
use glob::Pattern;
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

impl FromStr for ConsentLevel {
    type Err = (); // Infallible - defaults to None

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        Ok(match s.to_lowercase().as_str() {
            "user_confirmed" => ConsentLevel::UserConfirmed,
            "admin_confirmed" => ConsentLevel::AdminConfirmed,
            _ => ConsentLevel::None,
        })
    }
}

impl std::fmt::Display for ConsentLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConsentLevel::None => write!(f, "none"),
            ConsentLevel::UserConfirmed => write!(f, "user_confirmed"),
            ConsentLevel::AdminConfirmed => write!(f, "admin_confirmed"),
        }
    }
}

/// Policy configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyConfig {
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

fn default_max_calls() -> usize {
    30
}

fn default_max_exec_ms() -> u64 {
    120000
}

impl Default for PolicyConfig {
    fn default() -> Self {
        PolicyConfig {
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

/// Authorization result
#[derive(Debug, Clone)]
pub struct AuthorizationResult {
    pub allowed: bool,
    pub reason: String,
    pub required_consent: Option<ConsentLevel>,
}

impl AuthorizationResult {
    pub fn allow() -> Self {
        AuthorizationResult {
            allowed: true,
            reason: "Authorized".to_string(),
            required_consent: None,
        }
    }

    pub fn deny(reason: String) -> Self {
        AuthorizationResult {
            allowed: false,
            reason,
            required_consent: None,
        }
    }

    pub fn deny_with_consent(reason: String, required: ConsentLevel) -> Self {
        AuthorizationResult {
            allowed: false,
            reason,
            required_consent: Some(required),
        }
    }
}

/// Policy engine
pub struct PolicyEngine {
    config: PolicyConfig,
    consent_required_tiers: HashSet<RiskTier>,
    trusted_servers: HashSet<String>,
    deny_tags: HashSet<String>,
    allow_patterns: Vec<Pattern>,
    deny_patterns: Vec<Pattern>,
}

impl PolicyEngine {
    /// Create new policy engine from config
    pub fn new(config: PolicyConfig) -> Result<Self> {
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

        Ok(PolicyEngine {
            config,
            consent_required_tiers,
            trusted_servers,
            deny_tags,
            allow_patterns,
            deny_patterns,
        })
    }

    /// Create permissive policy engine (for testing)
    pub fn new_permissive() -> Self {
        let config = PolicyConfig {
            default_risk: "read_only".to_string(),
            require_consent_for: vec![],
            trusted_servers: vec!["*".to_string()],
            deny_tags: vec![],
            max_calls_per_skill: 100,
            max_exec_ms: 300000,
            allow_patterns: vec!["*".to_string()],
            deny_patterns: vec![],
        };

        PolicyEngine::new(config).unwrap()
    }

    /// Authorize a callable for execution
    pub async fn authorize(
        &self,
        callable: &CallableRecord,
        _arguments: &serde_json::Value,
        consent: ConsentLevel,
    ) -> Result<AuthorizationResult> {
        debug!("Authorizing callable: {}", callable.id.as_str());

        // Check deny tags
        for tag in &callable.tags {
            if self.deny_tags.contains(tag) {
                warn!("Callable denied due to tag: {}", tag);
                return Ok(AuthorizationResult::deny(format!(
                    "Callable has denied tag: {}",
                    tag
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
                    "Server not in trusted list: {}",
                    server
                )));
            }
        }

        // Check risk tier and consent
        if self.consent_required_tiers.contains(&callable.risk_tier) {
            let required_consent = match callable.risk_tier {
                RiskTier::Admin => ConsentLevel::AdminConfirmed,
                RiskTier::Destructive => ConsentLevel::UserConfirmed,
                RiskTier::Writes => ConsentLevel::UserConfirmed,
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
    pub fn max_calls_per_skill(&self) -> usize {
        self.config.max_calls_per_skill
    }

    /// Check if a server is trusted
    pub fn is_server_trusted(&self, server: &str) -> bool {
        self.trusted_servers.contains(server) || self.trusted_servers.contains("*")
    }
}
