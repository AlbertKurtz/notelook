use anyhow::{Context, Result, anyhow};
use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config};
use hf_hub::api::sync::Api;
use std::sync::Arc;
use tokenizers::{PaddingParams, Tokenizer};

#[derive(Clone)]
pub struct Embedder {
    model: Arc<BertModel>,
    tokenizer: Arc<Tokenizer>,
    device: Device,
}

impl Embedder {
    pub fn try_new() -> Result<Self> {
        let device = Device::Cpu;
        let api = Api::new().context("Failed to initialize Hugging Face Api client")?;
        let repo = api.model("sentence-transformers/all-MiniLM-L6-v2".to_string());

        let config_path = repo
            .get("config.json")
            .context("Failed to get config.json")?;
        let tokenizer_path = repo
            .get("tokenizer.json")
            .context("Failed to get tokenizer.json")?;
        let weights_path = repo
            .get("model.safetensors")
            .context("Failed to get model.safetensors")?;

        let config_str = std::fs::read_to_string(config_path)?;
        let config: Config = serde_json::from_str(&config_str)?;

        let mut tokenizer = Tokenizer::from_file(tokenizer_path).map_err(|e| anyhow!("{e}"))?;

        tokenizer.with_padding(Some(PaddingParams {
            strategy: tokenizers::PaddingStrategy::BatchLongest,
            ..Default::default()
        }));

        let vb =
            unsafe { VarBuilder::from_mmaped_safetensors(&[weights_path], DType::F32, &device)? };
        let model = BertModel::load(vb, &config)?;

        Ok(Self {
            model: Arc::new(model),
            tokenizer: Arc::new(tokenizer),
            device,
        })
    }

    pub fn compute(&self, text: &str) -> Result<Vec<f32>> {
        let mut results = self.embed_batch(&[text])?;
        results
            .pop()
            .context("Failed to extract embedding from batch")
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

        // 1. Correct: Token IDs for vocabulary indexing
        let token_ids: Vec<Tensor> = encodings
            .iter()
            .map(|e| Tensor::new(e.get_ids(), &self.device))
            .collect::<candle_core::Result<Vec<_>>>()?;

        // 2. Token type IDs for segment distinction
        let token_type_ids: Vec<Tensor> = encodings
            .iter()
            .map(|e| Tensor::new(e.get_type_ids(), &self.device))
            .collect::<candle_core::Result<Vec<_>>>()?;

        let input_ids = Tensor::stack(&token_ids, 0)?;
        let token_type_ids = Tensor::stack(&token_type_ids, 0)?;

        // Forward pass
        let ys = self.model.forward(&input_ids, &token_type_ids, None)?;

        // Attention mask: [batch, seq_len, 1] as F32
        let attention_mask: Vec<Tensor> = encodings
            .iter()
            .map(|e| Tensor::new(e.get_attention_mask(), &self.device))
            .collect::<candle_core::Result<Vec<_>>>()?;

        let attention_mask = Tensor::stack(&attention_mask, 0)?
            .to_dtype(DType::F32)?
            .unsqueeze(2)?;

        let masked_embeddings = ys.broadcast_mul(&attention_mask)?;

        // Mean Pooling
        let sum_embeddings = masked_embeddings.sum(1)?;
        let sum_mask = attention_mask.sum(1)?.clamp(1e-9, f64::MAX)?;
        let pooled = sum_embeddings.broadcast_div(&sum_mask)?;

        // L2 Normalization (dot product == cosine similarity)
        let norm = pooled.sqr()?.sum_keepdim(1)?.sqrt()?;
        let normalized = pooled.broadcast_div(&norm)?;

        // Direct conversion of 2D tensor into Vec<Vec<f32>>
        let results = normalized.to_vec2::<f32>()?;
        Ok(results)
    }
    pub fn tokenizer(&self) -> tokenizers::Tokenizer {
        (*self.tokenizer).clone()
    }
}
