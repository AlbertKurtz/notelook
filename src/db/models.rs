/// A note as stored in the `notes` table.
#[derive(Debug, Clone)]
pub struct Note {
    pub id: i64,
    pub created_at: String,
    pub updated_at: String,
    pub event_at: Option<String>,
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
    /// Real cosine distance: 0.0 = identical, 1.0 = orthogonal / unrelated.
    pub distance: f64,
    /// Whether an exact keyword match was found in this chunk.
    pub lexical_matched: bool,
    /// Internal RRF ranking fusion score used for sorting order.
    pub rrf_score: f64,
}

impl SearchHit {
    /// Similarity percentage (100% is identical).
    /// Calibrated similarity percentage with baseline noise cutoff.
    /// If we do not subtract floor uncorrelated results will score 40% anyway in hybrid
    pub fn similarity_percent(&self) -> f64 {
        let raw_sim = (1.0 - self.distance).clamp(0.0, 1.0); // <- this is standard

        // If there's an exact lexical match baseline boosted
        if self.lexical_matched {
            let boosted = (raw_sim * 0.7 + 0.3) * 100.0;
            return boosted.clamp(0.0, 100.0);
        }

        // suppress random noise (< 0.25) with magic numers :-(
        // preserving decent hits (0.30 - 0.45) at realistic scores (15% - 35%)
        let score = (raw_sim.powf(1.6) * 100.0).clamp(0.0, 100.0);
        score
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
