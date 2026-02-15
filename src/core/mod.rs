//! Core types and data structures for skills.rs
//!
//! This crate defines the fundamental types used across the system:
//! - Callable identifiers and records
//! - Risk tiers and cost hints
//! - Schema digests and canonicalization
//! - MCP protocol types
//! - Persistence layer
//! - Registry store for callables
//! - Policy engine for access control

pub mod persistence;
pub mod policy;
pub mod registry;

use blake3::Hash;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;
use thiserror::Error;

/// Error types for core operations
#[derive(Error, Debug)]
pub enum CoreError {
    #[error("Invalid callable ID format: {0}")]
    InvalidCallableId(String),

    #[error("Schema canonicalization failed: {0}")]
    CanonicalizeError(String),

    #[error("Invalid risk tier: {0}")]
    InvalidRiskTier(String),

    #[error("Serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, CoreError>;

/// Risk tier classification for callables
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Hash)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum RiskTier {
    /// Read-only operations
    ReadOnly,
    /// Operations that write data
    Writes,
    /// Operations that can destroy data
    Destructive,
    /// Administrative operations
    Admin,
    /// Unknown risk level (default for untrusted sources)
    #[default]
    Unknown,
}

impl RiskTier {
    pub fn requires_consent(&self) -> bool {
        matches!(
            self,
            RiskTier::Writes | RiskTier::Destructive | RiskTier::Admin
        )
    }
}

impl FromStr for RiskTier {
    type Err = CoreError;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "read_only" | "readonly" => Ok(RiskTier::ReadOnly),
            "writes" | "write" => Ok(RiskTier::Writes),
            "destructive" => Ok(RiskTier::Destructive),
            "admin" => Ok(RiskTier::Admin),
            "unknown" => Ok(RiskTier::Unknown),
            _ => Err(CoreError::InvalidRiskTier(s.to_string())),
        }
    }
}

impl fmt::Display for RiskTier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RiskTier::ReadOnly => write!(f, "read_only"),
            RiskTier::Writes => write!(f, "writes"),
            RiskTier::Destructive => write!(f, "destructive"),
            RiskTier::Admin => write!(f, "admin"),
            RiskTier::Unknown => write!(f, "unknown"),
        }
    }
}

/// Cost hints for a callable
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CostHints {
    pub expected_calls: Option<u32>,
    pub estimated_duration_ms: Option<u32>,
    pub network_required: bool,
    pub filesystem_access: bool,
}

/// Bundled tool within a skill
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundledTool {
    pub name: String,
    pub description: String,
    pub command: Vec<String>,
    pub schema: serde_json::Value,
}

/// Kind of callable (tool or skill)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "lowercase")]
pub enum CallableKind {
    Tool,
    Skill,
}

impl fmt::Display for CallableKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CallableKind::Tool => write!(f, "tool"),
            CallableKind::Skill => write!(f, "skill"),
        }
    }
}

/// Stable identifier for a callable (tool or skill)
///
/// Format:
/// - Tools: `tool:srv:<alias>::<name>::sd:<digest8>`
/// - Skills: `skill:<skill_id>@<version>`
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CallableId(String);

impl CallableId {
    /// Create a tool ID
    pub fn tool(server_alias: &str, tool_name: &str, schema_digest: &str) -> Self {
        let digest8 = &schema_digest[..8.min(schema_digest.len())];
        CallableId(format!(
            "tool:srv:{}::{}::sd:{}",
            server_alias, tool_name, digest8
        ))
    }

    /// Create a skill ID
    pub fn skill(skill_id: &str, version: &str) -> Self {
        CallableId(format!("skill:{}@{}", skill_id, version))
    }

    /// Parse a callable ID and return its kind
    pub fn kind(&self) -> Result<CallableKind> {
        if self.0.starts_with("tool:") {
            Ok(CallableKind::Tool)
        } else if self.0.starts_with("skill:") {
            Ok(CallableKind::Skill)
        } else {
            Err(CoreError::InvalidCallableId(self.0.clone()))
        }
    }

    /// Extract server alias (tools only)
    pub fn server_alias(&self) -> Option<String> {
        if let Some(rest) = self.0.strip_prefix("tool:srv:") {
            if let Some(alias) = rest.split("::").next() {
                return Some(alias.to_string());
            }
        }
        None
    }

    /// Extract tool name (tools only)
    pub fn tool_name(&self) -> Option<String> {
        if let Some(rest) = self.0.strip_prefix("tool:srv:") {
            let parts: Vec<&str> = rest.split("::").collect();
            if parts.len() >= 2 {
                return Some(parts[1].to_string());
            }
        }
        None
    }

