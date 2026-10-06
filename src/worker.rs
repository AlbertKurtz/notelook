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
    let pieces = chunker.split(&content);
    if pieces.is_empty() {
        return Ok(());
    }

    let texts: Vec<String> = pieces.iter().map(|(_, t)| t.to_string()).collect();
    let vectors = tokio::task::spawn_blocking(move || -> Result<Vec<Vec<f32>>> {
        embedder.embed_batch(&texts)
    })
    .await??;

    let rows: Vec<ChunkRow> = pieces
        .into_iter()
        .zip(vectors)
        .enumerate()
        .map(|(i, ((start, text), embedding))| ChunkRow {
            index: i as i64,
            byte_start: start as i64,
            content: text.to_string(),
            embedding,
        })
        .collect();

    db.save_chunks(note_id, &rows).await?;

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
