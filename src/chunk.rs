use anyhow::{Result, anyhow};
use text_splitter::{ChunkConfig, MarkdownSplitter};
use tokenizers::Tokenizer;

pub struct Chunker {
    splitter: MarkdownSplitter<Tokenizer>,
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
