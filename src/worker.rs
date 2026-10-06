use anyhow::{Context, Result};
use std::sync::Arc;
use std::time::Duration;

use crate::chunk::Chunker;
use crate::db::{ChunkRow, DbHandle, Job};
use crate::embed::Embedder;

/// Runs forever: claims embed jobs, chunks + embeds the note, stores the result.
pub async fn run_embed_worker(db: DbHandle, embedder: Arc<Embedder>, chunker: Arc<Chunker>) {
    match db.ensure_embedding_model(embedder.model_id()).await {
        Ok(true) => eprintln!("embedding model changed: re-embedding all notes"),
        Ok(false) => {}
        Err(e) => eprintln!("could not check embedding model: {e:#}"),
    }

    loop {
        match db.claim_embed_job().await {
            Ok(Some(job)) => {
                if let Err(e) = process(&db, &embedder, &chunker, job).await {
                    eprintln!("embed job {} (note {}) failed: {e:#}", job.id, job.note_id);
                    let _ = db.fail_job(job.id, format!("{e:#}")).await;
                }
            }
            Ok(None) => {
                // Idle: wait for a signal; poll every 30 s as a safety net.
                tokio::select! {
                    _ = db.jobs_available() => {}
                    _ = tokio::time::sleep(Duration::from_secs(30)) => {}
                }
            }
            Err(e) => {
                // Probably the DB task is gone (app shutting down).
                eprintln!("worker: {e:#}");
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
    }
}

async fn process(
    db: &DbHandle,
    embedder: &Arc<Embedder>,
    chunker: &Arc<Chunker>,
    job: Job,
) -> Result<()> {
    let note = db
        .get_note(job.note_id)
        .await?
        .with_context(|| format!("note {} no longer exists", job.note_id))?;

    // CPU-heavy: move to the blocking pool so tokio's async threads stay free.
    let embedder = Arc::clone(embedder);
    let chunker = Arc::clone(chunker);
    let rows = tokio::task::spawn_blocking(move || -> Result<Vec<ChunkRow>> {
        let pieces = chunker.split(&note.content);
        let texts: Vec<&str> = pieces.iter().map(|(_, t)| *t).collect();
        let vectors = embedder.embed_batch(&texts)?;

        Ok(pieces
            .into_iter()
            .zip(vectors)
            .enumerate()
            .map(|(i, ((start, text), embedding))| ChunkRow {
                index: i as i64,
                byte_start: start as i64,
                content: text.to_string(),
                embedding,
            })
            .collect())
    })
    .await??;

    let written = db.complete_embed_job(job.id, job.note_id, rows).await?;
    if !written {
        // A newer save replaced this job; its own run will write fresh chunks.
    }
    Ok(())
}
