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
