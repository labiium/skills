//! Search and indexing engine for callables
//!
//! Provides fast discovery over the unified registry with:
//! - Multiple query modes (literal, regex, fuzzy)
//! - Field-weighted ranking
//! - Filters (kind, server, tags, requires)
//! - Pagination support

use crate::core::registry::Registry;
use crate::core::{CallableId, CallableKind, CallableRecord};
use parking_lot::RwLock;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use thiserror::Error;
use tracing::debug;

const SEARCHABLE_BODY_CHAR_LIMIT: usize = 50_000;

#[derive(Error, Debug)]
pub enum IndexError {
    #[error("Invalid query: {0}")]
    InvalidQuery(String),

    #[error("Regex compilation failed: {0}")]
    RegexError(#[from] regex::Error),

    #[error("Index error: {0}")]
    Internal(String),
}

pub type Result<T> = std::result::Result<T, IndexError>;

/// Search filters
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct SearchFilters {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub requires: Option<Vec<String>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub capability: Option<Vec<String>>,
}

/// Search query
#[derive(Debug, Clone)]
pub struct SearchQuery {
    pub q: String,
    pub kind: String, // "any", "tools", "skills"
    pub limit: usize,
    pub filters: Option<SearchFilters>,
    pub cursor: Option<String>,
}

/// Search match result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchMatch {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub fq_name: String,
    pub server: Option<String>,
    pub description_snippet: String,
    pub inputs: Vec<String>,
    pub score: f64,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature_short: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema_digest: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub uses: Option<Vec<String>>,
}

/// Search results
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResults {
    pub matches: Vec<SearchMatch>,
    pub total_matches: usize,
    pub next_cursor: Option<String>,
}

/// In-memory search engine
pub struct SearchEngine {
    registry: Arc<Registry>,
    index: Arc<RwLock<InMemoryIndex>>,
}

impl SearchEngine {
    pub fn new(registry: Arc<Registry>) -> Self {
        SearchEngine {
            registry,
            index: Arc::new(RwLock::new(InMemoryIndex::new())),
        }
    }

    /// Rebuild index from registry
    pub fn rebuild(&self) {
        debug!("Rebuilding search index");
        let callables = self.registry.all();
        let mut index = self.index.write();
        index.clear();

        for record in callables {
            index.add_record(&record);
        }

        debug!("Index rebuilt with {} entries", index.len());
    }

    /// Incremental index update
    pub fn update_record(&self, record: &CallableRecord) {
        let mut index = self.index.write();
        index.add_record(record);
    }

    /// Remove from index
    pub fn remove_record(&self, id: &CallableId) {
        let mut index = self.index.write();
        index.remove_record(id);
    }

    /// Search the index
    pub async fn search(&self, query: &SearchQuery) -> Result<SearchResults> {
        if query.q.is_empty() {
            return Err(IndexError::InvalidQuery(
                "Query cannot be empty".to_string(),
            ));
        }

        debug!("Search query: {:?}", query.q);

        // Detect query intent

        // Get all callables from registry
        let mut candidates = self.registry.all();

        // Apply kind filter
        if query.kind != "any" {
            let target_kind = match query.kind.as_str() {
                "tools" => CallableKind::Tool,
                "skills" => CallableKind::Skill,
                _ => {
                    return Err(IndexError::InvalidQuery(format!(
                        "Invalid kind: {}",
                        query.kind
                    )));
                }
            };
            candidates.retain(|c| c.kind == target_kind);
        }

        // Apply filters
        if let Some(filters) = &query.filters {
            candidates = apply_filters(candidates, filters);
        }

        // Score and rank matches using field-weighted, separator-insensitive matching.
        let mut scored: Vec<(CallableRecord, f64)> = candidates
            .into_iter()
            .filter_map(|record| {
                let body = {
                    let index = self.index.read();
                    index.body_texts.get(record.id.as_str()).cloned()
                };
                let score = score_record(&query.q, &record, body.as_deref());

                if score > 0.0 {
                    Some((record, score))
                } else {
                    None
                }
            })
            .collect();

        // Sort by score descending
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let total_matches = scored.len();

        // Apply pagination
        let offset = query
            .cursor
            .as_ref()
            .and_then(|c| c.parse::<usize>().ok())
            .unwrap_or(0);

        let matches: Vec<SearchMatch> = scored
            .into_iter()
            .skip(offset)
            .take(query.limit)
            .map(|(record, score)| {
                let inputs = extract_input_keys(&record.input_schema);
                let description_snippet = record
                    .description
                    .clone()
                    .unwrap_or_else(|| record.title.clone().unwrap_or_default())
                    .chars()
                    .take(200)
                    .collect();

                SearchMatch {
                    id: record.id.as_str().to_string(),
                    kind: record.kind.to_string(),
                    name: record.name.clone(),
                    fq_name: record.fq_name.clone(),
                    server: record.server_alias.clone(),
                    description_snippet,
                    inputs,
                    score,
                    signature_short: None,
                    schema_digest: Some(record.schema_digest.short().to_string()),
                    uses: if record.kind == CallableKind::Skill {
                        Some(
                            record
                                .uses
                                .iter()
                                .map(|id| id.as_str().to_string())
                                .collect(),
                        )
                    } else {
                        None
                    },
                }
            })
            .collect();

        let next_cursor = if offset + query.limit < total_matches {
            Some((offset + query.limit).to_string())
        } else {
            None
        };

        Ok(SearchResults {
            matches,
            total_matches,
            next_cursor,
        })
    }
}

