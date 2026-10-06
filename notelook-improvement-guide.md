# Notelook — Improvement Guide

A step-by-step plan to fix, restructure and extend the current codebase, written to be followed **by hand**.

Each phase ends with a **checkpoint**: something to compile, run or test before moving on. Commit after every checkpoint, so you can always go back to a working state.

> ⚠️ A few APIs below (turso 0.8, iced 0.14) are young and change between versions. Where the exact name is uncertain, the step is marked **⚠️ verify**. When that happens, check `cargo doc --open` for the crate before typing the code.

---

## Table of contents

0. [Overview of the changes](#0-overview-of-the-changes)
1. [Phase 1 — Project structure](#phase-1--project-structure)
2. [Phase 2 — Embedding fixes](#phase-2--embedding-fixes)
3. [Phase 3 — Database layer](#phase-3--database-layer)
4. [Phase 4 — DB actor and background worker](#phase-4--db-actor-and-background-worker)
5. [Phase 5 — UI](#phase-5--ui)
6. [Phase 6 — CLI and tests](#phase-6--cli-and-tests)
7. [Refactoring ideas](#7-refactoring-ideas)
8. [Roadmap — what comes after](#8-roadmap--what-comes-after)
9. [Master checklist](#9-master-checklist)

---

## 0. Overview of the changes

### What changes and why

| # | Change | Why |
|---|--------|-----|
| 1 | Add `src/lib.rs`, move CLI to `src/bin/cli.rs` | `main_cli.rs` is never compiled; two binaries should share one library |
| 2 | Use `anyhow::Result` everywhere | One error type; `Box<dyn Error>` isn't `Send` and will break async tasks |
| 3 | Pass the attention mask to `BertModel::forward` | **Bug:** padding tokens currently influence embeddings |
| 4 | Build the chunker once, use `MarkdownSplitter`, 128-token chunks | Cloning the tokenizer per save is wasteful; smaller chunks retrieve better |
| 5 | Schema v1 with migrations, `meta`, `jobs`, timestamps, hash | Room to grow without breaking the DB later |
| 6 | DB functions return structs, not `println!` | The UI needs data |
| 7 | Transactions around multi-statement writes | No half-saved notes after a crash |
| 8 | Update path for existing notes | Today each Save creates a duplicate |
| 9 | DB actor (one task owns the connection) | Safe transactions, no interleaving between UI and worker |
| 10 | Background embedding worker + job queue | Save returns instantly; embedding survives restarts |
| 11 | UI: split startup, dirty tracking, shortcuts, save guard | Instant startup, no double saves, keyboard-driven |
| 12 | DB stored in the OS data directory | `app.db` in the current folder breaks when launched from elsewhere |

### Target architecture

```
 ┌──────────────┐  Task::perform   ┌────────────────┐   mpsc + oneshot   ┌──────────────┐
 │   iced UI    │ ───────────────▶ │   DbHandle     │ ─────────────────▶ │  DB actor    │
 │  (main.rs)   │ ◀─────────────── │ (cheap clone)  │ ◀───────────────── │ owns Conn    │
 └──────────────┘    Message       └────────────────┘      replies       └──────┬───────┘
                                          ▲                                     │
                                          │ claim / complete jobs               │ SQL
                                   ┌──────┴────────┐                     ┌──────▼───────┐
                                   │ Embed worker  │ spawn_blocking      │  turso DB    │
                                   │  (tokio task) │──▶ Embedder/Chunker │ notes,chunks │
                                   └───────────────┘                     │ jobs, meta   │
                                                                         └──────────────┘
```

### Target file layout

```
notelook/
├── Cargo.toml
├── assets/logo.png
├── src/
│   ├── lib.rs          NEW  library root
│   ├── main.rs         REWRITE  GUI binary
│   ├── bin/
│   │   └── cli.rs      MOVED from main_cli.rs
│   ├── config.rs       NEW  constants + paths
│   ├── chunk.rs        NEW  Chunker
│   ├── embed.rs        EDIT  attention mask, chunker factory
│   ├── worker.rs       NEW  background embedding worker
│   ├── db.rs           EDIT  module root + open()
│   └── db/
│       ├── models.rs   NEW  Note, SearchHit, Job, ChunkRow
│       ├── schema.rs   NEW  migrations (replaces tables.rs)
│       ├── repo.rs     NEW  SQL functions (replaces methods.rs)
│       └── actor.rs    NEW  DbHandle
└── tests/
    └── db.rs           NEW  integration tests
```

---

## Phase 1 — Project structure

### 1.1 Cargo.toml

Add three dependencies and `default-run`. The second binary in `src/bin/` makes `cargo run` ambiguous without it.

```toml
[package]
name = "notelook"
version = "0.1.1"
edition = "2024"
description = "Lightweight yet powerful notepad app"
default-run = "notelook"          # NEW: `cargo run` starts the GUI

[dependencies]
anyhow = "1.0.104"
blake3 = "1"                      # NEW: content hashing
candle-core = "0.11.0"
candle-nn = "0.11.0"
candle-transformers = "0.11.0"
directories = "6"                 # NEW: per-OS data directory
hf-hub = { version = "0.4", features = ["tokio"] }
iced = { version = "0.14.0", features = ["tokio"] }
image = { version = "0.25", default-features = false, features = ["png"] }
serde_json = "1.0.151"
text-splitter = { version = "0.33", features = ["tokenizers", "markdown"] }
tokenizers = { version = "0.23", default-features = false, features = ["onig"] }
tokio = { version = "1.53.1", features = ["full"] }   # "full" already includes macros + rt-multi-thread
turso = "0.8.1"
```

**What you'll learn:** how Cargo discovers targets. `src/main.rs` becomes the binary `notelook`, `src/bin/cli.rs` becomes the binary `cli`, and `src/lib.rs` becomes the library `notelook`. Both binaries import the library with `use notelook::...`.

Run the CLI with `cargo run --bin cli -- search "fantasy books"`.

### 1.2 `src/lib.rs`

```rust
//! Notelook library: everything except the GUI entry point.

pub mod chunk;
pub mod config;
pub mod db;
pub mod embed;
pub mod worker;
```

### 1.3 `src/config.rs`

Gather the magic numbers in one place.

```rust
use anyhow::{Context, Result};
use std::path::PathBuf;

pub const APP_NAME: &str = "notelook";

/// Hugging Face model id. Changing it triggers a full re-embed (see worker.rs).
pub const EMBED_MODEL_ID: &str = "sentence-transformers/all-MiniLM-L6-v2";

/// Must match `F32_BLOB(...)` in db/schema.rs. Checked at model load.
pub const EMBED_DIM: usize = 384;

/// Upper bound for tokens fed to the model (queries are truncated to this).
pub const EMBED_MAX_TOKENS: usize = 256;

/// Chunk size for notes. all-MiniLM was trained on ~128-token inputs.
pub const CHUNK_MAX_TOKENS: usize = 128;
pub const CHUNK_OVERLAP: usize = 16;

/// After this many failures a job is marked 'failed' and left alone.
pub const JOB_MAX_ATTEMPTS: i64 = 3;

/// Linux:   ~/.local/share/notelook/notes.db
/// Windows: C:\Users\<you>\AppData\Roaming\notelook\data\notes.db
pub fn db_path() -> Result<PathBuf> {
    let dirs = directories::ProjectDirs::from("", "", APP_NAME)
        .context("could not determine the user's home directory")?;
    let dir = dirs.data_dir();
    std::fs::create_dir_all(dir)
        .with_context(|| format!("could not create data directory {}", dir.display()))?;
    Ok(dir.join("notes.db"))
}
```

**What you'll learn:** `.context()` / `.with_context()` from anyhow. They add a human-readable layer to an error. Printing the error with `{e:#}` shows the whole chain, e.g. `could not create data directory /x: Permission denied`.

### 1.4 Error handling rule

From now on, every fallible function in the library returns `anyhow::Result<T>`:

```rust
use anyhow::{Context, Result, anyhow, ensure};
```

You convert to `String` **only** at the iced `Message` boundary, because `Message` must be `Clone` and `anyhow::Error` isn't:

```rust
.map_err(|e| format!("{e:#}"))
```

Delete every `Box<dyn std::error::Error>` you find.

### 1.5 Delete the old files (after Phase 3)

`src/main_cli.rs`, `src/db/methods.rs` and `src/db/tables.rs` will be replaced. Don't delete them yet; use them as reference while you type the new code.

Also **delete `app.db`**. The new schema starts fresh, and during development that's simpler than migrating test data.

### ✅ Checkpoint 1

You can't compile yet, since the modules are empty. Create the empty files from the layout above, add `pub mod` lines, and make sure `cargo check` only complains about missing items, not about missing files.

---

## Phase 2 — Embedding fixes

### 2.1 The attention-mask bug (most important fix)

**Problem.** With `PaddingStrategy::BatchLongest`, short texts in a batch are padded with `[PAD]` tokens. You compute the mask but only use it for pooling. The model itself runs with `None`, so every token, padding included, attends to the padding. The same sentence then gets a slightly different vector depending on what it was batched with.

**Fix.** Build the mask **before** the forward pass and give it to the model:

```rust
// [batch, seq_len], values 1 = real token, 0 = padding
let attention_mask: Vec<Tensor> = encodings
    .iter()
    .map(|e| Tensor::new(e.get_attention_mask(), &self.device))
    .collect::<candle_core::Result<Vec<_>>>()?;
let attention_mask = Tensor::stack(&attention_mask, 0)?.to_dtype(DType::F32)?;

// The model now ignores padding
let ys = self.model.forward(&input_ids, &token_type_ids, Some(&attention_mask))?;

// Same mask, reshaped to [batch, seq_len, 1] for pooling
let pool_mask = attention_mask.unsqueeze(2)?;
let masked_embeddings = ys.broadcast_mul(&pool_mask)?;
let sum_embeddings = masked_embeddings.sum(1)?;
let sum_mask = pool_mask.sum(1)?.clamp(1e-9, f64::MAX)?;
let pooled = sum_embeddings.broadcast_div(&sum_mask)?;
```

**What you'll learn:** tensor shapes. Write the shape as a comment next to every tensor line (`// [b, s, h]`). It's the single best habit for this kind of code.

### 2.2 Full new `src/embed.rs`

Other changes compared to the current version:

- Remove `tokenizer()` (cloned the tokenizer on every save). Replace it with `chunker()`, a factory called once at startup.
- Truncate queries to `EMBED_MAX_TOKENS`, so a giant pasted query can't crash the model (BERT has a hard 512-position limit).
- Check the output dimension against `EMBED_DIM` at load time, so a model swap can't silently corrupt the DB.

```rust
use anyhow::{Context, Result, anyhow, ensure};
use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config};
use hf_hub::api::sync::Api;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use tokenizers::{PaddingParams, Tokenizer, TruncationParams};

use crate::chunk::Chunker;
use crate::config::{EMBED_DIM, EMBED_MAX_TOKENS, EMBED_MODEL_ID};

#[derive(Clone)]
pub struct Embedder {
    model: Arc<BertModel>,
    tokenizer: Arc<Tokenizer>,
    tokenizer_path: PathBuf,
    device: Device,
}

impl fmt::Debug for Embedder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Embedder")
            .field("model", &EMBED_MODEL_ID)
            .finish_non_exhaustive()
    }
}

impl Embedder {
    /// Downloads (first run) or loads from cache, then runs a probe embedding.
    /// Blocking: call it inside `tokio::task::spawn_blocking`.
    pub fn try_new() -> Result<Self> {
        let device = Device::Cpu;
        let api = Api::new().context("failed to initialize Hugging Face API client")?;
        let repo = api.model(EMBED_MODEL_ID.to_string());

        let config_path = repo.get("config.json").context("failed to get config.json")?;
        let tokenizer_path = repo.get("tokenizer.json").context("failed to get tokenizer.json")?;
        let weights_path = repo
            .get("model.safetensors")
            .context("failed to get model.safetensors")?;

        let config: Config = serde_json::from_str(&std::fs::read_to_string(&config_path)?)?;

        let mut tokenizer = Tokenizer::from_file(&tokenizer_path).map_err(|e| anyhow!("{e}"))?;
        tokenizer.with_padding(Some(PaddingParams {
            strategy: tokenizers::PaddingStrategy::BatchLongest,
            ..Default::default()
        }));
        tokenizer
            .with_truncation(Some(TruncationParams {
                max_length: EMBED_MAX_TOKENS,
                ..Default::default()
            }))
            .map_err(|e| anyhow!("{e}"))?;

        // SAFETY: the file is memory-mapped; it must not be modified while in use.
        let vb = unsafe { VarBuilder::from_mmaped_safetensors(&[weights_path], DType::F32, &device)? };
        let model = BertModel::load(vb, &config)?;

        let embedder = Self {
            model: Arc::new(model),
            tokenizer: Arc::new(tokenizer),
            tokenizer_path,
            device,
        };

        let probe = embedder.compute("dimension probe")?;
        ensure!(
            probe.len() == EMBED_DIM,
            "model {EMBED_MODEL_ID} produces {} dims but the schema expects {EMBED_DIM}",
            probe.len()
        );

        Ok(embedder)
    }

    pub fn model_id(&self) -> &'static str {
        EMBED_MODEL_ID
    }

    /// Builds a chunker that uses the same vocabulary as the model.
    /// Call once at startup, then share it with `Arc`.
    pub fn chunker(&self, max_tokens: usize, overlap: usize) -> Result<Chunker> {
        let tokenizer = Tokenizer::from_file(&self.tokenizer_path).map_err(|e| anyhow!("{e}"))?;
        Chunker::new(tokenizer, max_tokens, overlap)
    }

    pub fn compute(&self, text: &str) -> Result<Vec<f32>> {
        self.embed_batch(&[text])?
            .pop()
            .context("embedding batch was empty")
    }

    pub fn embed_batch<S: AsRef<str>>(&self, sentences: &[S]) -> Result<Vec<Vec<f32>>> {
        if sentences.is_empty() {
            return Ok(Vec::new());
        }

        let inputs: Vec<&str> = sentences.iter().map(|s| s.as_ref()).collect();
        let encodings = self
            .tokenizer
            .encode_batch(inputs, true)
            .map_err(|e| anyhow!("{e}"))?;

        let to_tensor = |f: &dyn Fn(&tokenizers::Encoding) -> &[u32]| -> Result<Tensor> {
            let rows = encodings
                .iter()
                .map(|e| Tensor::new(f(e), &self.device))
                .collect::<candle_core::Result<Vec<_>>>()?;
            Ok(Tensor::stack(&rows, 0)?)
        };

        let input_ids = to_tensor(&|e| e.get_ids())?; //             [b, s]
        let token_type_ids = to_tensor(&|e| e.get_type_ids())?; //   [b, s]
        let attention_mask = to_tensor(&|e| e.get_attention_mask())?
            .to_dtype(DType::F32)?; //                               [b, s]

        let ys = self
            .model
            .forward(&input_ids, &token_type_ids, Some(&attention_mask))?; // [b, s, h]

        // Mean pooling over real tokens only
        let pool_mask = attention_mask.unsqueeze(2)?; //                     [b, s, 1]
        let summed = ys.broadcast_mul(&pool_mask)?.sum(1)?; //               [b, h]
        let counts = pool_mask.sum(1)?.clamp(1e-9, f64::MAX)?; //            [b, 1]
        let pooled = summed.broadcast_div(&counts)?; //                      [b, h]

        // L2 normalization: dot product == cosine similarity
        let norm = pooled.sqr()?.sum_keepdim(1)?.sqrt()?; //                 [b, 1]
        let normalized = pooled.broadcast_div(&norm)?; //                    [b, h]

        Ok(normalized.to_vec2::<f32>()?)
    }
}
```

> The `to_tensor` closure removes the triple copy-paste of the "map encodings → stack" pattern. If closures taking `&dyn Fn` feel too clever, keep three explicit blocks. Both are fine.

### 2.3 New `src/chunk.rs`

`MarkdownSplitter` instead of `TextSplitter`: it prefers to split at headings, list items and paragraphs, which is how meeting notes are written. `chunk_indices` also returns the **byte offset** of each chunk, which you'll store so a search hit can jump to the right spot in the note.

```rust
use anyhow::{Result, anyhow};
use text_splitter::{ChunkConfig, MarkdownSplitter};
use tokenizers::Tokenizer;

/// Splits note text into token-bounded chunks. Build once, share with `Arc`.
pub struct Chunker {
    splitter: MarkdownSplitter<Tokenizer>,
}

impl Chunker {
    pub fn new(mut tokenizer: Tokenizer, max_tokens: usize, overlap: usize) -> Result<Self> {
        // Counting tokens must see the whole text: no truncation, no padding.
        tokenizer.with_truncation(None).map_err(|e| anyhow!("{e}"))?;
        tokenizer.with_padding(None);

        let config = ChunkConfig::new(max_tokens)
            .with_sizer(tokenizer)
            .with_overlap(overlap)?;

        Ok(Self {
            splitter: MarkdownSplitter::new(config),
        })
    }

    /// Returns `(byte_offset, chunk_text)` pairs borrowing from `text`.
    pub fn split<'t>(&self, text: &'t str) -> Vec<(usize, &'t str)> {
        self.splitter.chunk_indices(text).collect()
    }
}
```

**What you'll learn:** the lifetime `'t` says "the returned slices live as long as the input text, not as long as the chunker". Try removing it and read the compiler error. It's instructive.

### 2.4 Decide on the model now

all-MiniLM-L6-v2 is English-only. If you'll ever write notes in other languages, switch **before** you have real data, since a change means re-embedding everything. Two 384-dim multilingual options that keep `EMBED_DIM` and the schema unchanged:

- `sentence-transformers/paraphrase-multilingual-MiniLM-L12-v2`: BERT-like, should load with the same code. ⚠️ verify that it ships `model.safetensors` and loads with candle's BERT (it's XLM-R-based, so `Config` may need adjusting).
- `intfloat/multilingual-e5-small`: needs `"query: "` and `"passage: "` prefixes on input, so you'd add a parameter to `embed_batch`.

Phase 4 stores the model id in the DB, so a later switch re-embeds automatically.

### ✅ Checkpoint 2

Add this test at the bottom of `embed.rs`. It proves the mask fix: the same short text must produce the same vector alone and in a batch with a long text.

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "downloads the model on first run"]
    fn batching_does_not_change_embeddings() -> Result<()> {
        let e = Embedder::try_new()?;
        let short = "Marco said the cache failed.";
        let long = "This is a much longer sentence that forces the short one \
                    to be padded when both are encoded in the same batch.";

        let alone = e.compute(short)?;
        let batched = e.embed_batch(&[short, long])?;

        let dot: f32 = alone.iter().zip(&batched[0]).map(|(a, b)| a * b).sum();
        assert!(dot > 0.9999, "cosine similarity was {dot}");
        Ok(())
    }
}
```

Run with `cargo test -- --ignored`. For a lesson, temporarily put `None` back in `forward` and watch the test fail.

---

## Phase 3 — Database layer

The layer is split into three parts:

- `schema.rs`: **what** the tables are.
- `repo.rs`: plain `async fn(&Connection, ...)`, one per operation, easy to test.
- `actor.rs` (Phase 4): a thin wrapper that serializes calls through one task.

### 3.1 `src/db/models.rs`

```rust
/// A note as stored in the `notes` table.
#[derive(Debug, Clone)]
pub struct Note {
    pub id: i64,
    pub created_at: String,
    pub updated_at: String,
    /// When the event happened (meeting date). `None` means "same as created_at".
    pub event_at: Option<String>,
    /// Meeting / project / context label, e.g. "weekly standup".
    pub context: Option<String>,
    pub content: String,
}

/// One result of a semantic search.
#[derive(Debug, Clone)]
pub struct SearchHit {
    pub note_id: i64,
    pub chunk_index: i64,
    pub byte_start: i64,
    pub content: String,
    pub note_created_at: String,
    /// Cosine distance: 0.0 = identical, 2.0 = opposite.
    pub distance: f64,
}

/// A background job claimed by the worker.
#[derive(Debug, Clone, Copy)]
pub struct Job {
    pub id: i64,
    pub note_id: i64,
    pub attempts: i64,
}

/// A chunk ready to be written: produced by the worker, consumed by the repo.
#[derive(Debug, Clone)]
pub struct ChunkRow {
    pub index: i64,
    pub byte_start: i64,
    pub content: String,
    pub embedding: Vec<f32>,
}
```

### 3.2 `src/db/schema.rs`: migrations

The idea: an ordered list of migrations, plus the current version stored in a `meta` table. On startup, run every migration newer than the stored version, each in a transaction. To change the schema later, **never edit an old migration**; append a new one.

```rust
use anyhow::{Context, Result};
use turso::Connection;

use super::repo::{finish_tx, get_meta, set_meta};

/// Each entry is one schema version; each version is a list of statements.
/// Index 0 = version 1. Append only, never edit.
const MIGRATIONS: &[&[&str]] = &[
    // ── v1 ──────────────────────────────────────────────
    &[
        "CREATE TABLE notes (
            id           INTEGER PRIMARY KEY AUTOINCREMENT,
            created_at   TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at   TEXT NOT NULL DEFAULT (datetime('now')),
            event_at     TEXT,
            context      TEXT,
            content      TEXT NOT NULL,
            content_hash TEXT NOT NULL
        )",
        "CREATE TABLE labels (
            id      INTEGER PRIMARY KEY AUTOINCREMENT,
            note_id INTEGER NOT NULL REFERENCES notes(id) ON DELETE CASCADE,
            label   TEXT NOT NULL,
            UNIQUE(note_id, label)
        )",
        // Keep 384 in sync with config::EMBED_DIM
        "CREATE TABLE chunks (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            note_id     INTEGER NOT NULL REFERENCES notes(id) ON DELETE CASCADE,
            chunk_index INTEGER NOT NULL,
            byte_start  INTEGER NOT NULL,
            content     TEXT NOT NULL,
            embedding   F32_BLOB(384),
            UNIQUE(note_id, chunk_index)
        )",
        "CREATE INDEX idx_chunks_note_id ON chunks(note_id)",
        // One job per (kind, note). Re-saving a note replaces its job.
        "CREATE TABLE jobs (
            id         INTEGER PRIMARY KEY AUTOINCREMENT,
            kind       TEXT NOT NULL,
            note_id    INTEGER NOT NULL REFERENCES notes(id) ON DELETE CASCADE,
            status     TEXT NOT NULL DEFAULT 'pending',
            attempts   INTEGER NOT NULL DEFAULT 0,
            last_error TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(kind, note_id)
        )",
        "CREATE INDEX idx_jobs_status ON jobs(kind, status)",
    ],
    // ── v2 goes here, e.g. entities/mentions tables ─────
];

pub async fn migrate(conn: &Connection) -> Result<()> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
        (),
    )
    .await?;

    let current: i64 = match get_meta(conn, "schema_version").await? {
        Some(v) => v.parse().context("schema_version in meta is not a number")?,
        None => 0,
    };

    for (i, statements) in MIGRATIONS.iter().enumerate() {
        let version = i as i64 + 1;
        if version <= current {
            continue;
        }

        conn.execute("BEGIN", ()).await?;
        let result: Result<()> = async {
            for sql in statements.iter() {
                conn.execute(sql, ())
                    .await
                    .with_context(|| format!("migration v{version} failed on:\n{sql}"))?;
            }
            set_meta(conn, "schema_version", &version.to_string()).await
        }
        .await;
        finish_tx(conn, result).await?;
    }

    Ok(())
}
```

**Why `meta` and not `PRAGMA user_version`?** It works the same in any SQLite-like engine, and you'll reuse the table for other settings, like the embedding model id.

**About the vector index.** Your current `libsql_vector_idx` belongs to **libsql**, while the `turso` crate is the newer Rust rewrite. That's why the index creation probably fails silently into your `eprintln!`. Search with `ORDER BY vector_distance_cos(...)` scans every chunk, which is fine up to tens of thousands of chunks. ⚠️ verify in the turso docs whether the version you use supports a vector index, and add it as a later migration if so.

### 3.3 `src/db/repo.rs`: the SQL functions

Patterns used in this file, worth understanding before typing:

**Transactions with an inner async block.** Run the body, then COMMIT on success or ROLLBACK on error. The `?` inside the block exits the *block*, not the function, so the rollback always runs:

```rust
conn.execute("BEGIN", ()).await?;
let result: Result<T> = async { /* statements with ? */ }.await;
finish_tx(conn, result).await
```

**Scope your `Rows`.** Read what you need from a query inside a `{ ... }` block, so the statement is dropped before the next one runs.

**Content hash.** `blake3` gives a stable hash for the content. Saving unchanged text is detected and skipped, which saves an embedding run. Don't use `std::hash::DefaultHasher` for anything stored on disk, since its output can change between Rust versions.

```rust
use anyhow::{Context, Result};
use turso::{Connection, Row};

use super::models::{ChunkRow, Job, Note, SearchHit};

// ─── helpers ────────────────────────────────────────────────────────────────

pub fn content_hash(text: &str) -> String {
    blake3::hash(text.as_bytes()).to_hex().to_string()
}

/// COMMIT if `result` is Ok, ROLLBACK otherwise. Returns `result` unchanged.
pub(crate) async fn finish_tx<T>(conn: &Connection, result: Result<T>) -> Result<T> {
    match result {
        Ok(value) => {
            conn.execute("COMMIT", ()).await?;
            Ok(value)
        }
        Err(e) => {
            // If rollback fails too, the original error is the interesting one.
            let _ = conn.execute("ROLLBACK", ()).await;
            Err(e)
        }
    }
}

const NOTE_COLUMNS: &str = "id, created_at, updated_at, event_at, context, content";

fn note_from_row(row: &Row) -> Result<Note> {
    Ok(Note {
        id: row.get(0)?,
        created_at: row.get(1)?,
        updated_at: row.get(2)?,
        event_at: row.get(3)?,
        context: row.get(4)?,
        content: row.get(5)?,
    })
}

// ─── meta ───────────────────────────────────────────────────────────────────

pub(crate) async fn get_meta(conn: &Connection, key: &str) -> Result<Option<String>> {
    let mut rows = conn
        .query("SELECT value FROM meta WHERE key = ?1", (key,))
        .await?;
    match rows.next().await? {
        Some(row) => Ok(Some(row.get(0)?)),
        None => Ok(None),
    }
}

pub(crate) async fn set_meta(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)",
        (key, value),
    )
    .await?;
    Ok(())
}

// ─── notes ──────────────────────────────────────────────────────────────────

/// Inserts a note and enqueues its embedding job, atomically.
pub async fn insert_note(conn: &Connection, content: &str) -> Result<i64> {
    let hash = content_hash(content);

    conn.execute("BEGIN", ()).await?;
    let result: Result<i64> = async {
        let id: i64 = {
            let mut rows = conn
                .query(
                    "INSERT INTO notes (content, content_hash) VALUES (?1, ?2) RETURNING id",
                    (content, hash.as_str()),
                )
                .await?;
            rows.next()
                .await?
                .context("INSERT ... RETURNING produced no row")?
                .get(0)?
        };
        enqueue_job(conn, "embed", id).await?;
        Ok(id)
    }
    .await;
    finish_tx(conn, result).await
}

/// Updates a note's content. Returns `false` if the content didn't change
/// (nothing is written and no job is enqueued).
pub async fn update_note(conn: &Connection, id: i64, content: &str) -> Result<bool> {
    let new_hash = content_hash(content);

    conn.execute("BEGIN", ()).await?;
    let result: Result<bool> = async {
        let old_hash: String = {
            let mut rows = conn
                .query("SELECT content_hash FROM notes WHERE id = ?1", (id,))
                .await?;
            rows.next()
                .await?
                .with_context(|| format!("note {id} not found"))?
                .get(0)?
        };

        if old_hash == new_hash {
            return Ok(false);
        }

        conn.execute(
            "UPDATE notes
             SET content = ?1, content_hash = ?2, updated_at = datetime('now')
             WHERE id = ?3",
            (content, new_hash.as_str(), id),
        )
        .await?;
        enqueue_job(conn, "embed", id).await?;
        Ok(true)
    }
    .await;
    finish_tx(conn, result).await
}

pub async fn get_note(conn: &Connection, id: i64) -> Result<Option<Note>> {
    let sql = format!("SELECT {NOTE_COLUMNS} FROM notes WHERE id = ?1");
    let mut rows = conn.query(&sql, (id,)).await?;
    match rows.next().await? {
        Some(row) => Ok(Some(note_from_row(&row)?)),
        None => Ok(None),
    }
}

/// Most recently edited first.
pub async fn recent_notes(conn: &Connection, limit: i64) -> Result<Vec<Note>> {
    let sql = format!("SELECT {NOTE_COLUMNS} FROM notes ORDER BY updated_at DESC, id DESC LIMIT ?1");
    let mut rows = conn.query(&sql, (limit,)).await?;
    let mut notes = Vec::new();
    while let Some(row) = rows.next().await? {
        notes.push(note_from_row(&row)?);
    }
    Ok(notes)
}

/// Chunks, labels and jobs are removed by ON DELETE CASCADE.
pub async fn delete_note(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM notes WHERE id = ?1", (id,)).await?;
    Ok(())
}

pub async fn add_label(conn: &Connection, note_id: i64, label: &str) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO labels (note_id, label) VALUES (?1, ?2)",
        (note_id, label),
    )
    .await?;
    Ok(())
}

// ─── search ─────────────────────────────────────────────────────────────────

pub async fn search_chunks(conn: &Connection, query_vec: &[f32], limit: i64) -> Result<Vec<SearchHit>> {
    let query_json = serde_json::to_string(query_vec)?;
    let mut rows = conn
        .query(
            "SELECT c.note_id, c.chunk_index, c.byte_start, c.content, n.created_at,
                    vector_distance_cos(c.embedding, vector(?1)) AS distance
             FROM chunks c
             JOIN notes n ON n.id = c.note_id
             WHERE c.embedding IS NOT NULL
             ORDER BY distance ASC
             LIMIT ?2",
            (query_json, limit),
        )
        .await?;

    let mut hits = Vec::new();
    while let Some(row) = rows.next().await? {
        hits.push(SearchHit {
            note_id: row.get(0)?,
            chunk_index: row.get(1)?,
            byte_start: row.get(2)?,
            content: row.get(3)?,
            note_created_at: row.get(4)?,
            distance: row.get(5)?,
        });
    }
    Ok(hits)
}

// ─── jobs ───────────────────────────────────────────────────────────────────

/// REPLACE gives the row a new id. A worker still processing the old job will
/// see its id gone and discard its (stale) result. See `complete_embed_job`.
async fn enqueue_job(conn: &Connection, kind: &str, note_id: i64) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO jobs (kind, note_id, status, attempts)
         VALUES (?1, ?2, 'pending', 0)",
        (kind, note_id),
    )
    .await?;
    Ok(())
}

/// Picks the oldest pending job and marks it running.
/// SELECT-then-UPDATE is only safe because the actor runs one call at a time.
pub async fn claim_next_job(conn: &Connection, kind: &str) -> Result<Option<Job>> {
    let job = {
        let mut rows = conn
            .query(
                "SELECT id, note_id, attempts FROM jobs
                 WHERE kind = ?1 AND status = 'pending'
                 ORDER BY id LIMIT 1",
                (kind,),
            )
            .await?;
        match rows.next().await? {
            Some(row) => Job {
                id: row.get(0)?,
                note_id: row.get(1)?,
                attempts: row.get(2)?,
            },
            None => return Ok(None),
        }
    };

    conn.execute(
        "UPDATE jobs SET status = 'running', attempts = attempts + 1 WHERE id = ?1",
        (job.id,),
    )
    .await?;
    Ok(Some(job))
}

/// Replaces the note's chunks and deletes the job, atomically.
/// Returns `false` if the job was superseded by a newer save (nothing written).
pub async fn complete_embed_job(
    conn: &Connection,
    job_id: i64,
    note_id: i64,
    chunks: &[ChunkRow],
) -> Result<bool> {
    conn.execute("BEGIN", ()).await?;
    let result: Result<bool> = async {
        let still_current = {
            let mut rows = conn
                .query(
                    "SELECT 1 FROM jobs WHERE id = ?1 AND status = 'running'",
                    (job_id,),
                )
                .await?;
            rows.next().await?.is_some()
        };
        if !still_current {
            return Ok(false);
        }

        conn.execute("DELETE FROM chunks WHERE note_id = ?1", (note_id,))
            .await?;

        for c in chunks {
            let vector_json = serde_json::to_string(&c.embedding)?;
            conn.execute(
                "INSERT INTO chunks (note_id, chunk_index, byte_start, content, embedding)
                 VALUES (?1, ?2, ?3, ?4, vector(?5))",
                (note_id, c.index, c.byte_start, c.content.as_str(), vector_json),
            )
            .await?;
        }

        conn.execute("DELETE FROM jobs WHERE id = ?1", (job_id,))
            .await?;
        Ok(true)
    }
    .await;
    finish_tx(conn, result).await
}

/// Back to 'pending' for a retry, or 'failed' after too many attempts.
pub async fn fail_job(conn: &Connection, job_id: i64, error: &str, max_attempts: i64) -> Result<()> {
    conn.execute(
        "UPDATE jobs
         SET status = CASE WHEN attempts >= ?3 THEN 'failed' ELSE 'pending' END,
             last_error = ?2
         WHERE id = ?1",
        (job_id, error, max_attempts),
    )
    .await?;
    Ok(())
}

/// Called at startup: jobs left 'running' were interrupted by a crash or quit.
pub async fn reset_running_jobs(conn: &Connection) -> Result<()> {
    conn.execute("UPDATE jobs SET status = 'pending' WHERE status = 'running'", ())
        .await?;
    Ok(())
}

/// If the stored model id differs from `model_id`, store the new one and
/// enqueue every note for re-embedding. Returns true if a re-embed was queued.
pub async fn ensure_embedding_model(conn: &Connection, model_id: &str) -> Result<bool> {
    if get_meta(conn, "embedding_model").await?.as_deref() == Some(model_id) {
        return Ok(false);
    }

    conn.execute("BEGIN", ()).await?;
    let result: Result<bool> = async {
        set_meta(conn, "embedding_model", model_id).await?;
        conn.execute(
            "INSERT OR REPLACE INTO jobs (kind, note_id, status, attempts)
             SELECT 'embed', id, 'pending', 0 FROM notes",
            (),
        )
        .await?;
        Ok(true)
    }
    .await;
    finish_tx(conn, result).await
}
```

> ⚠️ verify: the exact type behind `rows.next()` (is it `turso::Row`?) and that `Option<String>` parameters/columns map to NULL. Your current code already reads `Option<String>` from a row, so reading works.

**The stale-job trick, explained.** Picture this timeline:

1. Save → job #7 pending.
2. Worker claims #7 and starts embedding (slow).
3. You edit and save again → `INSERT OR REPLACE` deletes #7 and creates #8 pending.
4. Worker finishes #7 → `complete_embed_job(7, ...)` finds no running job #7, writes nothing, returns `false`.
5. Worker claims #8 → embeds the new text.

No locks or version numbers, just the row id. Draw it on paper once; it's a nice pattern to have in your head.

### 3.4 `src/db.rs`: module root + `open`

Keep the file-style module root you already use (`src/db.rs` + `src/db/` folder). It's equivalent to `src/db/mod.rs`.

```rust
pub mod actor;
pub mod models;
pub mod repo;
mod schema;

pub use actor::{DbHandle, spawn_actor};
pub use models::*;

use anyhow::{Context, Result};
use std::path::Path;
use turso::{Builder, Connection};

/// Opens (or creates) the database, applies migrations and recovers jobs.
/// Pass `Path::new(":memory:")` for tests.
pub async fn open(path: &Path) -> Result<Connection> {
    let path_str = path.to_str().context("database path is not valid UTF-8")?;
    let db = Builder::new_local(path_str)
        .build()
        .await
        .with_context(|| format!("failed to open database at {path_str}"))?;
    let conn = db.connect()?;

    conn.execute("PRAGMA foreign_keys = ON", ()).await?;
    schema::migrate(&conn).await?;
    repo::reset_running_jobs(&conn).await?;
    Ok(conn)
}
```

> Your current code already drops `db` after `connect()`, and it works. If you ever see odd connection errors, keep the `Database` alive (e.g. return it alongside the connection).

### ✅ Checkpoint 3

`cargo check` should pass for `db`, `embed` and `chunk` (stub `worker.rs` and `actor.rs` with nothing inside for now, or comment out their `mod` lines). Then write the tests in [Phase 6.2](#62-integration-tests-testsdbrs) and run them. They only need the DB, not the model.

---

## Phase 4 — DB actor and background worker

### 4.1 Why an actor

The UI and the worker will both use the database. If they share one `Connection`, a `BEGIN` from the UI and an `INSERT` from the worker can interleave inside the same transaction, which is a subtle, rare, terrible bug. Separate connections avoid that but bring busy/locking errors.

**Actor pattern:** one tokio task owns the connection and handles requests one at a time from a channel. Everyone else holds a cheap, cloneable `DbHandle` that sends a request plus a `oneshot` channel for the answer.

You get serialized access (transactions are safe), a single place for logging and timing all DB calls, and a `Notify` that wakes the worker exactly when a job is enqueued.

### 4.2 `src/db/actor.rs`

```rust
use anyhow::{Result, anyhow};
use std::sync::Arc;
use tokio::sync::{Notify, mpsc, oneshot};
use turso::Connection;

use super::models::{ChunkRow, Job, Note, SearchHit};
use super::repo;
use crate::config::JOB_MAX_ATTEMPTS;

type Reply<T> = oneshot::Sender<Result<T>>;

/// One variant per operation. Each carries its arguments and a reply channel.
enum DbCmd {
    InsertNote { content: String, reply: Reply<i64> },
    UpdateNote { id: i64, content: String, reply: Reply<bool> },
    GetNote { id: i64, reply: Reply<Option<Note>> },
    RecentNotes { limit: i64, reply: Reply<Vec<Note>> },
    DeleteNote { id: i64, reply: Reply<()> },
    Search { query_vec: Vec<f32>, limit: i64, reply: Reply<Vec<SearchHit>> },
    ClaimEmbedJob { reply: Reply<Option<Job>> },
    CompleteEmbedJob { job_id: i64, note_id: i64, chunks: Vec<ChunkRow>, reply: Reply<bool> },
    FailJob { job_id: i64, error: String, reply: Reply<()> },
    EnsureEmbeddingModel { model_id: String, reply: Reply<bool> },
}

/// Cheap to clone; every clone talks to the same database task.
#[derive(Debug, Clone)]
pub struct DbHandle {
    tx: mpsc::Sender<DbCmd>,
    job_signal: Arc<Notify>,
}

/// Moves `conn` into a background task. Must be called inside a tokio runtime.
pub fn spawn_actor(conn: Connection) -> DbHandle {
    let (tx, mut rx) = mpsc::channel::<DbCmd>(64);

    tokio::spawn(async move {
        while let Some(cmd) = rx.recv().await {
            // `let _ =`: if the caller stopped waiting, nobody wants the answer.
            match cmd {
                DbCmd::InsertNote { content, reply } => {
                    let _ = reply.send(repo::insert_note(&conn, &content).await);
                }
                DbCmd::UpdateNote { id, content, reply } => {
                    let _ = reply.send(repo::update_note(&conn, id, &content).await);
                }
                DbCmd::GetNote { id, reply } => {
                    let _ = reply.send(repo::get_note(&conn, id).await);
                }
                DbCmd::RecentNotes { limit, reply } => {
                    let _ = reply.send(repo::recent_notes(&conn, limit).await);
                }
                DbCmd::DeleteNote { id, reply } => {
                    let _ = reply.send(repo::delete_note(&conn, id).await);
                }
                DbCmd::Search { query_vec, limit, reply } => {
                    let _ = reply.send(repo::search_chunks(&conn, &query_vec, limit).await);
                }
                DbCmd::ClaimEmbedJob { reply } => {
                    let _ = reply.send(repo::claim_next_job(&conn, "embed").await);
                }
                DbCmd::CompleteEmbedJob { job_id, note_id, chunks, reply } => {
                    let _ = reply.send(repo::complete_embed_job(&conn, job_id, note_id, &chunks).await);
                }
                DbCmd::FailJob { job_id, error, reply } => {
                    let _ = reply.send(repo::fail_job(&conn, job_id, &error, JOB_MAX_ATTEMPTS).await);
                }
                DbCmd::EnsureEmbeddingModel { model_id, reply } => {
                    let _ = reply.send(repo::ensure_embedding_model(&conn, &model_id).await);
                }
            }
        }
        // All handles dropped: the loop ends and `conn` is dropped cleanly.
    });

    DbHandle {
        tx,
        job_signal: Arc::new(Notify::new()),
    }
}

impl DbHandle {
    /// Sends a command built by `make` and waits for its reply.
    async fn request<T>(&self, make: impl FnOnce(Reply<T>) -> DbCmd) -> Result<T> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(make(reply))
            .await
            .map_err(|_| anyhow!("database task has stopped"))?;
        rx.await
            .map_err(|_| anyhow!("database task dropped the request"))?
    }

    pub async fn insert_note(&self, content: String) -> Result<i64> {
        let id = self.request(|reply| DbCmd::InsertNote { content, reply }).await?;
        self.job_signal.notify_one();
        Ok(id)
    }

    pub async fn update_note(&self, id: i64, content: String) -> Result<bool> {
        let changed = self.request(|reply| DbCmd::UpdateNote { id, content, reply }).await?;
        if changed {
            self.job_signal.notify_one();
        }
        Ok(changed)
    }

    pub async fn get_note(&self, id: i64) -> Result<Option<Note>> {
        self.request(|reply| DbCmd::GetNote { id, reply }).await
    }

    pub async fn recent_notes(&self, limit: i64) -> Result<Vec<Note>> {
        self.request(|reply| DbCmd::RecentNotes { limit, reply }).await
    }

    pub async fn delete_note(&self, id: i64) -> Result<()> {
        self.request(|reply| DbCmd::DeleteNote { id, reply }).await
    }

    pub async fn search(&self, query_vec: Vec<f32>, limit: i64) -> Result<Vec<SearchHit>> {
        self.request(|reply| DbCmd::Search { query_vec, limit, reply }).await
    }

    pub async fn claim_embed_job(&self) -> Result<Option<Job>> {
        self.request(|reply| DbCmd::ClaimEmbedJob { reply }).await
    }

    pub async fn complete_embed_job(&self, job_id: i64, note_id: i64, chunks: Vec<ChunkRow>) -> Result<bool> {
        self.request(|reply| DbCmd::CompleteEmbedJob { job_id, note_id, chunks, reply }).await
    }

    pub async fn fail_job(&self, job_id: i64, error: String) -> Result<()> {
        self.request(|reply| DbCmd::FailJob { job_id, error, reply }).await
    }

    pub async fn ensure_embedding_model(&self, model_id: &str) -> Result<bool> {
        let model_id = model_id.to_string();
        let queued = self
            .request(|reply| DbCmd::EnsureEmbeddingModel { model_id, reply })
            .await?;
        if queued {
            self.job_signal.notify_one();
        }
        Ok(queued)
    }

    /// Resolves when a job was enqueued since the last call.
    pub async fn jobs_available(&self) {
        self.job_signal.notified().await;
    }
}
```

**What you'll learn:**

- `oneshot` = a channel for exactly one value: the classic "request/response over channels" pattern.
- `Notify::notify_one` stores a permit if nobody is waiting, so a job enqueued *while the worker is busy* isn't lost. The next `notified().await` returns immediately.
- `request` takes a closure that builds the command. Read its signature slowly; it's the trick that keeps each public method to one line.

### 4.3 `src/worker.rs`

```rust
use anyhow::{Context, Result};
use std::sync::Arc;
use std::time::Duration;

use crate::chunk::Chunker;
use crate::db::{ChunkRow, DbHandle, Job};
use crate::embed::Embedder;

/// Runs forever: claims embed jobs, chunks + embeds the note, stores the result.
pub async fn run_embed_worker(db: DbHandle, embedder: Arc<Embedder>, chunker: Arc<Chunker>) {
    match db.ensure_embedding_model(embedder.model_id()).await {
        Ok(true) => eprintln!("embedding model changed: re-embedding all notes"),
        Ok(false) => {}
        Err(e) => eprintln!("could not check embedding model: {e:#}"),
    }

    loop {
        match db.claim_embed_job().await {
            Ok(Some(job)) => {
                if let Err(e) = process(&db, &embedder, &chunker, job).await {
                    eprintln!("embed job {} (note {}) failed: {e:#}", job.id, job.note_id);
                    let _ = db.fail_job(job.id, format!("{e:#}")).await;
                }
            }
            Ok(None) => {
                // Idle: wait for a signal; poll every 30 s as a safety net.
                tokio::select! {
                    _ = db.jobs_available() => {}
                    _ = tokio::time::sleep(Duration::from_secs(30)) => {}
                }
            }
            Err(e) => {
                // Probably the DB task is gone (app shutting down).
                eprintln!("worker: {e:#}");
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
    }
}

async fn process(db: &DbHandle, embedder: &Arc<Embedder>, chunker: &Arc<Chunker>, job: Job) -> Result<()> {
    let note = db
        .get_note(job.note_id)
        .await?
        .with_context(|| format!("note {} no longer exists", job.note_id))?;

    // CPU-heavy: move to the blocking pool so tokio's async threads stay free.
    let embedder = Arc::clone(embedder);
    let chunker = Arc::clone(chunker);
    let rows = tokio::task::spawn_blocking(move || -> Result<Vec<ChunkRow>> {
        let pieces = chunker.split(&note.content);
        let texts: Vec<&str> = pieces.iter().map(|(_, t)| *t).collect();
        let vectors = embedder.embed_batch(&texts)?;

        Ok(pieces
            .into_iter()
            .zip(vectors)
            .enumerate()
            .map(|(i, ((start, text), embedding))| ChunkRow {
                index: i as i64,
                byte_start: start as i64,
                content: text.to_string(),
                embedding,
            })
            .collect())
    })
    .await??;

    let written = db.complete_embed_job(job.id, job.note_id, rows).await?;
    if !written {
        // A newer save replaced this job; its own run will write fresh chunks.
    }
    Ok(())
}
```

**What you'll learn:**

- `.await??`: the first `?` handles the `JoinError` (the blocking thread panicked), the second the `Result` your closure returned.
- `tokio::select!` waits on several futures and runs the branch of whichever finishes first.
- Why `spawn_blocking`: an embedding batch takes tens to hundreds of ms of pure CPU. Doing that on an async thread would stall everything else scheduled there, including DB replies to the UI.

> **Batch size.** A very long note becomes many chunks, embedded in one batch. If memory spikes, embed in slices with `texts.chunks(16)` and concatenate the results. That makes a nice small exercise.

### ✅ Checkpoint 4

`cargo check` passes for the whole library. You can't see the worker run until Phase 5, but the CLI from Phase 6 can exercise the actor if you want to test it earlier.

---

## Phase 5 — UI

### 5.1 What changes

| Before | After |
|--------|-------|
| One init waits for DB **and** model | DB first → editor usable immediately; model loads after |
| `self.content.text()` in `view` (every frame) | `char_count` updated only on edits |
| Save always inserts | Insert first time, update afterwards, skip if unchanged |
| Save embeds inline | Save writes + enqueues; worker embeds in background |
| Double-click Save → two inserts | Button disabled while saving or when nothing changed |
| No shortcuts | Ctrl+S, Ctrl+N, Ctrl+Q |
| `std::process::exit` | `iced::exit()` |

### 5.2 Dirty tracking with revisions

A boolean `dirty` flag has a race: you press Save, keep typing during the save, the save completes and sets `dirty = false`, and your newest keystrokes look saved when they aren't.

Fix: a **revision counter**. Each edit increments `revision`. A save remembers which revision it saved. The note is dirty when `revision != saved_revision`. Edits that happen during a save bump `revision` past the saved one, so the note stays dirty. It's correct by construction, with no special cases.

### 5.3 Full new `src/main.rs`

```rust
use iced::widget::{button, column, row, space, text, text_editor};
use iced::{Element, Fill, Padding, Size, Task, Theme, keyboard, window};
use std::sync::Arc;

use notelook::config;
use notelook::db::{self, DbHandle};
use notelook::embed::Embedder;
use notelook::worker;

pub fn main() -> iced::Result {
    iced::application(App::boot, App::update, App::view)
        .title("Notelook")
        .theme(theme)
        .window(window::Settings {
            size: Size::new(900.0, 600.0),
            icon: load_window_icon(),
            ..Default::default()
        })
        .run()
}

fn load_window_icon() -> Option<window::Icon> {
    const PNG_BYTES: &[u8] = include_bytes!("../assets/logo.png");
    let img = image::load_from_memory(PNG_BYTES)
        .map_err(|e| eprintln!("failed to decode icon: {e}"))
        .ok()?
        .to_rgba8();
    let (width, height) = img.dimensions();
    window::icon::from_rgba(img.into_raw(), width, height)
        .map_err(|e| eprintln!("failed to build icon: {e}"))
        .ok()
}

fn theme(_state: &App) -> Theme {
    Theme::TokyoNightStorm
}

// ─── state ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModelState {
    Loading,
    Ready,
    Failed,
}

struct App {
    content: text_editor::Content,
    /// Incremented on every edit.
    revision: u64,
    /// Revision that was last written to the DB.
    saved_revision: u64,
    char_count: usize,
    saving: bool,
    status: String,
    current_note_id: Option<i64>,
    db: Option<DbHandle>,
    model: ModelState,
}

impl App {
    fn is_dirty(&self) -> bool {
        self.revision != self.saved_revision
    }

    fn can_save(&self) -> bool {
        self.db.is_some() && !self.saving && self.is_dirty()
    }
}

#[derive(Debug, Clone, Copy)]
enum SaveOutcome {
    Created(i64),
    Updated(i64),
    Unchanged(i64),
}

impl SaveOutcome {
    fn note_id(self) -> i64 {
        match self {
            Self::Created(id) | Self::Updated(id) | Self::Unchanged(id) => id,
        }
    }
}

#[derive(Debug, Clone)]
enum Message {
    DbReady(Result<DbHandle, String>),
    ModelReady(Result<(), String>),
    Edit(text_editor::Action),
    NewNote,
    SaveNote,
    NoteSaved {
        revision: u64,
        result: Result<SaveOutcome, String>,
    },
    Quit,
}

// ─── async startup steps (plain functions: easy to read and test) ───────────

async fn open_database() -> anyhow::Result<DbHandle> {
    let path = config::db_path()?;
    let conn = db::open(&path).await?;
    Ok(db::spawn_actor(conn))
}

async fn start_model(db: DbHandle) -> anyhow::Result<()> {
    let embedder = tokio::task::spawn_blocking(Embedder::try_new).await??;
    let chunker = embedder.chunker(config::CHUNK_MAX_TOKENS, config::CHUNK_OVERLAP)?;
    tokio::spawn(worker::run_embed_worker(db, Arc::new(embedder), Arc::new(chunker)));
    Ok(())
}

// ─── update ─────────────────────────────────────────────────────────────────

impl App {
    fn boot() -> (Self, Task<Message>) {
        let app = Self {
            content: text_editor::Content::new(),
            revision: 0,
            saved_revision: 0,
            char_count: 0,
            saving: false,
            status: String::from("Opening database…"),
            current_note_id: None,
            db: None,
            model: ModelState::Loading,
        };

        let task = Task::perform(
            async { open_database().await.map_err(|e| format!("{e:#}")) },
            Message::DbReady,
        );
        (app, task)
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::DbReady(Ok(db)) => {
                self.db = Some(db.clone());
                self.status = String::from("Ready");
                // Step 2 of startup: load the model in the background.
                Task::perform(
                    async move { start_model(db).await.map_err(|e| format!("{e:#}")) },
                    Message::ModelReady,
                )
            }
            Message::DbReady(Err(e)) => {
                self.status = format!("Database error: {e}");
                eprintln!("{e}");
                Task::none()
            }

            Message::ModelReady(Ok(())) => {
                self.model = ModelState::Ready;
                Task::none()
            }
            Message::ModelReady(Err(e)) => {
                // Notes still save; their jobs wait in the queue until next start.
                self.model = ModelState::Failed;
                self.status = format!("Model error (notes still save): {e}");
                eprintln!("{e}");
                Task::none()
            }

            Message::Edit(action) => {
                let is_edit = action.is_edit();
                self.content.perform(action);
                if is_edit {
                    self.revision += 1;
                    // Once per keystroke is fine; once per frame was not.
                    self.char_count = self.content.text().chars().count();
                }
                Task::none()
            }

            Message::NewNote => {
                if self.is_dirty() {
                    self.status = String::from("Unsaved changes: press Ctrl+S first");
                    return Task::none();
                }
                self.content = text_editor::Content::new();
                self.current_note_id = None;
                self.revision = 0;
                self.saved_revision = 0;
                self.char_count = 0;
                self.status = String::from("New note");
                Task::none()
            }

            Message::SaveNote => {
                if !self.can_save() {
                    return Task::none();
                }
                let note_text = self.content.text();
                if note_text.trim().is_empty() {
                    self.status = String::from("Cannot save an empty note");
                    return Task::none();
                }
                let Some(db) = self.db.clone() else {
                    return Task::none();
                };

                self.saving = true;
                self.status = String::from("Saving…");
                let revision = self.revision;
                let note_id = self.current_note_id;

                Task::perform(
                    async move {
                        let outcome = match note_id {
                            None => SaveOutcome::Created(db.insert_note(note_text).await?),
                            Some(id) => {
                                if db.update_note(id, note_text).await? {
                                    SaveOutcome::Updated(id)
                                } else {
                                    SaveOutcome::Unchanged(id)
                                }
                            }
                        };
                        anyhow::Ok(outcome)
                    },
                    move |result| Message::NoteSaved {
                        revision,
                        result: result.map_err(|e| format!("{e:#}")),
                    },
                )
            }

            Message::NoteSaved { revision, result } => {
                self.saving = false;
                match result {
                    Ok(outcome) => {
                        self.current_note_id = Some(outcome.note_id());
                        self.saved_revision = revision;
                        self.status = match outcome {
                            SaveOutcome::Created(id) => format!("Note {id} created"),
                            SaveOutcome::Updated(id) => format!("Note {id} updated"),
                            SaveOutcome::Unchanged(id) => format!("Note {id}: no changes"),
                        };
                    }
                    Err(e) => self.status = format!("Save failed: {e}"),
                }
                Task::none()
            }

            Message::Quit => iced::exit(),
        }
    }

    // ─── view ───────────────────────────────────────────────────────────────

    fn view(&self) -> Element<'_, Message> {
        let size = 13;

        let dirty_mark = if self.is_dirty() { "● " } else { "" };
        let model_label = match self.model {
            ModelState::Loading => "model loading…",
            ModelState::Ready => "search ready",
            ModelState::Failed => "model unavailable",
        };
        let stats = format!(
            "{dirty_mark}Lines: {} | Chars: {} | {model_label}",
            self.content.line_count(),
            self.char_count
        );

        let action_bar = row![
            button("New").on_press(Message::NewNote),
            button("Save").on_press_maybe(self.can_save().then_some(Message::SaveNote)),
            space::horizontal(),
            text(&self.status).size(size),
            text(" · ").size(size),
            text(stats).size(size),
        ]
        .spacing(10)
        .align_y(iced::Alignment::Center);

        let editor = text_editor(&self.content)
            .placeholder("Type your note here…")
            .on_action(Message::Edit)
            .key_binding(editor_bindings)
            .height(Fill);

        column![action_bar, editor]
            .spacing(12)
            .padding(Padding::new(16.0))
            .width(Fill)
            .height(Fill)
            .into()
    }
}

/// Ctrl+S / Ctrl+N / Ctrl+Q while the editor has focus; everything else
/// falls through to the editor's default bindings.
fn editor_bindings(key_press: text_editor::KeyPress) -> Option<text_editor::Binding<Message>> {
    use keyboard::Key;

    if key_press.modifiers.command() {
        let message = match key_press.key.as_ref() {
            Key::Character("s") => Some(Message::SaveNote),
            Key::Character("n") => Some(Message::NewNote),
            Key::Character("q") => Some(Message::Quit),
            _ => None,
        };
        if let Some(message) = message {
            return Some(text_editor::Binding::Custom(message));
        }
    }

    text_editor::Binding::from_key_press(key_press)
}
```

> ⚠️ verify against the iced 0.14 docs: `Action::is_edit`, the fields of `text_editor::KeyPress`, `Binding::from_key_press`, and `on_press_maybe`. These exist in recent iced versions, but names sometimes shift. `modifiers.command()` means Ctrl on Linux/Windows and Cmd on macOS.

> Bindings on the editor only fire while it has focus, which in a single-editor app is nearly always. For global shortcuts later, look at iced's keyboard subscriptions.

**What you'll learn:**

- `anyhow::Ok(outcome)`: inside an `async move` block the compiler can't infer the error type for `?`. `anyhow::Ok` pins it to `anyhow::Error`. Remove it and read the error message.
- The second argument of `Task::perform` is a closure, so you can capture `revision` and build a richer message, not just pass `Message::NoteSaved`.
- `bool::then_some` turns a condition into an `Option`, which is what `on_press_maybe` wants.

### ✅ Checkpoint 5

Run `cargo run` and check each of these:

1. The editor appears immediately; the status bar says "model loading…", then "search ready".
2. Type, press Ctrl+S → "Note 1 created". The ● disappears.
3. Press Ctrl+S again without changes → nothing happens (the button is disabled).
4. Edit, save → "Note 1 updated", **not** a new note.
5. Quit, then inspect the DB with the turso shell (`tursodb ~/.local/share/notelook/notes.db`, ⚠️ verify the binary name) or with `sqlite3` if the file format is compatible:
   ```sql
   SELECT id, updated_at, length(content) FROM notes;
   SELECT note_id, chunk_index, byte_start, substr(content, 1, 40) FROM chunks;
   SELECT * FROM jobs;   -- should be empty once the worker caught up
   ```
6. Save, then quit *immediately*. On the next start, the job is reset to pending and processed.

---

## Phase 6 — CLI and tests

### 6.1 `src/bin/cli.rs` (replaces `main_cli.rs`)

The CLI talks to `repo` **directly**, without the actor. It's one process doing one thing at a time, so the actor isn't needed. That's one benefit of keeping `repo` as plain functions.

```rust
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
            let text = std::fs::read_to_string(file)
                .with_context(|| format!("cannot read {file}"))?;
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
```

**Exercise:** add a `cli process-jobs` command that loads the embedder and runs the claim → embed → complete loop until no jobs remain, using `repo` functions instead of the actor. You'll see how small the worker logic is once DB access is a plain function. Then delete the `Duration` line.

### 6.2 Integration tests (`tests/db.rs`)

These use an in-memory DB, are fast, and don't need the model. ⚠️ verify that turso accepts `":memory:"` with `Builder::new_local`.

```rust
use anyhow::Result;
use std::path::Path;

use notelook::db::{self, ChunkRow, repo};

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
```

If `delete_cascades_to_chunks_and_jobs` fails, your turso version doesn't enforce foreign keys. In that case, delete chunks/labels/jobs explicitly inside a transaction in `repo::delete_note`. Good thing you have a test for it.

**Exercise:** make `migrations_are_idempotent` meaningful by using a temporary file (`std::env::temp_dir().join("notelook-test.db")`), opening it twice, and removing it at the end.

### ✅ Checkpoint 6

`cargo test` passes (the model test stays ignored), and `cargo run --bin cli -- list` shows the notes saved from the GUI.

---

## 7. Refactoring ideas

These are ideas to apply once the phases above work. Each one is a self-contained learning exercise.

### 7.1 Newtype ids

`i64` everywhere means you can pass a chunk id where a note id is expected. Wrap them:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NoteId(pub i64);
```

The compiler then catches mix-ups. You'll learn how to make a newtype work with turso params and rows (implement the conversion traits, or convert at the repo boundary).

### 7.2 Split the UI into modules

When the UI gains a sidebar and a search panel, `main.rs` will grow fast. A structure that scales with iced's Elm-like architecture:

```
src/ui/
├── mod.rs        App, Message, update, view (dispatch only)
├── editor.rs     editor state + its own Message enum
├── sidebar.rs    note list
└── search.rs     search box + results
```

Each component has `struct State`, `enum Message`, `fn update(&mut self, Message) -> Task<Message>` and `fn view(&self) -> Element<Message>`. The parent wraps child messages: `Message::Editor(editor::Message)` and uses `.map(Message::Editor)` on the child's view and tasks.

### 7.3 Logging with `tracing`

Replace `eprintln!` with `tracing::{info, warn, error}` and `tracing-subscriber`. You get levels, timestamps, and the option to log to a file, which matters on Windows where a GUI app has no console. Add a `#[tracing::instrument]` to the actor's dispatch to time every DB call.

### 7.4 A trait for the embedder

```rust
pub trait Embed: Send + Sync {
    fn model_id(&self) -> &str;
    fn embed_batch(&self, texts: &[&str]) -> anyhow::Result<Vec<Vec<f32>>>;
}
```

The worker takes `Arc<dyn Embed>`. In tests, a fake embedder returns deterministic vectors (e.g. a hash of the text spread over 384 floats), so you can test the full save → embed → search pipeline without downloading a model.

### 7.5 Reduce actor boilerplate

Every new DB operation needs a variant, a match arm and a method. Once it annoys you, write a `macro_rules!` that generates all three from one line. It's a good first macro, because you already know exactly what code it must produce.

### 7.6 Clippy as a teacher

```bash
cargo clippy --all-targets -- -W clippy::pedantic
```

Pedantic is noisy, but read each warning once. Many teach idioms you wouldn't find otherwise. Keep the ones you agree with as `#![warn(...)]` in `lib.rs`.

### 7.7 Graceful shutdown

Right now the worker loops forever and dies with the process. Add a `tokio_util::sync::CancellationToken` (or a `watch` channel) that `Message::Quit` triggers. The worker finishes its current job and exits, then the app closes.

---

## 8. Roadmap — what comes after

Ordered by how directly each step serves your target questions ("when did we speak of this?", "who mentioned X?", "when was topic Y mentioned?", "during Z, was W ever said?").

### 8.1 Notes sidebar + opening notes

- `recent_notes` → list on the left; click → load into the editor (`text_editor::Content::with_text`), set `current_note_id`, reset revisions.
- Decide what happens to unsaved changes when switching (auto-save is the most streamlined option).

### 8.2 Search panel

- The UI needs its own `Arc<Embedder>` to embed queries: change `ModelReady` to carry it (`Embedder` already implements `Debug` and `Clone`).
- Flow: query text → `spawn_blocking(embed)` → `db.search(vec, 30)` → group hits by note → show date + snippet.
- Clicking a hit opens the note and moves the cursor to `byte_start`.
- **Debounce:** search 250 ms after the last keystroke, not on every key.

### 8.3 Context and event time (for "during Z…")

- UI: a small context field above the editor (autocomplete from `SELECT DISTINCT context FROM notes`).
- Optional convention: a first line like `@ weekly standup 2026-10-05` fills `context` and `event_at` automatically.
- Search: `WHERE n.context = ?` before ranking.

### 8.4 Query syntax

Parse a single search bar into structured filters:

```
migration @marco in:standup after:2026-09
```

```rust
#[derive(Debug, Default, PartialEq)]
pub struct Query {
    pub text: String,              // "migration": ranked by vector search
    pub people: Vec<String>,       // @marco
    pub context: Option<String>,   // in:standup
    pub after: Option<String>,     // after:2026-09
    pub before: Option<String>,
}
```

Write `fn parse(input: &str) -> Query` **test-first**. List 10 input/output pairs as unit tests, then make them pass. A parser with tests is the most satisfying code you can write by hand.

### 8.5 Speaker attribution (for "who mentioned X?")

Start without ML. A line beginning with `Name:` assigns the rest of the line to that speaker:

```
Marco: we should drop Postgres for this service
Anna: agreed, but not before Q1
```

- During chunking, record `speaker` per line. A `chunk_lines` table (chunk_id, line_no, speaker, text) or a `speaker` column on line-level chunks.
- Edge cases to test: `Note: this is not a person`, times like `10:30`, URLs `https://`.

### 8.6 Entities (schema v2)

```sql
CREATE TABLE entities (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    kind       TEXT NOT NULL,          -- person | project | org | topic
    canonical  TEXT NOT NULL,
    UNIQUE(kind, canonical)
);
CREATE TABLE entity_aliases (
    entity_id  INTEGER NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
    alias      TEXT NOT NULL,          -- lowercase
    UNIQUE(alias, entity_id)
);
CREATE TABLE mentions (
    entity_id  INTEGER NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
    note_id    INTEGER NOT NULL REFERENCES notes(id) ON DELETE CASCADE,
    chunk_id   INTEGER REFERENCES chunks(id) ON DELETE CASCADE,
    byte_start INTEGER NOT NULL,
    byte_end   INTEGER NOT NULL,
    role       TEXT NOT NULL DEFAULT 'mentioned'   -- mentioned | speaker
);
CREATE INDEX idx_mentions_entity ON mentions(entity_id);
CREATE INDEX idx_mentions_note   ON mentions(note_id);
```

- Extraction: rules first (`@name`, `#tag`, speaker lines), NER later. For NER in Rust, look at GLiNER ports (e.g. `gline-rs`) or a token-classification BERT in candle.
- Add a `'ner'` job kind. The worker infrastructure is already there.
- Entity resolution ("Marco" = "Marco Rossi" = "MR") as a manual merge action in the UI first. Automatic resolution is a research topic; don't start there.

### 8.7 Hybrid search

Vector search misses exact names, IDs and codes. ⚠️ verify whether your turso version supports full-text search. If not, `LIKE '%term%'` on chunks is a fine first step at personal-notes scale. Combine the two rankings with Reciprocal Rank Fusion: `score = Σ 1 / (60 + rank_i)`. It's simple, robust and needs no tuning.

### 8.8 Distribution

- **Model:** first run downloads ~90 MB from Hugging Face. Decide between bundling it in the release, or keeping the download with a clear progress/status message (and offline handling).
- **Linux:** build against an older glibc with `cargo zigbuild --target x86_64-unknown-linux-gnu.2.28`, so the binary runs on older distros. Consider an AppImage.
- **Windows:** add `#![windows_subsystem = "windows"]` at the top of `main.rs` (release builds only, via `cfg_attr(not(debug_assertions), ...)`) so no console window opens. That also means logs need to go to a file (see 7.3).

---

## 9. Master checklist

### Phase 1 — Structure
- [ ] Cargo.toml: `default-run`, `blake3`, `directories`
- [ ] `src/lib.rs` with module declarations
- [ ] `src/config.rs` with constants and `db_path()`
- [ ] Every library function returns `anyhow::Result`
- [ ] Old `app.db` deleted
- [ ] ✅ Checkpoint 1

### Phase 2 — Embedding
- [ ] Attention mask passed to `forward`
- [ ] Query truncation to `EMBED_MAX_TOKENS`
- [ ] Dimension probe at load
- [ ] `tokenizer()` removed, `chunker()` added
- [ ] `src/chunk.rs` with `MarkdownSplitter` + `chunk_indices`
- [ ] Model language decision made
- [ ] ✅ Checkpoint 2: batch test passes, and fails with `None`

### Phase 3 — Database
- [ ] `models.rs`
- [ ] `schema.rs` with `meta` + v1 migration
- [ ] `repo.rs`: notes, meta, search, jobs
- [ ] `db.rs` with `open()`
- [ ] Old `methods.rs` / `tables.rs` deleted
- [ ] ✅ Checkpoint 3: library compiles

### Phase 4 — Actor + worker
- [ ] `actor.rs` with `DbHandle` and `request` helper
- [ ] `Notify` wakes the worker
- [ ] `worker.rs` with claim → embed → complete loop
- [ ] Model-change re-embed via `ensure_embedding_model`
- [ ] ✅ Checkpoint 4

### Phase 5 — UI
- [ ] Two-step startup (DB, then model)
- [ ] Revision-based dirty tracking
- [ ] Insert / update / unchanged save flow
- [ ] `on_press_maybe` save guard
- [ ] Ctrl+S / Ctrl+N / Ctrl+Q
- [ ] `iced::exit()`
- [ ] ✅ Checkpoint 5: all six manual checks pass

### Phase 6 — CLI + tests
- [ ] `src/bin/cli.rs` (add / search / list)
- [ ] `tests/db.rs`: four tests pass
- [ ] Exercise: `cli process-jobs`
- [ ] Exercise: real idempotency test with a temp file
- [ ] ✅ Checkpoint 6

### Afterwards
- [ ] Refactors from section 7, one at a time
- [ ] Roadmap 8.1 → 8.8