    /// Extract skill name (skills only)
    pub fn skill_name(&self) -> Option<String> {
        if let Some(rest) = self.0.strip_prefix("skill:") {
            if let Some(name) = rest.split('@').next() {
                return Some(name.to_string());
            }
        }
        None
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CallableId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for CallableId {
    fn from(s: String) -> Self {
        CallableId(s)
    }
}

impl From<&str> for CallableId {
    fn from(s: &str) -> Self {
        CallableId(s.to_string())
    }
}

/// Schema digest using BLAKE3
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaDigest(String);

impl SchemaDigest {
    /// Compute digest from JSON schema
    pub fn from_schema(schema: &serde_json::Value) -> Result<Self> {
        let canonical = canonicalize_json(schema)?;
        let hash = blake3::hash(canonical.as_bytes());
        Ok(SchemaDigest(hash.to_hex().to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Get first 8 characters for short representation
    pub fn short(&self) -> &str {
        &self.0[..8.min(self.0.len())]
    }
}

impl fmt::Display for SchemaDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<Hash> for SchemaDigest {
    fn from(hash: Hash) -> Self {
        SchemaDigest(hash.to_hex().to_string())
    }
}

impl From<String> for SchemaDigest {
    fn from(s: String) -> Self {
        SchemaDigest(s)
    }
}

impl From<&str> for SchemaDigest {
    fn from(s: &str) -> Self {
        SchemaDigest(s.to_string())
    }
}

/// Canonicalize JSON for stable hashing (sorted keys)
pub fn canonicalize_json(value: &serde_json::Value) -> Result<String> {
    fn sort_value(value: &serde_json::Value) -> serde_json::Value {
        match value {
            serde_json::Value::Object(map) => {
                let mut sorted = serde_json::Map::new();
                let mut keys: Vec<_> = map.keys().collect();
                keys.sort();
                for key in keys {
                    sorted.insert(key.clone(), sort_value(&map[key]));
                }
                serde_json::Value::Object(sorted)
            }
            serde_json::Value::Array(arr) => {
                serde_json::Value::Array(arr.iter().map(sort_value).collect())
            }
            other => other.clone(),
        }
    }

    let sorted = sort_value(value);
    serde_json::to_string(&sorted).map_err(CoreError::from)
}

/// Main registry record for a callable (tool or skill)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallableRecord {
    pub id: CallableId,
    pub kind: CallableKind,
    pub fq_name: String,
    pub name: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub input_schema: serde_json::Value,
    pub output_schema: Option<serde_json::Value>,
    pub schema_digest: SchemaDigest,

    // Tool-specific fields
    pub server_alias: Option<String>,
    pub upstream_tool_name: Option<String>,

    // Skill-specific fields
    pub skill_version: Option<String>,
    pub uses: Vec<CallableId>,
    pub skill_directory: Option<std::path::PathBuf>,
    pub bundled_tools: Vec<BundledTool>,
    pub additional_files: Vec<String>,

    // Metadata
    pub cost_hints: CostHints,
    pub risk_tier: RiskTier,
    pub last_seen: DateTime<Utc>,
}

impl CallableRecord {
    pub fn is_tool(&self) -> bool {
        self.kind == CallableKind::Tool
    }

    pub fn is_skill(&self) -> bool {
        self.kind == CallableKind::Skill
    }
}

/// MCP tool result content
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ToolResultContent {
    #[serde(rename = "text")]
    Text { text: String },

    #[serde(rename = "image")]
    Image { data: String, mime_type: String },

    #[serde(rename = "resource")]
    Resource { resource: ResourceContent },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceContent {
    pub uri: String,
    pub mime_type: Option<String>,
    pub text: Option<String>,
    pub blob: Option<String>,
}

/// MCP tool result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub content: Vec<ToolResultContent>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub structured_content: Option<serde_json::Value>,

    #[serde(default)]
    pub is_error: bool,
}

impl ToolResult {
    pub fn success(text: String) -> Self {
        ToolResult {
            content: vec![ToolResultContent::Text { text }],
            structured_content: None,
            is_error: false,
        }
    }

    pub fn error(message: String) -> Self {
        ToolResult {
            content: vec![ToolResultContent::Text { text: message }],
            structured_content: None,
            is_error: true,
        }
    }

    pub fn with_structured(mut self, data: serde_json::Value) -> Self {
        self.structured_content = Some(data);
        self
    }
}

/// MCP tool definition (upstream)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: Option<String>,
    #[serde(rename = "inputSchema")]
    pub input_schema: serde_json::Value,
}

/// Distilled signature for a callable (human-readable schema summary)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallableSignature {
    pub required: Vec<String>,
    pub optional: Vec<String>,
    pub constraints: HashMap<String, String>,
    pub examples: Vec<serde_json::Value>,
}

impl CallableSignature {
    /// Generate signature from JSON Schema
    pub fn from_schema(schema: &serde_json::Value) -> Self {
        let mut signature = CallableSignature {
            required: Vec::new(),
            optional: Vec::new(),
            constraints: HashMap::new(),
            examples: Vec::new(),
        };

        if let Some(obj) = schema.as_object() {
            // Extract required fields
            if let Some(required) = obj.get("required").and_then(|r| r.as_array()) {
                signature.required = required
                    .iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect();
            }

            // Extract properties
            if let Some(properties) = obj.get("properties").and_then(|p| p.as_object()) {
                for (key, prop) in properties {
                    let is_required = signature.required.contains(key);

                    if !is_required {
                        signature.optional.push(key.clone());
                    }

                    // Extract constraints
                    if let Some(prop_obj) = prop.as_object() {
                        let mut constraint_parts = Vec::new();

                        if let Some(typ) = prop_obj.get("type").and_then(|t| t.as_str()) {
                            constraint_parts.push(typ.to_string());
                        }

                        if let Some(desc) = prop_obj.get("description").and_then(|d| d.as_str()) {
                            constraint_parts.push(desc.to_string());
                        }

                        if !constraint_parts.is_empty() {
                            signature
                                .constraints
                                .insert(key.clone(), constraint_parts.join("; "));
                        }
                    }
                }
            }

            // Extract examples
            if let Some(examples) = obj.get("examples").and_then(|e| e.as_array()) {
                signature.examples = examples.clone();
            }
        }

        signature
    }
}