/// In-memory inverted index
struct InMemoryIndex {
    // Token -> CallableId mappings
    tokens: HashMap<String, HashSet<String>>,
    // CallableId -> tokens for removal
    reverse: HashMap<String, HashSet<String>>,
    // CallableId -> capped SKILL.md body text for low-weight matching
    body_texts: HashMap<String, Arc<str>>,
}

impl InMemoryIndex {
    fn new() -> Self {
        InMemoryIndex {
            tokens: HashMap::new(),
            reverse: HashMap::new(),
            body_texts: HashMap::new(),
        }
    }

    fn add_record(&mut self, record: &CallableRecord) {
        let id = record.id.as_str().to_string();
        let body = load_searchable_body(record);
        let tokens = tokenize_record(record, body.as_deref());

        for token in &tokens {
            self.tokens
                .entry(token.clone())
                .or_default()
                .insert(id.clone());
        }

        if let Some(body) = body {
            self.body_texts.insert(id.clone(), Arc::from(body));
        } else {
            self.body_texts.remove(&id);
        }

        self.reverse.insert(id, tokens);
    }

    fn remove_record(&mut self, id: &CallableId) {
        let id_str = id.as_str();
        if let Some(tokens) = self.reverse.remove(id_str) {
            for token in tokens {
                if let Some(ids) = self.tokens.get_mut(&token) {
                    ids.remove(id_str);
                }
            }
        }
        self.body_texts.remove(id_str);
    }

    fn clear(&mut self) {
        self.tokens.clear();
        self.reverse.clear();
        self.body_texts.clear();
    }

    fn len(&self) -> usize {
        self.reverse.len()
    }
}

/// Tokenize a record for indexing
fn tokenize_record(record: &CallableRecord, body: Option<&str>) -> HashSet<String> {
    let mut tokens = HashSet::new();

    // Tokenize name
    for token in tokenize(&record.name) {
        tokens.insert(token);
    }

    // Tokenize FQ name
    for token in tokenize(&record.fq_name) {
        tokens.insert(token);
    }

    // Tokenize title
    if let Some(title) = &record.title {
        for token in tokenize(title) {
            tokens.insert(token);
        }
    }

    // Tokenize description
    if let Some(desc) = &record.description {
        for token in tokenize(desc) {
            tokens.insert(token);
        }
    }

    if let Some(body) = body {
        for token in tokenize(body) {
            tokens.insert(token);
        }
    }

    // Add tags
    for tag in &record.tags {
        tokens.insert(tag.to_lowercase());
    }

    tokens
}

/// Simple tokenization (lowercase, split on non-alphanumeric including underscores)
fn tokenize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect()
}

fn strip_frontmatter(content: &str) -> String {
    let normalized = content
        .strip_prefix('\u{feff}')
        .unwrap_or(content)
        .replace("\r\n", "\n");

    if let Some(after_first) = normalized.strip_prefix("---\n") {
        if let Some(end_pos) = after_first.find("\n---\n") {
            return after_first[end_pos + "\n---\n".len()..].trim().to_string();
        }
    }

    normalized.trim().to_string()
}

