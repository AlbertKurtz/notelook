use anyhow::{Context, Result};
use turso::Connection;

use super::repo::{finish_tx, get_meta, set_meta};

/// Each entry is one schema version; each version is a list of statements.
const MIGRATIONS: &[&[&str]] = &[
    // ── v1 ──────────────────────────────────────────────
    &[
        "CREATE TABLE IF NOT EXISTS notes (
            id           INTEGER PRIMARY KEY AUTOINCREMENT,
            created_at   TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at   TEXT NOT NULL DEFAULT (datetime('now')),
            event_at     TEXT,
            context      TEXT,
            content      TEXT NOT NULL,
            content_hash TEXT NOT NULL
        )",
        "CREATE TABLE IF NOT EXISTS labels (
            id      INTEGER PRIMARY KEY AUTOINCREMENT,
            note_id INTEGER NOT NULL REFERENCES notes(id) ON DELETE CASCADE,
            label   TEXT NOT NULL,
            UNIQUE(note_id, label)
        )",
        "CREATE TABLE IF NOT EXISTS chunks (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            note_id     INTEGER NOT NULL REFERENCES notes(id) ON DELETE CASCADE,
            chunk_index INTEGER NOT NULL,
            byte_start  INTEGER NOT NULL,
            content     TEXT NOT NULL,
            embedding   F32_BLOB(384),
            UNIQUE(note_id, chunk_index)
        )",
        "CREATE INDEX IF NOT EXISTS idx_chunks_note_id ON chunks(note_id)",
        "CREATE INDEX IF NOT EXISTS idx_chunks_vec ON chunks(libsql_vector_idx(embedding, 'metric=cosine'))",
    ],
    // ── v2 (vector index for databases created under v1) ─
    &[
        "CREATE INDEX IF NOT EXISTS idx_chunks_vec ON chunks(libsql_vector_idx(embedding, 'metric=cosine'))",
    ],
    // ── v3 (Native Turso Full-Text Search on chunks) ────
    &[
        "CREATE INDEX IF NOT EXISTS idx_chunks_fts ON chunks USING fts (content)",
    ],
];

pub async fn migrate(conn: &Connection) -> Result<()> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
        (),
    )
    .await?;

    let current: i64 = match get_meta(conn, "schema_version").await? {
        Some(v) => v
            .parse()
            .context("schema_version in meta is not a number")?,
        None => 0,
    };

    for (i, statements) in MIGRATIONS.iter().enumerate() {
        let version = i as i64 + 1;
        if version <= current {
            continue;
        }

        conn.execute("BEGIN", ()).await?;
        let result: Result<()> = async {
            for sql in statements.iter() {
                if let Err(e) = conn.execute(sql, ()).await {
                    if sql.contains("libsql_vector_idx") {
                        eprintln!(
                            "Warning: vector index not supported on this libSQL build: {e:#}"
                        );
                        continue;
                    }
                    if sql.contains("USING fts") {
                        eprintln!(
                            "Warning: native FTS index not supported on this build: {e:#}"
                        );
                        continue;
                    }
                    return Err(e)
                        .with_context(|| format!("migration v{version} failed on:\n{sql}"));
                }
            }
            set_meta(conn, "schema_version", &version.to_string()).await
        }
        .await;
        finish_tx(conn, result).await?;
    }

    Ok(())
}
