use anyhow::{Context, Result};
use turso::{Connection, Row};

use super::models::{ChunkRow, Note, SearchHit};

// ─── helpers ────────────────────────────────────────────────────────────────

pub fn content_hash(text: &str) -> String {
    blake3::hash(text.as_bytes()).to_hex().to_string()
}

pub(crate) async fn finish_tx<T>(conn: &Connection, result: Result<T>) -> Result<T> {
    match result {
        Ok(value) => {
            conn.execute("COMMIT", ()).await?;
            Ok(value)
        }
        Err(e) => {
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

/// Inserts a note into the database. Returns the new note id.
pub async fn insert_note(conn: &Connection, content: &str) -> Result<i64> {
    let hash = content_hash(content);
    let mut rows = conn
        .query(
            "INSERT INTO notes (content, content_hash) VALUES (?1, ?2) RETURNING id",
            (content, hash.as_str()),
        )
        .await?;

    let id: i64 = rows
        .next()
        .await?
        .context("INSERT ... RETURNING produced no row")?
        .get(0)?;

    Ok(id)
}

/// Updates a note's content. Returns `false` if the content didn't change (no-op).
pub async fn update_note(conn: &Connection, id: i64, content: &str) -> Result<bool> {
    let new_hash = content_hash(content);

    let mut rows = conn
        .query("SELECT content_hash FROM notes WHERE id = ?1", (id,))
        .await?;
    let old_hash: String = rows
        .next()
        .await?
        .with_context(|| format!("note {id} not found"))?
        .get(0)?;

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

    Ok(true)
}

pub async fn note_hash_matches(
    conn: &Connection,
    note_id: i64,
    expected_hash: &str,
) -> Result<bool> {
    let mut rows = conn
        .query("SELECT content_hash FROM notes WHERE id = ?1", (note_id,))
        .await?;
    if let Some(row) = rows.next().await? {
        let hash: String = row.get(0)?;
        Ok(hash == expected_hash)
    } else {
        Ok(false)
    }
}

pub async fn get_note(conn: &Connection, id: i64) -> Result<Option<Note>> {
    let sql = format!("SELECT {NOTE_COLUMNS} FROM notes WHERE id = ?1");
    let mut rows = conn.query(&sql, (id,)).await?;
    match rows.next().await? {
        Some(row) => Ok(Some(note_from_row(&row)?)),
        None => Ok(None),
    }
}

/// Most recently updated notes first.
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

/// Chunks and labels are removed automatically by ON DELETE CASCADE.
pub async fn delete_note(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM notes WHERE id = ?1", (id,))
        .await?;
    Ok(())
}

// ─── chunks & search ────────────────────────────────────────────────────────

/// Replaces all chunks for a note within an atomic transaction.
pub async fn save_chunks(conn: &Connection, note_id: i64, chunks: &[ChunkRow]) -> Result<()> {
    conn.execute("BEGIN", ()).await?;
    let result: Result<()> = async {
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
        Ok(())
    }
    .await;
    finish_tx(conn, result).await
}

/// Returns notes that do not currently have any embedded chunks (used on startup).
pub async fn unindexed_notes(conn: &Connection) -> Result<Vec<(i64, String)>> {
    let sql = "SELECT id, content FROM notes WHERE id NOT IN (SELECT DISTINCT note_id FROM chunks)";
    let mut rows = conn.query(sql, ()).await?;
    let mut results = Vec::new();
    while let Some(row) = rows.next().await? {
        results.push((row.get(0)?, row.get(1)?));
    }
    Ok(results)
}

pub async fn search_raw(
    _conn: &Connection,
    _query_vec: &[f32],
    _limit: i64,
) -> Result<Vec<SearchHit>> {
    let mut hits = Vec::new();
    hits.push(SearchHit {
        note_id: 1,
        chunk_index: 1,
        byte_start: 1,
        content: "Hello".to_string(),
        note_created_at: "String".to_string(),
        distance: 0.3,
    });
    Ok(hits)
}

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
