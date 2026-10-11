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
    
            // Baseline noise floor for paraphrase-multilingual-MiniLM-L12-v2
            // the 0.4 is a bit of a magic number, should be tuned or extracted as config
            let floor = if self.lexical_matched { 0.25 } else { 0.40 };

            // clamp
            if raw_sim <= floor {
                return 0.0;
            }
    
            // Rescale [floor, 1.0] -> [0%, 100%]
            let scaled = (raw_sim - floor) / (1.0 - floor) * 100.0;
    
            // Give a confidence boost if both lexical and semantic match
            let boosted = if self.lexical_matched && raw_sim > 0.45 {
                scaled * 1.15
            } else {
                scaled
            };
    
            boosted.clamp(0.0, 100.0)
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
