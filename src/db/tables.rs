// in db.rs
use turso::Connection;

pub async fn init_db(conn: &Connection) -> Result<(), Box<dyn std::error::Error>> {
    conn.execute("PRAGMA foreign_keys = ON;", ()).await?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS notes (
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             timestamp DATETIME DEFAULT (datetime('now')),
             content TEXT NOT NULL
         );",
        (),
    )
    .await?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS labels (
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             note_id INTEGER NOT NULL,
             label TEXT NOT NULL,
             FOREIGN KEY(note_id) REFERENCES notes(id) ON DELETE CASCADE,
             UNIQUE(note_id, label)
         );",
        (),
    )
    .await?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS chunks (
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             note_id INTEGER NOT NULL,
             chunk_index INTEGER NOT NULL,
             content TEXT NOT NULL,
             embedding F32_BLOB(384),
             FOREIGN KEY(note_id) REFERENCES notes(id) ON DELETE CASCADE
         );",
        (),
    )
    .await?;

    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_chunks_note_id ON chunks(note_id);",
        (),
    )
    .await?;

    let vector_idx_res = conn
        .execute(
            "CREATE INDEX IF NOT EXISTS idx_chunks_embedding
             ON chunks (libsql_vector_idx(embedding, 'metric=cosine'));",
            (),
        )
        .await;

    if let Err(e) = vector_idx_res {
        eprintln!("Vector index creation skipped or not supported locally: {e}");
    }
    Ok(())
}
