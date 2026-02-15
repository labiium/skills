//! Persistence layer for registry and skill state
//!
//! Provides SQLite-based persistence for:
//! - Callable registry (tools and skills)
//! - Skill metadata and content
//! - Execution history
//! - Server state

use crate::core::{
    CallableId, CallableKind, CallableRecord, CoreError, CostHints, RiskTier, SchemaDigest,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use sqlx::Row;
use std::path::Path;
use std::str::FromStr;
use thiserror::Error;
use tracing::{debug, info};

#[derive(Error, Debug)]
pub enum PersistenceError {
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Invalid data: {0}")]
    InvalidData(String),
}

pub type Result<T> = std::result::Result<T, PersistenceError>;

/// Persistence layer for skills.rs
pub struct PersistenceLayer {
    pool: SqlitePool,
}

impl PersistenceLayer {
    /// Create a new persistence layer with the given database path
    pub async fn new(db_path: impl AsRef<Path>) -> Result<Self> {
        let db_path = db_path.as_ref();

        // Create parent directory if it doesn't exist
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                PersistenceError::InvalidData(format!("Failed to create directory: {}", e))
            })?;
        }

        let options =
            SqliteConnectOptions::from_str(&format!("sqlite://{}?mode=rwc", db_path.display()))
                .map_err(|e| {
                    PersistenceError::InvalidData(format!("Invalid database path: {}", e))
                })?
                .create_if_missing(true);

        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(options)
            .await?;

        let layer = PersistenceLayer { pool };
        layer.initialize_schema().await?;

        info!("Persistence layer initialized at: {:?}", db_path);
        Ok(layer)
    }

    /// Initialize database schema
    async fn initialize_schema(&self) -> Result<()> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS callables (
                id TEXT PRIMARY KEY,
                kind TEXT NOT NULL,
                fq_name TEXT NOT NULL,
                name TEXT NOT NULL,
                title TEXT,
                description TEXT,
                tags TEXT NOT NULL,
                input_schema TEXT NOT NULL,
                output_schema TEXT,
                schema_digest TEXT NOT NULL,
                server_alias TEXT,
                upstream_tool_name TEXT,
                skill_version TEXT,
                uses_tools TEXT NOT NULL,
                skill_directory TEXT,
                bundled_tools TEXT NOT NULL,
                additional_files TEXT NOT NULL,
                cost_hints TEXT NOT NULL,
                risk_tier TEXT NOT NULL,
                last_seen INTEGER NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS idx_callables_kind ON callables(kind)
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS idx_callables_name ON callables(name)
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS execution_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                execution_id TEXT NOT NULL,
                callable_id TEXT NOT NULL,
                arguments TEXT NOT NULL,
                result TEXT,
                is_error INTEGER NOT NULL,
                duration_ms INTEGER,
                started_at INTEGER NOT NULL,
                completed_at INTEGER,
                trace TEXT
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS idx_execution_callable ON execution_history(callable_id)
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS idx_execution_time ON execution_history(started_at)
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS server_state (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                updated_at INTEGER NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        debug!("Database schema initialized");
        Ok(())
    }

    /// Save a callable record
    pub async fn save_callable(&self, record: &CallableRecord) -> Result<()> {
        let now = Utc::now().timestamp();

        let tags_json = serde_json::to_string(&record.tags)?;
        let input_schema_json = serde_json::to_string(&record.input_schema)?;
        let output_schema_json = record
            .output_schema
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        let uses_json =
            serde_json::to_string(&record.uses.iter().map(|id| id.as_str()).collect::<Vec<_>>())?;
        let bundled_tools_json = serde_json::to_string(&record.bundled_tools)?;
        let additional_files_json = serde_json::to_string(&record.additional_files)?;
        let cost_hints_json = serde_json::to_string(&record.cost_hints)?;

        sqlx::query(
            r#"
            INSERT OR REPLACE INTO callables (
                id, kind, fq_name, name, title, description, tags,
                input_schema, output_schema, schema_digest,
                server_alias, upstream_tool_name, skill_version, uses_tools,
                skill_directory, bundled_tools, additional_files,
                cost_hints, risk_tier, last_seen, created_at, updated_at
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20,
                COALESCE((SELECT created_at FROM callables WHERE id = ?1), ?21),
                ?21
            )
            "#,
        )
        .bind(record.id.as_str())
        .bind(format!("{:?}", record.kind))
        .bind(&record.fq_name)
        .bind(&record.name)
        .bind(&record.title)
        .bind(&record.description)
        .bind(tags_json)
        .bind(input_schema_json)
        .bind(output_schema_json)
        .bind(record.schema_digest.as_str())
        .bind(&record.server_alias)
        .bind(&record.upstream_tool_name)
        .bind(&record.skill_version)
        .bind(uses_json)
        .bind(record.skill_directory.as_ref().map(|p| p.to_string_lossy().to_string()))
        .bind(bundled_tools_json)
        .bind(additional_files_json)
        .bind(cost_hints_json)
        .bind(record.risk_tier.to_string())
        .bind(record.last_seen.timestamp())
        .bind(now)
        .execute(&self.pool)
        .await?;

        debug!("Saved callable: {}", record.id.as_str());
        Ok(())
    }

    /// Load a callable by ID
    pub async fn load_callable(&self, id: &CallableId) -> Result<CallableRecord> {
        let row = sqlx::query(
            r#"
            SELECT * FROM callables WHERE id = ?1
            "#,
        )
        .bind(id.as_str())
        .fetch_one(&self.pool)
        .await?;

        self.row_to_callable(row)
    }

    /// Load all callables
    pub async fn load_all_callables(&self) -> Result<Vec<CallableRecord>> {
        let rows = sqlx::query(
            r#"
            SELECT * FROM callables ORDER BY name
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|row| self.row_to_callable(row))
            .collect()
    }

    /// Load callables by kind
    pub async fn load_callables_by_kind(&self, kind: CallableKind) -> Result<Vec<CallableRecord>> {
        let rows = sqlx::query(
            r#"
            SELECT * FROM callables WHERE kind = ?1 ORDER BY name
            "#,
        )
        .bind(format!("{:?}", kind))
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|row| self.row_to_callable(row))
            .collect()
    }

    /// Delete a callable
    pub async fn delete_callable(&self, id: &CallableId) -> Result<()> {
        sqlx::query(
            r#"
            DELETE FROM callables WHERE id = ?1
            "#,
        )
        .bind(id.as_str())
        .execute(&self.pool)
        .await?;

        debug!("Deleted callable: {}", id.as_str());
        Ok(())
    }

    /// Convert database row to CallableRecord
    fn row_to_callable(&self, row: sqlx::sqlite::SqliteRow) -> Result<CallableRecord> {
        let id_str: String = row.get("id");
        let kind_str: String = row.get("kind");
        let tags_json: String = row.get("tags");
        let input_schema_json: String = row.get("input_schema");
        let output_schema_json: Option<String> = row.get("output_schema");
        let schema_digest_str: String = row.get("schema_digest");
        let uses_json: String = row.get("uses_tools");
        let bundled_tools_json: String = row.get("bundled_tools");
        let additional_files_json: String = row.get("additional_files");
        let cost_hints_json: String = row.get("cost_hints");
        let risk_tier_str: String = row.get("risk_tier");
        let last_seen_ts: i64 = row.get("last_seen");

        let kind = match kind_str.as_str() {
            "Tool" => CallableKind::Tool,
            "Skill" => CallableKind::Skill,
            _ => {
                return Err(PersistenceError::InvalidData(format!(
                    "Invalid kind: {}",
                    kind_str
                )))
            }
        };

        let tags: Vec<String> = serde_json::from_str(&tags_json)?;
        let input_schema: serde_json::Value = serde_json::from_str(&input_schema_json)?;
        let output_schema: Option<serde_json::Value> = output_schema_json
            .map(|s| serde_json::from_str(&s))
            .transpose()?;
        let uses_ids: Vec<String> = serde_json::from_str(&uses_json)?;
        let uses: Vec<CallableId> = uses_ids.into_iter().map(CallableId::from).collect();
        let bundled_tools: Vec<crate::BundledTool> = serde_json::from_str(&bundled_tools_json)?;
        let additional_files: Vec<String> = serde_json::from_str(&additional_files_json)?;
        let cost_hints: CostHints = serde_json::from_str(&cost_hints_json)?;
        let risk_tier: RiskTier = risk_tier_str
            .parse()
            .map_err(|e: CoreError| PersistenceError::InvalidData(e.to_string()))?;

        let skill_directory: Option<String> = row.get("skill_directory");

        Ok(CallableRecord {
            id: CallableId::from(id_str),
            kind,
            fq_name: row.get("fq_name"),
            name: row.get("name"),
            title: row.get("title"),
            description: row.get("description"),
            tags,
            input_schema,
            output_schema,
            schema_digest: SchemaDigest::from(schema_digest_str),
            server_alias: row.get("server_alias"),
            upstream_tool_name: row.get("upstream_tool_name"),
            skill_version: row.get("skill_version"),
            uses,
            skill_directory: skill_directory.map(std::path::PathBuf::from),
            bundled_tools,
            additional_files,
            cost_hints,
            risk_tier,
            last_seen: DateTime::from_timestamp(last_seen_ts, 0)
                .ok_or_else(|| PersistenceError::InvalidData("Invalid timestamp".to_string()))?,
            sandbox_config: None,
        })
    }

    /// Record execution history
    #[allow(clippy::too_many_arguments)]
    pub async fn record_execution(
        &self,
        execution_id: &str,
        callable_id: &CallableId,
        arguments: &serde_json::Value,
        result: Option<&serde_json::Value>,
        is_error: bool,
        duration_ms: Option<u64>,
        started_at: DateTime<Utc>,
        completed_at: Option<DateTime<Utc>>,
        trace: Option<&serde_json::Value>,
    ) -> Result<()> {
        let arguments_json = serde_json::to_string(arguments)?;
        let result_json = result.map(serde_json::to_string).transpose()?;
        let trace_json = trace.map(serde_json::to_string).transpose()?;

        sqlx::query(
            r#"
            INSERT INTO execution_history (
                execution_id, callable_id, arguments, result, is_error,
                duration_ms, started_at, completed_at, trace
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            "#,
        )
        .bind(execution_id)
        .bind(callable_id.as_str())
        .bind(arguments_json)
        .bind(result_json)
        .bind(is_error as i32)
        .bind(duration_ms.map(|d| d as i64))
        .bind(started_at.timestamp())
        .bind(completed_at.map(|t| t.timestamp()))
        .bind(trace_json)
        .execute(&self.pool)
        .await?;

        debug!("Recorded execution: {}", execution_id);
        Ok(())
    }

    /// Get execution history for a callable
    pub async fn get_execution_history(
        &self,
        callable_id: &CallableId,
        limit: i64,
    ) -> Result<Vec<ExecutionRecord>> {
        let rows = sqlx::query(
            r#"
            SELECT * FROM execution_history
            WHERE callable_id = ?1
            ORDER BY started_at DESC
            LIMIT ?2
            "#,
        )
        .bind(callable_id.as_str())
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|row| {
                let started_at_ts: i64 = row.get("started_at");
                let completed_at_ts: Option<i64> = row.get("completed_at");

                Ok(ExecutionRecord {
                    id: row.get("id"),
                    execution_id: row.get("execution_id"),
                    callable_id: row.get("callable_id"),
                    arguments: serde_json::from_str(&row.get::<String, _>("arguments"))?,
                    result: row
                        .get::<Option<String>, _>("result")
                        .map(|s| serde_json::from_str(&s))
                        .transpose()?,
                    is_error: row.get::<i32, _>("is_error") != 0,
                    duration_ms: row.get::<Option<i64>, _>("duration_ms").map(|d| d as u64),
                    started_at: DateTime::from_timestamp(started_at_ts, 0).ok_or_else(|| {
                        PersistenceError::InvalidData("Invalid timestamp".to_string())
                    })?,
                    completed_at: completed_at_ts.and_then(|ts| DateTime::from_timestamp(ts, 0)),
                    trace: row
                        .get::<Option<String>, _>("trace")
                        .map(|s| serde_json::from_str(&s))
                        .transpose()?,
                })
            })
            .collect()
    }

    /// Save server state
    pub async fn save_state(&self, key: &str, value: &serde_json::Value) -> Result<()> {
        let value_json = serde_json::to_string(value)?;
        let now = Utc::now().timestamp();

        sqlx::query(
            r#"
            INSERT OR REPLACE INTO server_state (key, value, updated_at)
            VALUES (?1, ?2, ?3)
            "#,
        )
        .bind(key)
        .bind(value_json)
        .bind(now)
        .execute(&self.pool)
        .await?;

        debug!("Saved state: {}", key);
        Ok(())
    }

    /// Load server state
    pub async fn load_state(&self, key: &str) -> Result<serde_json::Value> {
        let row = sqlx::query(
            r#"
            SELECT value FROM server_state WHERE key = ?1
            "#,
        )
        .bind(key)
        .fetch_one(&self.pool)
        .await?;

        let value_json: String = row.get("value");
        Ok(serde_json::from_str(&value_json)?)
    }

    /// Prune old execution history
    pub async fn prune_execution_history(&self, older_than_days: i64) -> Result<u64> {
        let cutoff = Utc::now().timestamp() - (older_than_days * 86400);

        let result = sqlx::query(
            r#"
            DELETE FROM execution_history WHERE started_at < ?1
            "#,
        )
        .bind(cutoff)
        .execute(&self.pool)
        .await?;

        let deleted = result.rows_affected();
        info!("Pruned {} old execution records", deleted);
        Ok(deleted)
    }

    /// Get database statistics
    pub async fn get_stats(&self) -> Result<PersistenceStats> {
        let callables_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM callables")
            .fetch_one(&self.pool)
            .await?;

        let tools_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM callables WHERE kind = 'Tool'")
                .fetch_one(&self.pool)
                .await?;

        let skills_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM callables WHERE kind = 'Skill'")
                .fetch_one(&self.pool)
                .await?;

        let executions_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM execution_history")
            .fetch_one(&self.pool)
            .await?;

        Ok(PersistenceStats {
            total_callables: callables_count as usize,
            total_tools: tools_count as usize,
            total_skills: skills_count as usize,
            total_executions: executions_count as usize,
        })
    }

    /// Close the database connection
    pub async fn close(self) {
        self.pool.close().await;
        info!("Persistence layer closed");
    }
}

/// Execution history record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionRecord {
    pub id: i64,
    pub execution_id: String,
    pub callable_id: String,
    pub arguments: serde_json::Value,
    pub result: Option<serde_json::Value>,
    pub is_error: bool,
    pub duration_ms: Option<u64>,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub trace: Option<serde_json::Value>,
}

/// Persistence statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistenceStats {
    pub total_callables: usize,
    pub total_tools: usize,
    pub total_skills: usize,
    pub total_executions: usize,
}
