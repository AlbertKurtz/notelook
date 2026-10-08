use anyhow::{Context, Result};
use std::path::PathBuf;

pub const APP_NAME: &str = "notelook";

/// Hugging Face model id. Changing it triggers a full re-embed (see worker.rs).
pub const EMBED_MODEL_ID: &str = "sentence-transformers/paraphrase-multilingual-MiniLM-L12-v2";

/// Must match `F32_BLOB(...)` in db/schema.rs. Checked at model load.
pub const EMBED_DIM: usize = 384;

/// Upper bound for tokens fed to the model (queries are truncated to this).
pub const EMBED_MAX_TOKENS: usize = 256;

/// Chunk size for notes. all-MiniLM was trained on ~128-token inputs.
pub const CHUNK_MAX_TOKENS: usize = 128;
pub const CHUNK_OVERLAP: usize = 16;

/// After this many failures a job is marked 'failed' and left alone.
pub const JOB_MAX_ATTEMPTS: i64 = 3;

pub fn db_path() -> Result<PathBuf> {
    let dirs = directories::ProjectDirs::from("", "", APP_NAME)
        .context("could not determine the user's home")?;
    let dir = dirs.data_dir();
    std::fs::create_dir_all(dir)
        .with_context(|| format!("could not create data directory {}", dir.display()))?;
    Ok(dir.join("notes.db"))
}
