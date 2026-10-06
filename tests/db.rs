use anyhow::Result;
use std::path::Path;

use notelook::db::{ChunkRow, Db};

fn fake_chunk(index: i64) -> ChunkRow {
    ChunkRow {
        index,
        byte_start: 0,
        content: format!("chunk {index}"),
        embedding: vec![0.1; notelook::config::EMBED_DIM],
    }
}

#[tokio::test]
async fn insert_and_retrieve_note() -> Result<()> {
    let db = Db::open(Path::new(":memory:")).await?;
    let id = db
        .insert_note("Meeting notes: project alpha launch")
        .await?;

    let note = db.get_note(id).await?.expect("note exists");
    assert_eq!(note.id, id);
    assert_eq!(note.content, "Meeting notes: project alpha launch");
    assert_eq!(note.title(), "Meeting notes: project alpha launch");
    Ok(())
}

#[tokio::test]
async fn unchanged_update_returns_false() -> Result<()> {
    let db = Db::open(Path::new(":memory:")).await?;
    let id = db.insert_note("initial content").await?;

    // Same content -> returns false
    assert!(!db.update_note(id, "initial content").await?);

    // Changed content -> returns true
    assert!(db.update_note(id, "updated content").await?);

    let updated = db.get_note(id).await?.unwrap();
    assert_eq!(updated.content, "updated content");
    Ok(())
}

#[tokio::test]
async fn delete_cascades_to_chunks() -> Result<()> {
    let db = Db::open(Path::new(":memory:")).await?;
    let id = db.insert_note("note to delete").await?;

    let chunks = vec![fake_chunk(0), fake_chunk(1)];
    db.save_chunks(id, &chunks).await?;

    // Search returns our chunk
    let hits = db
        .search(&vec![0.1; notelook::config::EMBED_DIM], 5)
        .await?;
    assert_eq!(hits.len(), 2);

    // Delete the note
    db.delete_note(id).await?;

    // Chunks are gone due to ON DELETE CASCADE
    let hits_after = db
        .search(&vec![0.1; notelook::config::EMBED_DIM], 5)
        .await?;
    assert_eq!(hits_after.len(), 0);
    Ok(())
}

#[tokio::test]
async fn unindexed_notes_detection() -> Result<()> {
    let db = Db::open(Path::new(":memory:")).await?;
    let id1 = db.insert_note("note 1").await?;
    let _id2 = db.insert_note("note 2").await?;

    // Save chunks for note 1 only
    db.save_chunks(id1, &[fake_chunk(0)]).await?;

    // note 2 is unindexed
    let unindexed = db.unindexed_notes().await?;
    assert_eq!(unindexed.len(), 1);
    assert_eq!(unindexed[0].0, 2);
    Ok(())
}