fn cap_text(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

fn load_searchable_body(record: &CallableRecord) -> Option<String> {
    if record.kind != CallableKind::Skill {
        return None;
    }

    let skill_dir = record.skill_directory.as_ref()?;
    let skill_md = std::fs::read_to_string(skill_dir.join("SKILL.md")).ok()?;
    let body = strip_frontmatter(&skill_md);

    if body.is_empty() {
        None
    } else {
        Some(cap_text(&body, SEARCHABLE_BODY_CHAR_LIMIT))
    }
}

fn compact(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect()
}

fn score_field(query: &str, query_tokens: &[String], text: &str, weight: f64) -> f64 {
    let text_lower = text.to_lowercase();
    let text_tokens = tokenize(text);
    let text_compact = compact(text);
    let query_compact = compact(query);
    let mut score = 0.0;

    if text_lower == query.to_lowercase() {
        score += weight * 3.0;
    } else if text_lower.contains(&query.to_lowercase()) {
        score += weight * 2.0;
    }

    if !query_compact.is_empty() && text_compact.contains(&query_compact) {
        score += weight * 1.75;
    }

    let mut matched_tokens = 0;
    for query_token in query_tokens {
        let query_token_compact = compact(query_token);
        if query_token_compact.is_empty() {
            continue;
        }

        let matched = text_tokens.iter().any(|text_token| {
            text_token == query_token
                || text_token.starts_with(query_token)
                || query_token.starts_with(text_token)
        }) || text_compact.contains(&query_token_compact);

        if matched {
            matched_tokens += 1;
            score += weight;
        }
    }

    if !query_tokens.is_empty() && matched_tokens == query_tokens.len() {
        score += weight * 1.25;
    }

    score
}

/// Score a callable with forgiving discovery semantics:
/// - case-insensitive matching
/// - separator-insensitive matching (`pytestlab core` -> `pytestlab-core`)
/// - per-token matching so partial intent still finds plausible callables
/// - field weighting so names outrank incidental description mentions
fn score_record(query: &str, record: &CallableRecord, body: Option<&str>) -> f64 {
    let query_tokens = tokenize(query);
    let mut score = 0.0;

    score += score_field(query, &query_tokens, &record.name, 30.0);
    score += score_field(query, &query_tokens, &record.fq_name, 25.0);

    if let Some(title) = &record.title {
        score += score_field(query, &query_tokens, title, 15.0);
    }

    if let Some(desc) = &record.description {
        score += score_field(query, &query_tokens, desc, 8.0);
    }

    for tag in &record.tags {
        score += score_field(query, &query_tokens, tag, 12.0);
    }

    if let Some(body) = body {
        score += score_field(query, &query_tokens, body, 3.0);
    }

    score
}

/// Apply filters to candidates
fn apply_filters(candidates: Vec<CallableRecord>, filters: &SearchFilters) -> Vec<CallableRecord> {
    let mut filtered = candidates;

    // Server filter (tools only)
    if let Some(server) = &filters.server {
        filtered.retain(|c| {
            c.server_alias
                .as_ref()
                .map(|s| s == server)
                .unwrap_or(false)
        });
    }

    // Tags filter (any match)
    if let Some(tags) = &filters.tags {
        if !tags.is_empty() {
            filtered.retain(|c| tags.iter().any(|tag| c.tags.contains(tag)));
        }
    }

    // Requires filter (schema must have all keys)
    if let Some(requires) = &filters.requires {
        if !requires.is_empty() {
            filtered.retain(|c| {
                let keys = extract_input_keys(&c.input_schema);
                requires.iter().all(|req| keys.contains(req))
            });
        }
    }

    // Capability filter (tags-based capability matching)
    if let Some(capability) = &filters.capability {
        if !capability.is_empty() {
            filtered.retain(|c| capability.iter().any(|cap| c.tags.contains(cap)));
        }
    }

    filtered
}

/// Extract top-level input keys from JSON schema
fn extract_input_keys(schema: &serde_json::Value) -> Vec<String> {
    if let Some(properties) = schema.get("properties").and_then(|p| p.as_object()) {
        properties.keys().cloned().collect()
    } else {
        vec![]
    }
}
