mod db_methods;
use std::io;
use std::{
    error::Error,
    path::{self, Path},
};

use db_methods::{add_note, last_notes};
use std::fs;
use turso::Builder;

use crate::db_methods::{add_label, all_notes, delete_note};
// use turso::Builder;
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // println!("{}", summm(2, 3));
    let db = Builder::new_local("app.db").build().await?;
    let conn = db.connect()?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS notes (
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             timestamp DATETIME DEFAULT (datetime('now')),
             content TEXT NOT NULL
         )",
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
        )",
        (),
    )
    .await?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS embeddings (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                note_id INTEGER NOT NULL,
                embedding F32_BLOB(384),
                FOREIGN KEY(note_id) REFERENCES notes(id) ON DELETE CASCADE
            )",
        (),
    )
    .await?;

    let text = get_text_from_file("my_text.md").unwrap();
    add_note(text.to_string(), &conn).await?;
    // all_notes(&conn).await?;
    last_notes(20, &conn).await?;
    add_label("crazy!!!!!", 4, &conn).await?;
    // delete_note(5, &conn).await?;
    // all_notes(&conn).await?;
    Ok(())
}

fn get_text_from_file(file_path: impl AsRef<Path>) -> io::Result<String> {
    fs::read_to_string(file_path)
}
