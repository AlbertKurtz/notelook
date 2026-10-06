use anyhow::{Context, Result, bail};
use std::time::Duration;

use notelook::config;
use notelook::db::Db;
use notelook::embed::Embedder;

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    let db = Db::open(&config::db_path()?).await?;

    match args.first().map(String::as_str) {
        Some("add") => {
            let file = args.get(1).context("usage: cli add <file.md>")?;
            let text =
                std::fs::read_to_string(file).with_context(|| format!("cannot read {file}"))?;
            for part in text.split("[***]").map(str::trim).filter(|p| !p.is_empty()) {
                let id = db.insert_note(part).await?;
                println!("added note {id}");
            }
            println!("notes saved: start GUI or run search to query them");
        }

        Some("search") => {
            let query = args.get(1).context("usage: cli search \"<query>\"")?;
            let embedder = Embedder::try_new()?;
            let query_vec = embedder.compute(query)?;
            let hits = db.search(&query_vec, 5).await?;

            println!("Query: {query:?}\n");
            for hit in hits {
                println!(
                    "── note {} · chunk {} · {} · similarity {:.1}%",
                    hit.note_id,
                    hit.chunk_index,
                    hit.note_created_at,
                    hit.similarity_percent()
                );
                println!("{}\n", hit.content);
            }
        }

        Some("list") => {
            for note in db.recent_notes(20).await? {
                let preview: String = note.content.chars().take(60).collect();
                println!("{:>4}  {}  {preview}", note.id, note.updated_at);
            }
        }

        _ => bail!("usage: cli <add FILE | search QUERY | list>"),
    }

    let _ = Duration::ZERO;
    Ok(())
}
