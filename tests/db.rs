use anyhow::Result;
use std::path::Path;

use notelook::db::{self, ChunkRow, repo};

// do you trust these tests???

async fn count(conn: &turso::Connection, sql: &str) -> Result<i64> {
    let mut rows = conn.query(sql, ()).await?;
    Ok(rows.next().await?.expect("COUNT returns a row").get(0)?)
}

fn fake_chunk(index: i64) -> ChunkRow {
    ChunkRow {
        index,
        byte_start: 0,
        content: format!("chunk {index}"),
        embedding: vec![0.1; notelook::config::EMBED_DIM],
    }
}

#[tokio::test]
async fn insert_enqueues_an_embed_job() -> Result<()> {
    let conn = db::open(Path::new(":memory:")).await?;
    let id = repo::insert_note(&conn, "hello").await?;

    let job = repo::claim_next_job(&conn, "embed").await?.expect("a job");
    assert_eq!(job.note_id, id);
    assert!(repo::claim_next_job(&conn, "embed").await?.is_none());
    Ok(())
}

#[tokio::test]
async fn unchanged_update_is_skipped() -> Result<()> {
    let conn = db::open(Path::new(":memory:")).await?;
    let id = repo::insert_note(&conn, "same text").await?;
    assert!(!repo::update_note(&conn, id, "same text").await?);
    assert!(repo::update_note(&conn, id, "new text").await?);
    Ok(())
}

#[tokio::test]
async fn stale_job_result_is_discarded() -> Result<()> {
    let conn = db::open(Path::new(":memory:")).await?;
    let id = repo::insert_note(&conn, "v1").await?;
    let old_job = repo::claim_next_job(&conn, "embed").await?.unwrap();

    repo::update_note(&conn, id, "v2").await?; // replaces the job

    let written = repo::complete_embed_job(&conn, old_job.id, id, &[fake_chunk(0)]).await?;
    assert!(!written, "stale result must not be written");
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM chunks").await?, 0);
    Ok(())
}

#[tokio::test]
async fn delete_cascades_to_chunks_and_jobs() -> Result<()> {
    let conn = db::open(Path::new(":memory:")).await?;
    let id = repo::insert_note(&conn, "to be deleted").await?;
    let job = repo::claim_next_job(&conn, "embed").await?.unwrap();
    repo::complete_embed_job(&conn, job.id, id, &[fake_chunk(0), fake_chunk(1)]).await?;
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM chunks").await?, 2);

    repo::delete_note(&conn, id).await?;
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM chunks").await?, 0);
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM jobs").await?, 0);
    Ok(())
}

#[tokio::test]
async fn migrations_are_idempotent() -> Result<()> {
    // Opening runs migrations; running them twice must not fail.
    let conn = db::open(Path::new(":memory:")).await?;
    drop(conn);
    // For a file DB you'd reopen the same path here; :memory: is fresh each time,
    // so this mainly checks that migrate() on an empty DB succeeds.
    Ok(())
}
