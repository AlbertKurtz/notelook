use anyhow::{Context, Result, anyhow, ensure};
use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config};
use std::fmt;
use std::sync::Arc;
use tokenizers::{PaddingParams, Tokenizer, TruncationParams};

use crate::chunk::Chunker;
use crate::config::{EMBED_DIM, EMBED_MAX_TOKENS, EMBED_MODEL_ID};

#[derive(Clone)]
pub struct Embedder {
    model: Arc<BertModel>,
    tokenizer: Arc<Tokenizer>,
    device: Device,
}
impl fmt::Debug for Embedder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Embedder")
            .field("model", &EMBED_MODEL_ID)
            .finish_non_exhaustive()
    }
}

const CONFIG_BYTES: &[u8] = include_bytes!("../assets/model/config.json");
const TOKENIZER_BYTES: &[u8] = include_bytes!("../assets/model/tokenizer.json");
const WEIGHTS_BYTES: &[u8] = include_bytes!("../assets/model/model.safetensors");

impl Embedder {
    pub fn try_new() -> Result<Self> {
        let device = Device::Cpu;

        let config: Config = serde_json::from_slice(CONFIG_BYTES)?;

        let mut tokenizer =
            Tokenizer::from_bytes(TOKENIZER_BYTES).map_err(|e| anyhow!("{e}"))?;
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

        let vb = VarBuilder::from_slice_safetensors(WEIGHTS_BYTES, DType::F32, &device)?;
        let model = BertModel::load(vb, &config)?;

        let embedder = Self {
            model: Arc::new(model),
            tokenizer: Arc::new(tokenizer),
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

    pub fn chunker(&self, max_tokens: usize, overlap: usize) -> Result<Chunker> {
        Chunker::new((*self.tokenizer).clone(), max_tokens, overlap)
    }

    pub fn compute(&self, text: &str) -> Result<Vec<f32>> {
        self.embed_batch(&[text])?
            .pop()
            .context("embedding batch was empty")
    }

    pub fn embed_batch<S: AsRef<str>>(&self, sentence: &[S]) -> Result<Vec<Vec<f32>>> {
        if sentence.is_empty() {
            return Ok(Vec::new());
        }

        let sentence_strs: Vec<&str> = sentence.iter().map(|s| s.as_ref()).collect();
        let encodings = self
            .tokenizer
            .encode_batch(sentence_strs, true)
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
        let attention_mask = to_tensor(&|e| e.get_attention_mask())?.to_dtype(DType::F32)?; //                               [b, s]

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
    pub fn tokenizer(&self) -> tokenizers::Tokenizer {
        (*self.tokenizer).clone()
    }
}

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
