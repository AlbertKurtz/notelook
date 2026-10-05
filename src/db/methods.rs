use text_splitter::{ChunkConfig, TextSplitter};
use tokenizers::Tokenizer;
use turso::Connection;

use crate::embed::Embedder;

pub async fn add_note(text: String, conn: &Connection) -> Result<i64, Box<dyn std::error::Error>> {
    let mut rows = conn
        .query(
            "INSERT INTO notes (content) VALUES (?1) RETURNING id",
            (text,),
        )
        .await?;

    let row = rows
        .next()
        .await?
        .ok_or_else(|| "Failed to retrieve inserted note ID".to_string())?;

    let note_id: i64 = row.get(0)?;
    println!("created note with id: {note_id}");

    Ok(note_id)
}

pub fn chunk_with_tokenizer(
    text: &str,
    tokenizer: Tokenizer,
    max_tokens: usize,
    token_overlap: usize,
) -> anyhow::Result<Vec<String>> {
    let mut tok = tokenizer;
    tok.with_truncation(None)
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let config = ChunkConfig::new(max_tokens)
        .with_sizer(tok)
        .with_overlap(token_overlap)?;

    let splitter = TextSplitter::new(config);
    let chunks: Vec<String> = splitter.chunks(text).map(|s| s.to_string()).collect();

    Ok(chunks)
}

pub async fn add_chunks(
    note_id: i64,
    text: &str,
    embedder: &Embedder,
    conn: &Connection,
) -> Result<usize, Box<dyn std::error::Error>> {
    let tokenizer = embedder.tokenizer();
    let max_tokens = 256;
    let overlap = 32;

    let chunks = chunk_with_tokenizer(text, tokenizer, max_tokens, overlap)?;
    if chunks.is_empty() {
        return Ok(0);
    }

    let emb = embedder.clone();
    let chunks_to_embed = chunks.clone();
    let embeddings = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<Vec<f32>>> {
        emb.embed_batch(&chunks_to_embed)
    })
    .await??;

    for (idx, (chunk_content, vector)) in chunks.iter().zip(embeddings.iter()).enumerate() {
        let vector_str = serde_json::to_string(vector)?;

        conn.execute(
            "INSERT INTO chunks (note_id, chunk_index, content, embedding)
                         VALUES (?1, ?2, ?3, vector(?4))",
            (note_id, idx as i64, chunk_content.as_str(), vector_str),
        )
        .await?;
    }
    Ok(chunks.len())
}

pub async fn search_chunks(
    query: &str,
    embedder: &Embedder,
    conn: &Connection,
    limit: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let query_vec = embedder.compute(query)?;
    let query_vec_str = serde_json::to_string(&query_vec)?;

    let mut rows = conn
        .query(
            "SELECT content, vector_distance_cos(embedding, vector(?1))
        AS distance FROM chunks
        ORDER BY distance ASC
        LIMIT ?2",
            (query_vec_str, limit as i64),
        )
        .await?;

    println!("\nQuery: \"{query}\"");
    while let Some(row) = rows.next().await? {
        let content: String = row.get(0)?;
        let distance: f64 = row.get(1)?;
        println!("--------------------------------------------------");
        println!(
            "Distance: {:.4} (Similarity: {:.1}%)",
            distance,
            (1.0 - distance) * 100.0
        );
        println!("{content}");
    }

    Ok(())
}

pub async fn note_by_id(id: i32, conn: &Connection) -> Result<(), Box<dyn std::error::Error>> {
    // let text: String = "A random note to you =),  second one lol!".to_string();
    let mut row_by_id = conn
        .query(
            "SELECT id, timestamp, content FROM notes where id=(?1)",
            (id,),
        )
        .await?;
    while let Some(row) = row_by_id.next().await? {
        let id: i32 = row.get(0)?;
        let timestamp: Option<String> = row.get(1)?;
        let content: String = row.get(2)?;
        let ts_display = timestamp.as_deref().unwrap_or("no timestamp");
        println!("{id} [{ts_display}]: {content}");
    }

    Ok(())
}

pub async fn delete_note(id: i32, conn: &Connection) -> Result<(), Box<dyn std::error::Error>> {
    // let text: String = "A random note to you =),  second one lol!".to_string();
    let _ = conn
        .execute("DELETE FROM notes where id=(?1)", (id,))
        .await?;

    Ok(())
}

pub async fn all_notes(conn: &Connection) -> Result<(), Box<dyn std::error::Error>> {
    // let text: String = "A random note to you =),  second one lol!".to_string();
    let mut current_rows = conn
        .query("SELECT id, timestamp, content FROM notes", ())
        .await?;
    while let Some(row) = current_rows.next().await? {
        let id: i32 = row.get(0)?;
        let timestamp: Option<String> = row.get(1)?;
        let content: String = row.get(2)?;
        let ts_display = timestamp.as_deref().unwrap_or("no timestamp");
        println!("{id} [{ts_display}]: {content}");
    }

    Ok(())
}

pub async fn last_notes(last_n: i32, conn: &Connection) -> Result<(), Box<dyn std::error::Error>> {
    let sql = "
            SELECT id, timestamp, content
            FROM (
                SELECT id, timestamp, content
                FROM notes
                ORDER BY id DESC
                LIMIT ?1
            )
            ORDER BY id ASC
        ";
    let mut current_rows = conn.query(sql, (last_n,)).await?;
    while let Some(row) = current_rows.next().await? {
        let id: i32 = row.get(0)?;
        let timestamp: Option<String> = row.get(1)?;
        let content: String = row.get(2)?;
        let ts_display = timestamp.as_deref().unwrap_or("no timestamp");
        println!("{id} [{ts_display}]: {content}");
    }

    Ok(())
}

pub async fn add_label(
    label: &str,
    note_id: i32,
    conn: &Connection,
) -> Result<(), Box<dyn std::error::Error>> {
    let sql = "
            INSERT OR IGNORE INTO labels (label, note_id) VALUES (?1, ?2)
        ";

    let _new_label = conn.execute(sql, (label, note_id)).await?;

    Ok(())
}
