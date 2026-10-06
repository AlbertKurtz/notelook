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
    /// Cosine distance: 0.0 = identical, 1.0 = opposite.
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
