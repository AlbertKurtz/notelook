# Notelook — Architecture Simplification & Feature Guide

A step-by-step master guide to **declutter the architecture**, remove the over-engineered actor and job queue, and build the **missing UI features** (sidebar, note list, and in-app semantic search).

Written to be followed **by hand**, file by file. Each phase ends with a **checkpoint** to compile, test, and run.

---

## Table of Contents

0. [Why We Are Refactoring](#0-why-we-are-refactoring)
1. [Target Architecture & File Layout](#1-target-architecture--file-layout)
2. [Phase 1 — Housekeeping & Dead Code Removal](#phase-1--housekeeping--dead-code-removal)
3. [Phase 2 — The Simplified Database Layer](#phase-2--the-simplified-database-layer)
4. [Phase 3 — Self-Healing Background Embedding](#phase-3--self-healing-background-embedding)
5. [Phase 4 — UI Revolution: Sidebar, Note List & Semantic Search](#phase-4--ui-revolution-sidebar-note-list--semantic-search)
6. [Phase 5 — CLI & Integration Tests](#phase-5--cli--integration-tests)
7. [Master Verification & Next Steps](#7-master-verification--next-steps)

---

## 0. Why We Are Refactoring

### The Problem With the Current Codebase

In the last refactor, the codebase adopted the **Actor Pattern** (`src/db/actor.rs`) and a **Durable SQLite Job Queue** (`jobs` table in `schema.rs`). While this sounded good in theory, in practice it caused three major problems:

1. **The "Quadruple Dispatch" Boilerplate Tax**:
   Every database query had to be declared in **4 separate places**:
   - SQL query function in `repo.rs`
   - Command enum variant in `DbCmd` (`actor.rs`)
   - Channel match arm in `spawn_actor()` (`actor.rs`)
   - Method wrapper on `DbHandle` (`actor.rs`)
   This created hundreds of lines of indirection for basic database calls.

2. **The Connection Myth**:
   The actor was introduced because another LLM believed Turso's `Connection` could not be shared across async tasks. **In reality, `turso::Connection` implements `Clone + Send + Sync`**. You can clone a connection and hand it to any async task without an MPSC channel or oneshot reply.

3. **Enterprise Queue for a 10ms Task**:
   Embedding a 200-word note using `all-MiniLM-L6-v2` takes **~10ms on CPU**. Managing a database table of jobs with retry counters, crash-recovery resets, and state transitions (`pending` -> `running` -> `completed`) is massive over-engineering for a desktop notepad.

4. **Product vs Plumbing Imbalance**:
   The app spent 60% of its lines on queue mechanics, but the user interface remained a blank editor without a note list or search bar!

---

## 1. Target Architecture & File Layout

### The Clean Architecture

```
┌────────────────────────────────────────────────────────┐
│                   iced UI (main.rs)                    │
│  - Sidebar: Recent Notes & Click-to-Open               │
│  - Search Bar: In-App Semantic Search (Cosine Sim %)   │
│  - Editor: Full text editing + Dirty state + Ctrl+S    │
└───────────────┬────────────────────────┬───────────────┘
                │                        │
         Direct │ Calls           Spawns │ Background Task
                ▼                        ▼
    ┌──────────────────────┐  ┌─────────────────────────────────┐
    │     db::Db handle    │  │       worker::embed_note        │
    │  (Clone, Send, Sync) │  │   - Tokenize & MarkdownSplitter │
    │                      │  │   - Candle BERT CPU (10ms)      │
    │  - insert_note()     │  │   - db.save_chunks()            │
    │  - update_note()     │  └────────────────┬────────────────┘
    │  - recent_notes()    │                   │ writes vectors
    │  - search()          │                   │
    └──────────┬───────────┘                   │
               │                               ▼
               └───────────────────▶ ┌──────────────────┐
                                     │   Turso SQLite   │
                                     │  notes & chunks  │
                                     └──────────────────┘
```

### Files to Remove
- `src/db/actor.rs` (replaced by direct `Db` struct)
- `src/db/methods.rs` (dead legacy code)
- `src/db/tables.rs` (dead legacy code)
- Root `app.db`, `app.db-wal` (stray files from early runs)

### Final Clean File Tree
```
notelook/
├── Cargo.toml
├── assets/logo.png
├── src/
│   ├── lib.rs              Library root
│   ├── main.rs             GUI: Sidebar, Search bar, Editor
│   ├── bin/
│   │   └── cli.rs          CLI tool (add, search, list)
│   ├── config.rs           Constants & OS paths
│   ├── chunk.rs            MarkdownSplitter chunker
│   ├── embed.rs            Candle BERT embeddings
│   ├── worker.rs           Self-healing background embedding
│   ├── db.rs               Db struct & connection factory
│   └── db/
│       ├── models.rs       Note, ChunkRow, SearchHit
│       ├── schema.rs       Clean migrations (no jobs table)
│       └── repo.rs         Clean SQL queries
└── tests/
    └── db.rs               Real integration tests
```

---

## Phase 1 — Housekeeping & Dead Code Removal

Let's clean up all obsolete files before writing new code.

### 1.1 Delete Dead Modules
Run the following in your terminal:

```powershell
Remove-Item src\db\methods.rs
Remove-Item src\db\tables.rs
Remove-Item src\db\actor.rs
```

### 1.2 Delete Root Database Files
The app stores real data in `%APPDATA%\notelook\data\notes.db`. The `app.db` files in the repository root are remnants of early testing.

```powershell
Remove-Item -ErrorAction SilentlyContinue app.db, app.db-wal, app.db-shm
```

### 1.3 Update `.gitignore`
Make sure local database files in the workspace are never committed. Check that your `.gitignore` includes:

```gitignore
*.db
*.db-wal
*.db-shm
/target
```

### ✅ Checkpoint 1
You have removed the dead code files. `cargo check` will now fail because `lib.rs` and `db.rs` still reference `actor.rs`. That is expected! We will fix them in Phase 2.

---

## Phase 2 — The Simplified Database Layer

We will replace the actor with an ergonomic `Db` struct that wraps `turso::Connection`. Because `Connection` is `Clone + Send + Sync`, you can clone `Db` cheaply and call methods directly.

### 2.1 `src/db/models.rs`

Remove the `Job` struct (no longer needed). The models file becomes clean and focused.

**Replace the contents of `src/db/models.rs` with:**

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

impl Note {
    /// Helper to get the first non-empty line as a title preview.
    pub fn title(&self) -> &str {
        self.content
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("Untitled Note")
    }
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

impl SearchHit {
    /// Similarity percentage (100% is identical).
    pub fn similarity_percent(&self) -> f64 {
        ((1.0 - self.distance).max(0.0) * 100.0).min(100.0)
    }
}

/// A chunk ready to be written into the `chunks` table.
#[derive(Debug, Clone)]
pub struct ChunkRow {
    pub index: i64,
    pub byte_start: i64,
    pub content: String,
    pub embedding: Vec<f32>,
}
```

**What you'll learn:**
Adding convenience methods directly onto model structs (like `Note::title()` and `SearchHit::similarity_percent()`) keeps UI rendering code clean and concise.

---

### 2.2 `src/db/schema.rs`

Remove the `jobs` table from the schema. We keep the schema versioning migration runner.

**Replace the contents of `src/db/schema.rs` with:**

```rust
use anyhow::{Context, Result};
use turso::Connection;

use super::repo::{finish_tx, get_meta, set_meta};

/// Each entry is one schema version; each version is a list of statements.
const MIGRATIONS: &[&[&str]] = &[
    // ── v1 ──────────────────────────────────────────────
    &[
        "CREATE TABLE IF NOT EXISTS notes (
            id           INTEGER PRIMARY KEY AUTOINCREMENT,
            created_at   TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at   TEXT NOT NULL DEFAULT (datetime('now')),
            event_at     TEXT,
            context      TEXT,
            content      TEXT NOT NULL,
            content_hash TEXT NOT NULL
        )",
        "CREATE TABLE IF NOT EXISTS labels (
            id      INTEGER PRIMARY KEY AUTOINCREMENT,
            note_id INTEGER NOT NULL REFERENCES notes(id) ON DELETE CASCADE,
            label   TEXT NOT NULL,
            UNIQUE(note_id, label)
        )",
        "CREATE TABLE IF NOT EXISTS chunks (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            note_id     INTEGER NOT NULL REFERENCES notes(id) ON DELETE CASCADE,
            chunk_index INTEGER NOT NULL,
            byte_start  INTEGER NOT NULL,
            content     TEXT NOT NULL,
            embedding   F32_BLOB(384),
            UNIQUE(note_id, chunk_index)
        )",
        "CREATE INDEX IF NOT EXISTS idx_chunks_note_id ON chunks(note_id)",
    ],
];

pub async fn migrate(conn: &Connection) -> Result<()> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
        (),
    )
    .await?;

    let current: i64 = match get_meta(conn, "schema_version").await? {
        Some(v) => v
            .parse()
            .context("schema_version in meta is not a number")?,
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

---

### 2.3 `src/db/repo.rs`

Remove all job-related SQL queries (`claim_next_job`, `reset_running_jobs`, `fail_job`, `enqueue_job`). Add a clean `save_chunks()` and `unindexed_notes()` function.

**Replace the contents of `src/db/repo.rs` with:**

```rust
use anyhow::{Context, Result};
use turso::{Connection, Row};

use super::models::{ChunkRow, Note, SearchHit};

// ─── helpers ────────────────────────────────────────────────────────────────

pub fn content_hash(text: &str) -> String {
    blake3::hash(text.as_bytes()).to_hex().to_string()
}

pub(crate) async fn finish_tx<T>(conn: &Connection, result: Result<T>) -> Result<T> {
    match result {
        Ok(value) => {
            conn.execute("COMMIT", ()).await?;
            Ok(value)
        }
        Err(e) => {
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

/// Inserts a note into the database. Returns the new note id.
pub async fn insert_note(conn: &Connection, content: &str) -> Result<i64> {
    let hash = content_hash(content);
    let mut rows = conn
        .query(
            "INSERT INTO notes (content, content_hash) VALUES (?1, ?2) RETURNING id",
            (content, hash.as_str()),
        )
        .await?;

    let id: i64 = rows
        .next()
        .await?
        .context("INSERT ... RETURNING produced no row")?
        .get(0)?;

    Ok(id)
}

/// Updates a note's content. Returns `false` if the content didn't change (no-op).
pub async fn update_note(conn: &Connection, id: i64, content: &str) -> Result<bool> {
    let new_hash = content_hash(content);

    let mut rows = conn
        .query("SELECT content_hash FROM notes WHERE id = ?1", (id,))
        .await?;
    let old_hash: String = rows
        .next()
        .await?
        .with_context(|| format!("note {id} not found"))?
        .get(0)?;

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

    Ok(true)
}

pub async fn get_note(conn: &Connection, id: i64) -> Result<Option<Note>> {
    let sql = format!("SELECT {NOTE_COLUMNS} FROM notes WHERE id = ?1");
    let mut rows = conn.query(&sql, (id,)).await?;
    match rows.next().await? {
        Some(row) => Ok(Some(note_from_row(&row)?)),
        None => Ok(None),
    }
}

/// Most recently updated notes first.
pub async fn recent_notes(conn: &Connection, limit: i64) -> Result<Vec<Note>> {
    let sql = format!("SELECT {NOTE_COLUMNS} FROM notes ORDER BY updated_at DESC, id DESC LIMIT ?1");
    let mut rows = conn.query(&sql, (limit,)).await?;
    let mut notes = Vec::new();
    while let Some(row) = rows.next().await? {
        notes.push(note_from_row(&row)?);
    }
    Ok(notes)
}

/// Chunks and labels are removed automatically by ON DELETE CASCADE.
pub async fn delete_note(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM notes WHERE id = ?1", (id,))
        .await?;
    Ok(())
}

// ─── chunks & search ────────────────────────────────────────────────────────

/// Replaces all chunks for a note within an atomic transaction.
pub async fn save_chunks(conn: &Connection, note_id: i64, chunks: &[ChunkRow]) -> Result<()> {
    conn.execute("BEGIN", ()).await?;
    let result: Result<()> = async {
        conn.execute("DELETE FROM chunks WHERE note_id = ?1", (note_id,))
            .await?;

        for c in chunks {
            let vector_json = serde_json::to_string(&c.embedding)?;
            conn.execute(
                "INSERT INTO chunks (note_id, chunk_index, byte_start, content, embedding)
                 VALUES (?1, ?2, ?3, ?4, vector(?5))",
                (
                    note_id,
                    c.index,
                    c.byte_start,
                    c.content.as_str(),
                    vector_json,
                ),
            )
            .await?;
        }
        Ok(())
    }
    .await;
    finish_tx(conn, result).await
}

/// Returns notes that do not currently have any embedded chunks (used on startup).
pub async fn unindexed_notes(conn: &Connection) -> Result<Vec<(i64, String)>> {
    let sql = "SELECT id, content FROM notes WHERE id NOT IN (SELECT DISTINCT note_id FROM chunks)";
    let mut rows = conn.query(sql, ()).await?;
    let mut results = Vec::new();
    while let Some(row) = rows.next().await? {
        results.push((row.get(0)?, row.get(1)?));
    }
    Ok(results)
}

pub async fn search_chunks(
    conn: &Connection,
    query_vec: &[f32],
    limit: i64,
) -> Result<Vec<SearchHit>> {
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
```

---

### 2.4 `src/db.rs`

This is the main entry point for the database. Instead of spawning an actor loop, `Db` directly wraps `Connection`.

**Replace the contents of `src/db.rs` with:**

```rust
pub mod models;
pub mod repo;
mod schema;

pub use models::*;

use anyhow::{Context, Result};
use std::path::Path;
use turso::{Builder, Connection};

/// Ergonomic database handle. Cheap to clone and share across async tasks.
#[derive(Debug, Clone)]
pub struct Db {
    conn: Connection,
}

impl Db {
    /// Opens (or creates) the database and applies migrations.
    /// Pass `Path::new(":memory:")` for integration tests.
    pub async fn open(path: &Path) -> Result<Self> {
        let path_str = path.to_str().context("database path is not valid UTF-8")?;
        let db = Builder::new_local(path_str)
            .build()
            .await
            .with_context(|| format!("failed to open database at {path_str}"))?;
        let conn = db.connect()?;

        conn.execute("PRAGMA foreign_keys = ON", ()).await?;
        schema::migrate(&conn).await?;
        Ok(Self { conn })
    }

    /// Access the underlying connection if needed.
    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    pub async fn insert_note(&self, content: &str) -> Result<i64> {
        repo::insert_note(&self.conn, content).await
    }

    pub async fn update_note(&self, id: i64, content: &str) -> Result<bool> {
        repo::update_note(&self.conn, id, content).await
    }

    pub async fn get_note(&self, id: i64) -> Result<Option<Note>> {
        repo::get_note(&self.conn, id).await
    }

    pub async fn recent_notes(&self, limit: i64) -> Result<Vec<Note>> {
        repo::recent_notes(&self.conn, limit).await
    }

    pub async fn delete_note(&self, id: i64) -> Result<()> {
        repo::delete_note(&self.conn, id).await
    }

    pub async fn search(&self, query_vec: &[f32], limit: i64) -> Result<Vec<SearchHit>> {
        repo::search_chunks(&self.conn, query_vec, limit).await
    }

    pub async fn save_chunks(&self, note_id: i64, chunks: &[ChunkRow]) -> Result<()> {
        repo::save_chunks(&self.conn, note_id, chunks).await
    }

    pub async fn unindexed_notes(&self) -> Result<Vec<(i64, String)>> {
        repo::unindexed_notes(&self.conn).await
    }
}
```

---

### 2.5 `src/lib.rs`

Make sure `src/lib.rs` exports the modules cleanly:

```rust
pub mod chunk;
pub mod config;
pub mod db;
pub mod embed;
pub mod worker;
---

### 2.6 `src/chunk.rs` (Add `Debug` Implementation)

Because `Message` in `src/main.rs` derives `Debug` and contains `Arc<Chunker>`, `Chunker` must implement `Debug` (just like `Embedder` does in `src/embed.rs`).

Add `impl std::fmt::Debug for Chunker` in `src/chunk.rs`:

```rust
use anyhow::{Result, anyhow};
use std::fmt;
use text_splitter::{ChunkConfig, MarkdownSplitter};
use tokenizers::Tokenizer;

pub struct Chunker {
    splitter: MarkdownSplitter<Tokenizer>,
}

impl fmt::Debug for Chunker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Chunker").finish_non_exhaustive()
    }
}

impl Chunker {
    pub fn new(mut tokenizer: Tokenizer, max_tokens: usize, overlap: usize) -> Result<Self> {
        tokenizer
            .with_truncation(None)
            .map_err(|e| anyhow!("{e}"))?;
        tokenizer.with_padding(None);

        let config = ChunkConfig::new(max_tokens)
            .with_sizer(tokenizer)
            .with_overlap(overlap)?;
        Ok(Self {
            splitter: MarkdownSplitter::new(config),
        })
    }
    pub fn split<'t>(&self, text: &'t str) -> Vec<(usize, &'t str)> {
        self.splitter.chunk_indices(text).collect()
    }
}
```

---

### ✅ Checkpoint 2
Check that the database layer compiles:
```powershell
cargo check --lib
```
It should compile cleanly without any actor dependencies!

---

## Phase 3 — Self-Healing Background Embedding

Now we simplify `src/worker.rs`.

Instead of an endless loop polling a `jobs` table with 30-second timers, we provide:
1. `embed_note`: an async function that splits, embeds, and saves chunks for a note.
2. `sync_unindexed`: called once at startup. It finds any notes that don't have chunks yet and embeds them.

**Replace the contents of `src/worker.rs` with:**

```rust
use anyhow::Result;
use std::sync::Arc;

use crate::chunk::Chunker;
use crate::db::{ChunkRow, Db};
use crate::embed::Embedder;

/// Chunks and embeds a single note, saving the resulting vectors to the database.
/// CPU inference runs in `spawn_blocking` so the UI never stutters.
pub async fn embed_note(
    db: Db,
    embedder: Arc<Embedder>,
    chunker: Arc<Chunker>,
    note_id: i64,
    content: String,
) -> Result<()> {
    let pieces = chunker.split(&content);
    if pieces.is_empty() {
        return Ok(());
    }

    let texts: Vec<String> = pieces.iter().map(|(_, t)| t.to_string()).collect();

    let vectors = tokio::task::spawn_blocking(move || -> Result<Vec<Vec<f32>>> {
        embedder.embed_batch(&texts)
    })
    .await??;

    let rows: Vec<ChunkRow> = pieces
        .into_iter()
        .zip(vectors)
        .enumerate()
        .map(|(i, ((start, text), embedding))| ChunkRow {
            index: i as i64,
            byte_start: start as i64,
            content: text.to_string(),
            embedding,
        })
        .collect();

    db.save_chunks(note_id, &rows).await?;
    Ok(())
}

/// Runs on startup: indexes any notes that were created while offline or missed.
pub async fn index_unindexed_notes(
    db: Db,
    embedder: Arc<Embedder>,
    chunker: Arc<Chunker>,
) -> Result<usize> {
    let unindexed = db.unindexed_notes().await?;
    let count = unindexed.len();

    for (id, content) in unindexed {
        if let Err(e) = embed_note(db.clone(), Arc::clone(&embedder), Arc::clone(&chunker), id, content).await {
            eprintln!("failed to index note {id}: {e:#}");
        }
    }

    Ok(count)
}
```

**What you'll learn:**
Notice how simple this is. There is no job status state machine, no error retry counter, and no crash recovery code. The system is inherently self-healing: on startup, it checks `unindexed_notes()` and catches up automatically.

---

### ✅ Checkpoint 3
Verify the worker compiles:
```powershell
cargo check --lib
```

---

## Phase 4 — UI Revolution: Sidebar, Note List & Semantic Search

Now we build the real application in `src/main.rs`.

### What We Are Adding:
1. **Collapsible / Split View Layout**:
   - **Left Panel (Sidebar)**:
     - Search Input (`"Search notes semantically…"`)
     - Button `+ New Note`
     - Scrollable list of recent notes with title preview and timestamp
     - Or, when searching, a list of search hits with **similarity %** badges!
   - **Right Panel (Editor)**:
     - Header bar with Note ID, Unsaved indicator (`●`), Save button, Delete button.
     - Full text editor with Ctrl+S / Ctrl+N / Ctrl+Q shortcuts.
     - Footer status bar with char count, line count, and model readiness status.

2. **Real In-App Semantic Search**:
   - As you type in the search box, pressing Enter triggers `Message::PerformSearch`.
   - The UI runs the search asynchronously (`Task::perform`), computing the query embedding and retrieving cosine matches from SQLite.
   - Clicking any result instantly opens the note!

**Replace the contents of `src/main.rs` with:**

```rust
use iced::widget::{
    button, column, container, row, scrollable, space, text, text_editor, text_input,
};
use iced::{
    Alignment, Border, Color, Element, Fill, Length, Padding, Size, Task, Theme, keyboard, window,
};
use std::sync::Arc;

use notelook::chunk::Chunker;
use notelook::config;
use notelook::db::{Db, Note, SearchHit};
use notelook::embed::Embedder;
use notelook::worker;

pub fn main() -> iced::Result {
    iced::application(App::boot, App::update, App::view)
        .title("Notelook")
        .theme(theme)
        .window(window::Settings {
            size: Size::new(1050.0, 680.0),
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
    revision: u64,
    saved_revision: u64,
    char_count: usize,
    saving: bool,
    status: String,
    current_note_id: Option<i64>,

    // Data & Models
    db: Option<Db>,
    embedder: Option<Arc<Embedder>>,
    chunker: Option<Arc<Chunker>>,
    model_state: ModelState,

    // Sidebar & Search State
    notes: Vec<Note>,
    search_query: String,
    search_results: Vec<SearchHit>,
    is_searching: bool,
}

impl App {
    fn is_dirty(&self) -> bool {
        self.revision != self.saved_revision
    }

    fn can_save(&self) -> bool {
        self.db.is_some() && !self.saving && self.is_dirty()
    }
}

#[derive(Debug, Clone)]
enum Message {
    DbReady(Result<Db, String>),
    ModelReady(Result<(Arc<Embedder>, Arc<Chunker>), String>),
    NotesLoaded(Result<Vec<Note>, String>),
    SelectNote(i64),
    NoteLoaded(Result<Option<Note>, String>),
    Edit(text_editor::Action),
    NewNote,
    SaveNote,
    NoteSaved(Result<(i64, bool), String>),
    DeleteNote,
    NoteDeleted(Result<(), String>),
    SearchInputChanged(String),
    PerformSearch,
    SearchResultsLoaded(Result<Vec<SearchHit>, String>),
    ClearSearch,
    Quit,
}

// ─── async startup steps ────────────────────────────────────────────────────

async fn open_database() -> anyhow::Result<Db> {
    let path = config::db_path()?;
    Db::open(&path).await
}

async fn load_model() -> anyhow::Result<(Arc<Embedder>, Arc<Chunker>)> {
    let embedder = tokio::task::spawn_blocking(Embedder::try_new).await??;
    let chunker = embedder.chunker(config::CHUNK_MAX_TOKENS, config::CHUNK_OVERLAP)?;
    Ok((Arc::new(embedder), Arc::new(chunker)))
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
            embedder: None,
            chunker: None,
            model_state: ModelState::Loading,
            notes: Vec::new(),
            search_query: String::new(),
            search_results: Vec::new(),
            is_searching: false,
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
                self.status = String::from("Loading notes…");

                // 1. Fetch recent notes list
                let load_notes = Task::perform(
                    {
                        let db = db.clone();
                        async move { db.recent_notes(50).await.map_err(|e| format!("{e:#}")) }
                    },
                    Message::NotesLoaded,
                );

                // 2. Load the embedding model in background
                let load_model_task = Task::perform(
                    async { load_model().await.map_err(|e| format!("{e:#}")) },
                    Message::ModelReady,
                );

                Task::batch([load_notes, load_model_task])
            }
            Message::DbReady(Err(e)) => {
                self.status = format!("Database error: {e}");
                eprintln!("{e}");
                Task::none()
            }

            Message::NotesLoaded(Ok(notes)) => {
                self.notes = notes;
                self.status = String::from("Ready");
                Task::none()
            }
            Message::NotesLoaded(Err(e)) => {
                self.status = format!("Could not load notes: {e}");
                Task::none()
            }

            Message::ModelReady(Ok((embedder, chunker))) => {
                self.embedder = Some(Arc::clone(&embedder));
                self.chunker = Some(Arc::clone(&chunker));
                self.model_state = ModelState::Ready;

                // Sync any unindexed notes in the background
                if let Some(db) = self.db.clone() {
                    tokio::spawn(async move {
                        let _ = worker::index_unindexed_notes(db, embedder, chunker).await;
                    });
                }
                Task::none()
            }
            Message::ModelReady(Err(e)) => {
                self.model_state = ModelState::Failed;
                self.status = format!("Model error: {e}");
                eprintln!("{e}");
                Task::none()
            }

            Message::SelectNote(id) => {
                if self.is_dirty() {
                    self.status = String::from("Unsaved changes: save before switching notes");
                    return Task::none();
                }
                let Some(db) = self.db.clone() else {
                    return Task::none();
                };
                self.status = String::from("Loading note…");
                Task::perform(
                    async move { db.get_note(id).await.map_err(|e| format!("{e:#}")) },
                    Message::NoteLoaded,
                )
            }

            Message::NoteLoaded(Ok(Some(note))) => {
                self.content = text_editor::Content::with_text(&note.content);
                self.current_note_id = Some(note.id);
                self.revision = 0;
                self.saved_revision = 0;
                self.char_count = self.content.text().chars().count();
                self.status = format!("Loaded note {}", note.id);
                Task::none()
            }
            Message::NoteLoaded(Ok(None)) => {
                self.status = String::from("Note not found");
                Task::none()
            }
            Message::NoteLoaded(Err(e)) => {
                self.status = format!("Error loading note: {e}");
                Task::none()
            }

            Message::Edit(action) => {
                let is_edit = action.is_edit();
                self.content.perform(action);
                if is_edit {
                    self.revision += 1;
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
                let text = self.content.text();
                if text.trim().is_empty() {
                    self.status = String::from("Cannot save an empty note");
                    return Task::none();
                }
                let Some(db) = self.db.clone() else {
                    return Task::none();
                };

                self.saving = true;
                self.status = String::from("Saving…");
                let note_id = self.current_note_id;
                let text_clone = text.clone();

                Task::perform(
                    async move {
                        let (id, updated) = match note_id {
                            None => (db.insert_note(&text_clone).await?, true),
                            Some(id) => {
                                let changed = db.update_note(id, &text_clone).await?;
                                (id, changed)
                            }
                        };
                        anyhow::Ok((id, updated))
                    },
                    move |result| Message::NoteSaved(result.map_err(|e| format!("{e:#}"))),
                )
            }

            Message::NoteSaved(Ok((id, changed))) => {
                self.saving = false;
                self.current_note_id = Some(id);
                self.saved_revision = self.revision;
                self.status = if changed {
                    format!("Note {id} saved")
                } else {
                    format!("Note {id}: unchanged")
                };

                // Asynchronously trigger embedding in background if content changed
                if changed {
                    if let (Some(db), Some(emb), Some(chk)) =
                        (self.db.clone(), self.embedder.clone(), self.chunker.clone())
                    {
                        let content = self.content.text();
                        tokio::spawn(async move {
                            let _ = worker::embed_note(db, emb, chk, id, content).await;
                        });
                    }
                }

                // Refresh the sidebar notes list
                if let Some(db) = self.db.clone() {
                    Task::perform(
                        async move { db.recent_notes(50).await.map_err(|e| format!("{e:#}")) },
                        Message::NotesLoaded,
                    )
                } else {
                    Task::none()
                }
            }
            Message::NoteSaved(Err(e)) => {
                self.saving = false;
                self.status = format!("Save failed: {e}");
                Task::none()
            }

            Message::DeleteNote => {
                let Some(id) = self.current_note_id else {
                    return Task::none();
                };
                let Some(db) = self.db.clone() else {
                    return Task::none();
                };

                self.status = format!("Deleting note {id}…");
                Task::perform(
                    async move { db.delete_note(id).await.map_err(|e| format!("{e:#}")) },
                    Message::NoteDeleted,
                )
            }

            Message::NoteDeleted(Ok(())) => {
                self.content = text_editor::Content::new();
                self.current_note_id = None;
                self.revision = 0;
                self.saved_revision = 0;
                self.char_count = 0;
                self.status = String::from("Note deleted");

                // Refresh sidebar
                if let Some(db) = self.db.clone() {
                    Task::perform(
                        async move { db.recent_notes(50).await.map_err(|e| format!("{e:#}")) },
                        Message::NotesLoaded,
                    )
                } else {
                    Task::none()
                }
            }
            Message::NoteDeleted(Err(e)) => {
                self.status = format!("Delete failed: {e}");
                Task::none()
            }

            Message::SearchInputChanged(query) => {
                self.search_query = query;
                if self.search_query.trim().is_empty() {
                    self.is_searching = false;
                    self.search_results.clear();
                }
                Task::none()
            }

            Message::PerformSearch => {
                let query = self.search_query.trim().to_string();
                if query.is_empty() {
                    self.is_searching = false;
                    self.search_results.clear();
                    return Task::none();
                }
                let (Some(db), Some(embedder)) = (self.db.clone(), self.embedder.clone()) else {
                    self.status = String::from("Embedding model not ready yet");
                    return Task::none();
                };

                self.is_searching = true;
                self.status = format!("Searching for \"{query}\"…");

                Task::perform(
                    async move {
                        let query_vec = tokio::task::spawn_blocking({
                            let emb = Arc::clone(&embedder);
                            let q = query.clone();
                            move || emb.compute(&q)
                        })
                        .await??;

                        db.search(&query_vec, 15).await
                    },
                    move |result| {
                        Message::SearchResultsLoaded(result.map_err(|e| format!("{e:#}")))
                    },
                )
            }

            Message::SearchResultsLoaded(Ok(results)) => {
                self.status = format!("Found {} semantic matches", results.len());
                self.search_results = results;
                Task::none()
            }
            Message::SearchResultsLoaded(Err(e)) => {
                self.status = format!("Search error: {e}");
                Task::none()
            }

            Message::ClearSearch => {
                self.search_query.clear();
                self.search_results.clear();
                self.is_searching = false;
                self.status = String::from("Search cleared");
                Task::none()
            }

            Message::Quit => iced::exit(),
        }
    }

    // ─── view ───────────────────────────────────────────────────────────────

    fn view(&self) -> Element<'_, Message> {
        let sidebar = self.view_sidebar();
        let editor = self.view_editor();

        row![sidebar, editor]
            .width(Fill)
            .height(Fill)
            .into()
    }

    fn view_sidebar(&self) -> Element<'_, Message> {
        let title_row = row![
            text("Notes").size(18),
            space::horizontal(),
            button("+ New").on_press(Message::NewNote).padding([4, 10]),
        ]
        .align_y(Alignment::Center);

        let search_box = row![
            text_input("Semantic search…", &self.search_query)
                .on_input(Message::SearchInputChanged)
                .on_submit(Message::PerformSearch)
                .padding(8),
            if self.is_searching {
                button("✕").on_press(Message::ClearSearch).padding([6, 8])
            } else {
                button("🔍").on_press(Message::PerformSearch).padding([6, 8])
            }
        ]
        .spacing(6);

        let list_content: Element<'_, Message> = if self.is_searching {
            if self.search_results.is_empty() {
                container(text("No matches found").size(13))
                    .padding(16)
                    .into()
            } else {
                let items: Vec<Element<'_, Message>> = self
                    .search_results
                    .iter()
                    .map(|hit| {
                        let preview: String = hit.content.chars().take(80).collect();
                        let sim_badge = format!("{:.0}% match", hit.similarity_percent());

                        button(
                            column![
                                row![
                                    text(format!("Note {}", hit.note_id)).size(13),
                                    space::horizontal(),
                                    text(sim_badge).size(12),
                                ],
                                text(preview).size(12),
                            ]
                            .spacing(4),
                        )
                        .on_press(Message::SelectNote(hit.note_id))
                        .width(Fill)
                        .padding(8)
                        .into()
                    })
                    .collect();

                scrollable(column(items).spacing(6)).height(Fill).into()
            }
        } else {
            let items: Vec<Element<'_, Message>> = self
                .notes
                .iter()
                .map(|note| {
                    let is_active = self.current_note_id == Some(note.id);
                    let title = note.title();
                    let date = &note.updated_at;

                    button(
                        column![
                            text(title).size(14),
                            text(date).size(11),
                        ]
                        .spacing(2),
                    )
                    .on_press(Message::SelectNote(note.id))
                    .width(Fill)
                    .padding(8)
                    .style(move |theme, status| {
                        let mut style = button::primary(theme, status);
                        if is_active {
                            style.border = Border {
                                color: Color::from_rgb(0.4, 0.6, 1.0),
                                width: 1.5,
                                radius: 4.0.into(),
                            };
                        }
                        style
                    })
                    .into()
                })
                .collect();

            scrollable(column(items).spacing(4)).height(Fill).into()
        };

        container(
            column![title_row, search_box, list_content]
                .spacing(12)
                .width(Length::Fixed(280.0))
                .height(Fill),
        )
        .padding(12)
        .style(|theme: &Theme| {
            let palette = theme.palette();
            container::Style {
                background: Some(palette.background.into()),
                border: Border {
                    color: Color::from_rgba(1.0, 1.0, 1.0, 0.08),
                    width: 1.0,
                    radius: 0.0.into(),
                },
                ..Default::default()
            }
        })
        .into()
    }

    fn view_editor(&self) -> Element<'_, Message> {
        let dirty_mark = if self.is_dirty() { "● " } else { "" };
        let note_header = match self.current_note_id {
            Some(id) => format!("{dirty_mark}Note #{id}"),
            None => format!("{dirty_mark}New Note (Unsaved)"),
        };

        let action_bar = row![
            text(note_header).size(16),
            space::horizontal(),
            button("Save (Ctrl+S)")
                .on_press_maybe(self.can_save().then_some(Message::SaveNote))
                .padding([6, 12]),
            button("Delete")
                .on_press_maybe(self.current_note_id.map(|_| Message::DeleteNote))
                .padding([6, 12]),
        ]
        .spacing(8)
        .align_y(Alignment::Center);

        let editor = text_editor(&self.content)
            .placeholder("Type your markdown note here…")
            .on_action(Message::Edit)
            .key_binding(editor_bindings)
            .height(Fill);

        let model_label = match self.model_state {
            ModelState::Loading => "Model loading…",
            ModelState::Ready => "AI Search Ready",
            ModelState::Failed => "AI Search Unavailable",
        };

        let status_bar = row![
            text(&self.status).size(12),
            space::horizontal(),
            text(format!(
                "Lines: {} | Chars: {} | {}",
                self.content.line_count(),
                self.char_count,
                model_label
            ))
            .size(12),
        ]
        .align_y(Alignment::Center);

        column![action_bar, editor, status_bar]
            .spacing(12)
            .padding(Padding::new(16.0))
            .width(Fill)
            .height(Fill)
            .into()
    }
}

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

