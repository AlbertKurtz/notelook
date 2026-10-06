use anyhow::{Context, Result};
use turso::{Connection, Row};

use super::models::{ChunkRow, Job, Note, SearchHit};

// ─── helpers ────────────────────────────────────────────────────────────────

pub fn content_hash(text: &str) -> String {
    blake3::hash(text.as_bytes()).to_hex().to_string()
}

/// COMMIT or ROLLBACK
pub(crate) async fn finish_tx<T>(conn: &Connection, result: Result<T>) -> Result<T> {
    match result {
        Ok(value) => {
            conn.execute("COMMIT", ()).await?;
            Ok(value)
        }
        Err(e) => {
            // If rollback fails too, the original error is the interesting one.
            let _ = conn.execute("ROLLBACK", ()).await;
            Err(e)
        }
    }
}

const NOTE_COLUMNS: &str = "id, created_at, updated_at, event_at, context, content";

fn note_from_row(row: &Row) -> Result<Note> {
    Ok(Note {
        id: row.get(0)?,
        created_at: row.get(1)?,
        updated_at: row.get(2)?,
        event_at: row.get(3)?,
        context: row.get(4)?,
        content: row.get(5)?,
    })
}

// ─── meta ───────────────────────────────────────────────────────────────────

pub(crate) async fn get_meta(conn: &Connection, key: &str) -> Result<Option<String>> {
    let mut rows = conn
        .query("SELECT value FROM meta WHERE key = ?1", (key,))
        .await?;
    match rows.next().await? {
        Some(row) => Ok(Some(row.get(0)?)),
        None => Ok(None),
    }
}

pub(crate) async fn set_meta(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)",
        (key, value),
    )
    .await?;
    Ok(())
}

// ─── notes ──────────────────────────────────────────────────────────────────

/// Inserts a note and enqueues its embedding job.
pub async fn insert_note(conn: &Connection, content: &str) -> Result<i64> {
    let hash = content_hash(content);

    conn.execute("BEGIN", ()).await?;
    let result: Result<i64> = async {
        let id: i64 = {
            let mut rows = conn
                .query(
                    "INSERT INTO notes (content, content_hash) VALUES (?1, ?2) RETURNING id",
                    (content, hash.as_str()),
                )
                .await?;
            rows.next()
                .await?
                .context("INSERT ... RETURNING produced no row")?
                .get(0)?
        };
        enqueue_job(conn, "embed", id).await?;
        Ok(id)
    }
    .await;
    finish_tx(conn, result).await
}

/// Updates a note's content. Returns `false` if the content didn't change
pub async fn update_note(conn: &Connection, id: i64, content: &str) -> Result<bool> {
    let new_hash = content_hash(content);

    conn.execute("BEGIN", ()).await?;
    let result: Result<bool> = async {
        let old_hash: String = {
            let mut rows = conn
                .query("SELECT content_hash FROM notes WHERE id = ?1", (id,))
                .await?;
            rows.next()
                .await?
                .with_context(|| format!("note {id} not found"))?
                .get(0)?
        };

        if old_hash == new_hash {
            return Ok(false);
        }

        conn.execute(
            "UPDATE notes
             SET content = ?1, content_hash = ?2, updated_at = datetime('now')
             WHERE id = ?3",
            (content, new_hash.as_str(), id),
        )
        .await?;
        enqueue_job(conn, "embed", id).await?;
        Ok(true)
    }
    .await;
    finish_tx(conn, result).await
}

pub async fn get_note(conn: &Connection, id: i64) -> Result<Option<Note>> {
    let sql = format!("SELECT {NOTE_COLUMNS} FROM notes WHERE id = ?1");
    let mut rows = conn.query(&sql, (id,)).await?;
    match rows.next().await? {
        Some(row) => Ok(Some(note_from_row(&row)?)),
        None => Ok(None),
    }
}

/// Most recently edited first.
pub async fn recent_notes(conn: &Connection, limit: i64) -> Result<Vec<Note>> {
    let sql =
        format!("SELECT {NOTE_COLUMNS} FROM notes ORDER BY updated_at DESC, id DESC LIMIT ?1");
    let mut rows = conn.query(&sql, (limit,)).await?;
    let mut notes = Vec::new();
    while let Some(row) = rows.next().await? {
        notes.push(note_from_row(&row)?);
    }
    Ok(notes)
}

/// Chunks, labels and jobs are removed by ON DELETE CASCADE.
pub async fn delete_note(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM notes WHERE id = ?1", (id,))
        .await?;
    Ok(())
}

pub async fn add_label(conn: &Connection, note_id: i64, label: &str) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO labels (note_id, label) VALUES (?1, ?2)",
        (note_id, label),
    )
    .await?;
    Ok(())
}

// ─── search ─────────────────────────────────────────────────────────────────

