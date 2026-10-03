use fastembed::{EmbeddingModel, TextEmbedding, TextInitOptions};
use std::sync::{Arc, Mutex}; // 1. Import Mutex

#[derive(Clone)]
// 2. Put Mutex inside Arc here:
pub struct Embedder(Arc<Mutex<TextEmbedding>>);

impl Embedder {
    pub fn try_new() -> Result<Self, Box<dyn std::error::Error>> {
        let options =
            TextInitOptions::new(EmbeddingModel::AllMiniLML6V2).with_show_download_progress(false);
        let model = TextEmbedding::try_new(options)?;

        // 3. Wrap model in Mutex::new(...) before Arc::new(...)
        Ok(Self(Arc::new(Mutex::new(model))))
    }

    pub fn compute(&self, text: &str) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
        // Now self.0 is Arc<Mutex<...>>, so .lock() is available:
        let mut model = self.0.lock().map_err(|e| format!("Mutex poisoned: {e}"))?;

        let mut results = model.embed(vec![text.to_string()], None)?;
        results
            .pop()
            .ok_or_else(|| "Failed to generate embedding".into())
    }
}
