use anyhow::Result;
use std::sync::Arc;

use crate::chunk::Chunker;
use crate::db::{ChunkRow, Db};
use crate::embed::Embedder;

/// Chunks and embeds a single note saving to database.
/// CPU inference runs in `spawn_blocking` so the UI never stutters. (I guess)
pub async fn embed_note(
    db: Db,
    embedder: Arc<Embedder>,
    chunker: Arc<Chunker>,
    note_id: i64,
    content: String,
) -> Result<()> {
    let expected_hash = crate::db::repo::content_hash(&content);

    let (pieces, vectors) = tokio::task::spawn_blocking(move || -> Result<(Vec<(usize, String)>, Vec<Vec<f32>>)> {
        let pieces: Vec<(usize, String)> = chunker
            .split(&content)
            .into_iter()
            .map(|(start, t)| (start, t.to_string()))
            .collect();

        if pieces.is_empty() {
            return Ok((pieces, Vec::new()));
        }

        let texts: Vec<&str> = pieces.iter().map(|(_, t)| t.as_str()).collect();
        let vectors = embedder.embed_batch(&texts)?;
        Ok((pieces, vectors))
    })
    .await??;

    if pieces.is_empty() {
        if crate::db::repo::note_hash_matches(db.conn(), note_id, &expected_hash).await? {
            db.save_chunks(note_id, &[]).await?;
        }
        return Ok(());
    }

    let rows: Vec<ChunkRow> = pieces
        .into_iter()
        .zip(vectors)
        .enumerate()
        .map(|(i, ((start, text), embedding))| ChunkRow {
            index: i as i64,
            byte_start: start as i64,
            content: text,
            embedding,
        })
        .collect();

    // Only save if the note hasn't been edited again while we were computing embeddings
    if crate::db::repo::note_hash_matches(db.conn(), note_id, &expected_hash).await? {
        db.save_chunks(note_id, &rows).await?;
    }

    Ok(())
}

/// Runs on startup: indexes any notes that were created while offline or missed.
/// Hope users don't write a lot of notes lol
pub async fn index_unindexed_notes(
    db: Db,
    embedder: Arc<Embedder>,
    chunker: Arc<Chunker>,
) -> Result<usize> {
    let unindexed = db.unindexed_notes().await?;
    let count = unindexed.len();

    for (id, content) in unindexed {
        if let Err(e) = embed_note(
            db.clone(),
            Arc::clone(&embedder),
            Arc::clone(&chunker),
            id,
            content,
        )
        .await
        {
            eprintln!("failed to index note {id}: {e:#}");
        }
    }

    Ok(count)
}
