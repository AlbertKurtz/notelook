// build.rs
use anyhow::{Context, Result};
use hf_hub::api::sync::Api;
use std::fs;
use std::path::Path;

fn main() -> Result<()> {
    println!("cargo:rerun-if-changed=src/config.rs");
    println!("cargo:rerun-if-changed=assets/model/model_id.txt");
    println!("cargo:rerun-if-changed=assets/model/config.json");
    println!("cargo:rerun-if-changed=assets/model/tokenizer.json");
    println!("cargo:rerun-if-changed=assets/model/model.safetensors");

    let config_content = fs::read_to_string("src/config.rs")
        .context("failed to read src/config.rs")?;
    let model_id = extract_model_id(&config_content)
        .unwrap_or_else(|| "sentence-transformers/all-MiniLM-L6-v2".to_string());

    let model_dir = Path::new("assets/model");
    fs::create_dir_all(model_dir)?;
    let marker_path = model_dir.join("model_id.txt");
    let current_marker = fs::read_to_string(&marker_path).unwrap_or_default();

    let weights_exist = model_dir.join("config.json").exists()
        && model_dir.join("tokenizer.json").exists()
        && model_dir.join("model.safetensors").exists();

    if !weights_exist || current_marker.trim() != model_id.trim() {
        println!("cargo:warning=Syncing model '{model_id}' from Hugging Face into assets/model/...");
        let api = Api::new().context("failed to initialize hf-hub Api client")?;
        let repo = api.model(model_id.clone());

        let cfg = repo.get("config.json").context("failed to fetch config.json")?;
        let tok = repo.get("tokenizer.json").context("failed to fetch tokenizer.json")?;
        let wgt = repo.get("model.safetensors").context("failed to fetch model.safetensors")?;

        fs::copy(cfg, model_dir.join("config.json"))?;
        fs::copy(tok, model_dir.join("tokenizer.json"))?;
        fs::copy(wgt, model_dir.join("model.safetensors"))?;
        fs::write(&marker_path, &model_id)?;
        println!("cargo:warning=Successfully bundled model '{model_id}' into assets/model/");
    }

    Ok(())
}

fn extract_model_id(content: &str) -> Option<String> {
    for line in content.lines() {
        if line.contains("EMBED_MODEL_ID") && line.contains('=') {
            if let Some(start) = line.find('"') {
                if let Some(end) = line[start + 1..].find('"') {
                    return Some(line[start + 1..start + 1 + end].to_string());
                }
            }
        }
    }
    None
}