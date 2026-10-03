mod db;

mod embed;

use embed::Embedder;
use std::io;
use std::{
    error::Error,
    path::{self, Path},
};

use db::{add_label, add_note, init_db, last_notes};
use std::fs;
use turso::Builder;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let db = Builder::new_local("app.db").build().await?;
    let conn = db.connect()?;
    conn.execute("PRAGMA foreign_keys = ON;", ()).await?;
    init_db(&conn).await?;

    let embedder = Embedder::try_new()?;

    let text = get_text_from_file("my_text.md").unwrap();
    add_note(text.to_string(), &conn).await?;
    // all_notes(&conn).await?;
    last_notes(20, &conn).await?;
    add_label("loco!", 10, &conn).await?;
    // delete_note(5, &conn).await?;
    // all_notes(&conn).await?;
    Ok(())
}

fn get_text_from_file(file_path: impl AsRef<Path>) -> io::Result<String> {
    fs::read_to_string(file_path)
}
