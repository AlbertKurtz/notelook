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
