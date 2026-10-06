use anyhow::{Context, Result, bail};
use std::time::Duration;

use notelook::config;
use notelook::db::{self, repo};
use notelook::embed::Embedder;

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let conn = db::open(&config::db_path()?).await?;

    match args.first().map(String::as_str) {
        Some("add") => {
            let file = args.get(1).context("usage: cli add <file.md>")?;
            let text =
                std::fs::read_to_string(file).with_context(|| format!("cannot read {file}"))?;
            for part in text.split("[***]").map(str::trim).filter(|p| !p.is_empty()) {
                let id = repo::insert_note(&conn, part).await?;
                println!("added note {id}");
            }
            println!("jobs queued: the GUI's worker will embed them on next start");
        }

        Some("search") => {
            let query = args.get(1).context("usage: cli search \"<query>\"")?;
            let embedder = Embedder::try_new()?;
            let query_vec = embedder.compute(query)?;
            let hits = repo::search_chunks(&conn, &query_vec, 5).await?;

            println!("Query: {query:?}\n");
            for hit in hits {
                println!(
                    "── note {} · chunk {} · {} · similarity {:.1}%",
                    hit.note_id,
                    hit.chunk_index,
                    hit.note_created_at,
                    (1.0 - hit.distance) * 100.0
                );
                println!("{}\n", hit.content);
            }
        }

        Some("list") => {
            for note in repo::recent_notes(&conn, 20).await? {
                let preview: String = note.content.chars().take(60).collect();
                println!("{:>4}  {}  {preview}", note.id, note.updated_at);
            }
        }

        _ => bail!("usage: cli <add FILE | search QUERY | list>"),
    }

    let _ = Duration::ZERO; // remove once you add a `process-jobs` command (see exercise)
    Ok(())
}
