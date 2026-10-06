use anyhow::{Context, Result};
use turso::Connection;

use super::repo::{finish_tx, get_meta, set_meta};

/// Each entry is one schema version; each version is a list of statements.
/// Index 0 = version 1. Append only, never edit.
const MIGRATIONS: &[&[&str]] = &[
    // ── v1 ──────────────────────────────────────────────
    &[
        "CREATE TABLE notes (
            id           INTEGER PRIMARY KEY AUTOINCREMENT,
            created_at   TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at   TEXT NOT NULL DEFAULT (datetime('now')),
            event_at     TEXT,
            context      TEXT,
            content      TEXT NOT NULL,
            content_hash TEXT NOT NULL
        )",
        "CREATE TABLE labels (
            id      INTEGER PRIMARY KEY AUTOINCREMENT,
            note_id INTEGER NOT NULL REFERENCES notes(id) ON DELETE CASCADE,
            label   TEXT NOT NULL,
            UNIQUE(note_id, label)
        )",
        // Keep 384 in sync with config::EMBED_DIM
        "CREATE TABLE chunks (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            note_id     INTEGER NOT NULL REFERENCES notes(id) ON DELETE CASCADE,
            chunk_index INTEGER NOT NULL,
            byte_start  INTEGER NOT NULL,
            content     TEXT NOT NULL,
            embedding   F32_BLOB(384),
            UNIQUE(note_id, chunk_index)
        )",
        "CREATE INDEX idx_chunks_note_id ON chunks(note_id)",
        // One job per (kind, note). Re-saving a note replaces its job.
        "CREATE TABLE jobs (
            id         INTEGER PRIMARY KEY AUTOINCREMENT,
            kind       TEXT NOT NULL,
            note_id    INTEGER NOT NULL REFERENCES notes(id) ON DELETE CASCADE,
            status     TEXT NOT NULL DEFAULT 'pending',
            attempts   INTEGER NOT NULL DEFAULT 0,
            last_error TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(kind, note_id)
        )",
        "CREATE INDEX idx_jobs_status ON jobs(kind, status)",
    ],
    // ── v2 goes here, e.g. entities/mentions tables ─────
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
                conn.execute(sql, ())
                    .await
                    .with_context(|| format!("migration v{version} failed on:\n{sql}"))?;
            }
            set_meta(conn, "schema_version", &version.to_string()).await
        }
        .await;
        finish_tx(conn, result).await?;
    }

    Ok(())
}
