pub mod models;
pub mod repo;
mod schema;

pub use models::*;

use anyhow::{Context, Result};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;
use turso::{Builder, Connection};

/// Ergonomic database handle. Cheap to clone and share across async tasks.
#[derive(Debug, Clone)]
pub struct Db {
    conn: Connection,
    tx_lock : Arc<Mutex<()>>
}

impl Db {
    /// Opens (or creates) the database and applies migrations.
    /// Pass `Path::new(":memory:")` for integration tests.
    pub async fn open(path: &Path) -> Result<Self> {
        let path_str = path.to_str().context("database path is not valid UTF-8")?;
        let db = Builder::new_local(path_str)
            .experimental_index_method(true)
            .build()
            .await
            .with_context(|| format!("failed to open database at {path_str}"))?;
        let conn = db.connect()?;

        conn.execute("PRAGMA foreign_keys = ON", ()).await?;
        let _ = conn.execute("PRAGMA journal_mode = WAL", ()).await;
        let _ = conn.execute("PRAGMA busy_timeout = 5000", ()).await;
        schema::migrate(&conn).await?;
        Ok(Self {
            conn,
            tx_lock: Arc::new(Mutex::new(())),
        })
    }

    /// Access the underlying connection if needed.
    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    pub async fn insert_note(&self, content: &str) -> Result<i64> {
        repo::insert_note(&self.conn, content).await
    }

    pub async fn update_note(&self, id: i64, content: &str) -> Result<bool> {
        repo::update_note(&self.conn, id, content).await
    }

    pub async fn note_hash_matches(&self, note_id: i64, expected_hash: &str) -> Result<bool> {
        repo::note_hash_matches(&self.conn, note_id, expected_hash).await
    }

    pub async fn get_note(&self, id: i64) -> Result<Option<Note>> {
        repo::get_note(&self.conn, id).await
    }

    pub async fn recent_notes(&self, limit: i64) -> Result<Vec<Note>> {
        repo::recent_notes(&self.conn, limit).await
    }

    pub async fn delete_note(&self, id: i64) -> Result<()> {
        repo::delete_note(&self.conn, id).await
    }

    pub async fn search_hybrid(
        &self,
        query: &str,
        query_vec: &[f32],
        limit: i64,
    ) -> Result<Vec<SearchHit>> {
        repo::search_hybrid(&self.conn, query, query_vec, limit).await
    }

    pub async fn search_lexical(&self, query: &str, limit: i64) -> Result<Vec<SearchHit>> {
        repo::search_lexical(&self.conn, query, limit).await
    }

    pub async fn search_semantic(&self, query_vec: &[f32], limit: i64) -> Result<Vec<SearchHit>> {
        repo::search_chunks(&self.conn, query_vec, limit).await
    }

    pub async fn save_chunks(&self, note_id: i64, chunks: &[ChunkRow]) -> Result<()> {
        let _guard = self.tx_lock.lock().await;
        repo::save_chunks(&self.conn, note_id, chunks).await
    }

    pub async fn unindexed_notes(&self) -> Result<Vec<(i64, String)>> {
        repo::unindexed_notes(&self.conn).await
    }
}
