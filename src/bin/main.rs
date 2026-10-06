mod db;

mod embed;

use embed::Embedder;
use std::io;
use std::path::Path;
use tokio::sync::watch::error;

use db::{add_chunks, add_label, add_note, init_db, last_notes, search_chunks};
use std::fs;
use turso::{Builder, Connection};

enum Task {
    Add,
    Search(String),
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let task = match args.get(1).map(|s| s.as_str()) {
        Some("add") => Task::Add,
        Some("search") => {
            let query = args
                .get(2)
                .cloned()
                .unwrap_or_else(|| "a good fantasy book?".to_string());
            Task::Search(query)
        }
        _ => {
            // Default to Search if no argument is provided
            let query = args
                .get(1)
                .cloned()
                .unwrap_or_else(|| "Why did the cache fail?".to_string());
            Task::Search(query)
        }
    };

    let db = Builder::new_local("app.db").build().await?;
    let conn = db.connect()?;
    conn.execute("PRAGMA foreign_keys = ON;", ()).await?;
    init_db(&conn).await?;

    let embedder = Embedder::try_new()?;

    match task {
        Task::Add => add(&conn, &embedder).await?,
        Task::Search(query) => {
            search_chunks(&query, &embedder, &conn, 5).await?;
        }
    }
    Ok(())
}

async fn add(conn: &Connection, embedder: &Embedder) -> Result<(), Box<dyn std::error::Error>> {
    let original_text = get_text_from_file("my_text.md").unwrap();
    let texts: Vec<&str> = original_text.split("[***]").collect();
    for text in texts.iter() {
        let note_id = add_note(text.to_string(), &conn).await?;
        add_chunks(note_id, &text, &embedder, &conn).await?;
        last_notes(20, &conn).await?;
    }
    Ok(())
}

fn get_text_from_file(file_path: impl AsRef<Path>) -> io::Result<String> {
    fs::read_to_string(file_path)
}