pub async fn search_chunks(
    conn: &Connection,
    query_vec: &[f32],
    limit: i64,
) -> Result<Vec<SearchHit>> {
    let query_json = serde_json::to_string(query_vec)?;
    let mut rows = conn
        .query(
            "SELECT c.note_id, c.chunk_index, c.byte_start, c.content, n.created_at,
                    vector_distance_cos(c.embedding, vector(?1)) AS distance
             FROM chunks c
             JOIN notes n ON n.id = c.note_id
             WHERE c.embedding IS NOT NULL
             ORDER BY distance ASC
             LIMIT ?2",
            (query_json, limit),
        )
        .await?;

    let mut hits = Vec::new();
    while let Some(row) = rows.next().await? {
        hits.push(SearchHit {
            note_id: row.get(0)?,
            chunk_index: row.get(1)?,
            byte_start: row.get(2)?,
            content: row.get(3)?,
            note_created_at: row.get(4)?,
            distance: row.get(5)?,
        });
    }
    Ok(hits)
}

// ─── jobs ───────────────────────────────────────────────────────────────────

/// REPLACE gives the row a new id. A worker still processing the old job will
/// see its id gone and discard its (stale) result. See `complete_embed_job`.
async fn enqueue_job(conn: &Connection, kind: &str, note_id: i64) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO jobs (kind, note_id, status, attempts)
         VALUES (?1, ?2, 'pending', 0)",
        (kind, note_id),
    )
    .await?;
    Ok(())
}

/// Picks the oldest pending job and marks it running.
/// SELECT-then-UPDATE is only safe because the actor runs one call at a time.
pub async fn claim_next_job(conn: &Connection, kind: &str) -> Result<Option<Job>> {
    let job = {
        let mut rows = conn
            .query(
                "SELECT id, note_id, attempts FROM jobs
                 WHERE kind = ?1 AND status = 'pending'
                 ORDER BY id LIMIT 1",
                (kind,),
            )
            .await?;
        match rows.next().await? {
            Some(row) => Job {
                id: row.get(0)?,
                note_id: row.get(1)?,
                attempts: row.get(2)?,
            },
            None => return Ok(None),
        }
    };

    conn.execute(
        "UPDATE jobs SET status = 'running', attempts = attempts + 1 WHERE id = ?1",
        (job.id,),
    )
    .await?;
    Ok(Some(job))
}

/// Replaces the note's chunks and deletes the job, atomically.
/// Returns `false` if the job was superseded by a newer save (nothing written).
pub async fn complete_embed_job(
    conn: &Connection,
    job_id: i64,
    note_id: i64,
    chunks: &[ChunkRow],
) -> Result<bool> {
    conn.execute("BEGIN", ()).await?;
    let result: Result<bool> = async {
        let still_current = {
            let mut rows = conn
                .query(
                    "SELECT 1 FROM jobs WHERE id = ?1 AND status = 'running'",
                    (job_id,),
                )
                .await?;
            rows.next().await?.is_some()
        };
        if !still_current {
            return Ok(false);
        }

        conn.execute("DELETE FROM chunks WHERE note_id = ?1", (note_id,))
            .await?;

        for c in chunks {
            let vector_json = serde_json::to_string(&c.embedding)?;
            conn.execute(
                "INSERT INTO chunks (note_id, chunk_index, byte_start, content, embedding)
                 VALUES (?1, ?2, ?3, ?4, vector(?5))",
                (
                    note_id,
                    c.index,
                    c.byte_start,
                    c.content.as_str(),
                    vector_json,
                ),
            )
            .await?;
        }

        conn.execute("DELETE FROM jobs WHERE id = ?1", (job_id,))
            .await?;
        Ok(true)
    }
    .await;
    finish_tx(conn, result).await
}

/// Back to 'pending' for a retry, or 'failed' after too many attempts.
pub async fn fail_job(
    conn: &Connection,
    job_id: i64,
    error: &str,
    max_attempts: i64,
) -> Result<()> {
    conn.execute(
        "UPDATE jobs
         SET status = CASE WHEN attempts >= ?3 THEN 'failed' ELSE 'pending' END,
             last_error = ?2
         WHERE id = ?1",
        (job_id, error, max_attempts),
    )
    .await?;
    Ok(())
}

/// Called at startup: jobs left 'running' were interrupted by a crash or quit.
pub async fn reset_running_jobs(conn: &Connection) -> Result<()> {
    conn.execute(
        "UPDATE jobs SET status = 'pending' WHERE status = 'running'",
        (),
    )
    .await?;
    Ok(())
}

/// If the stored model id differs from `model_id`, store the new one and
/// enqueue every note for re-embedding. Returns true if a re-embed was queued.
pub async fn ensure_embedding_model(conn: &Connection, model_id: &str) -> Result<bool> {
    if get_meta(conn, "embedding_model").await?.as_deref() == Some(model_id) {
        return Ok(false);
    }

    conn.execute("BEGIN", ()).await?;
    let result: Result<bool> = async {
        set_meta(conn, "embedding_model", model_id).await?;
        conn.execute(
            "INSERT OR REPLACE INTO jobs (kind, note_id, status, attempts)
             SELECT 'embed', id, 'pending', 0 FROM notes",
            (),
        )
        .await?;
        Ok(true)
    }
    .await;
    finish_tx(conn, result).await
}