---

### ✅ Checkpoint 4
Compile and run the GUI application:
```powershell
cargo check
cargo run
```
You now have:
- A real desktop notepad with a two-pane layout!
- A live recent notes list on the left with click-to-load.
- An in-app semantic search bar with similarity percentages.
- Clean Ctrl+S, Ctrl+N, Ctrl+Q shortcuts and note deletion!

---

## Phase 5 — CLI & Integration Tests

Now let's update the CLI and integration tests to use the new `Db` struct directly.

### 5.1 `src/bin/cli.rs`

**Replace the contents of `src/bin/cli.rs` with:**

```rust
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
```

---

### 5.2 `tests/db.rs`

Replace the cynical `// do you trust these tests???` with clean, reliable tests that test real note insertion, updates, cascading deletes, and chunk storage.

**Replace the contents of `tests/db.rs` with:**

```rust
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
    let id = db.insert_note("Meeting notes: project alpha launch").await?;

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
    let hits = db.search(&vec![0.1; notelook::config::EMBED_DIM], 5).await?;
    assert_eq!(hits.len(), 2);

    // Delete the note
    db.delete_note(id).await?;

    // Chunks are gone due to ON DELETE CASCADE
    let hits_after = db.search(&vec![0.1; notelook::config::EMBED_DIM], 5).await?;
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
```

---

## 7. Master Verification & Next Steps

Once you have applied these changes by hand:

1. **Verify all tests pass:**
   ```powershell
   cargo test
   ```
   All tests in `tests/db.rs` should complete in milliseconds and report `test result: ok`.

2. **Verify CLI works:**
   ```powershell
   cargo run --bin cli -- list
   ```

3. **Run the Notelook GUI:**
   ```powershell
   cargo run
   ```
   - Click `+ New` to write a note.
   - Type some markdown and press `Ctrl+S`. Notice the note immediately appears in the left sidebar!
   - Write another note about a different topic and save it.
   - Click between notes in the sidebar to switch effortlessly.
   - Type a semantic query in the search box (e.g., `"launch plans"` or `"meeting"`) and press Enter to see ranked semantic search results with percentage scores!

Enjoy your lightweight, powerful, and truly native Rust desktop notepad!
