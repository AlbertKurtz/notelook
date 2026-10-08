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

    if std::env::var("NOTELOOK_SKIP_MODEL_DOWNLOAD")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
    {
        if !weights_exist {
            println!("cargo:warning=NOTELOOK_SKIP_MODEL_DOWNLOAD is set, but assets/model/ is missing weights!");
        }
        return Ok(());
    }

    if !weights_exist || current_marker.trim() != model_id.trim() {
        println!("cargo:warning=Syncing model '{model_id}' from Hugging Face into assets/model/...");

        fetch_file(&model_id, "config.json", &model_dir.join("config.json"))?;
        fetch_file(&model_id, "tokenizer.json", &model_dir.join("tokenizer.json"))?;
        fetch_file(&model_id, "model.safetensors", &model_dir.join("model.safetensors"))?;

        fs::write(&marker_path, &model_id)?;
        println!("cargo:warning=Successfully bundled model '{model_id}' into assets/model/");
    }

    Ok(())
}

fn fetch_file(model_id: &str, filename: &str, dest: &Path) -> Result<()> {
    // 1. Try hf-hub first
    if let Ok(api) = Api::new() {
        let repo = api.model(model_id.to_string());
        if let Ok(src) = repo.get(filename) {
            fs::copy(src, dest)?;
            return Ok(());
        }
    }

    // 2. Fallback to system curl (handles corporate proxies, VPNs, and --ssl-no-revoke)
    println!("cargo:warning=hf-hub TLS failed for {filename}; falling back to system curl...");
    let url = format!("https://huggingface.co/{model_id}/resolve/main/{filename}");

    #[cfg(windows)]
    let mut cmd = std::process::Command::new("curl.exe");
    #[cfg(not(windows))]
    let mut cmd = std::process::Command::new("curl");

    cmd.args(["-f", "-L"]);
    #[cfg(windows)]
    cmd.arg("--ssl-no-revoke");
    cmd.arg("-o");
    cmd.arg(dest);
    cmd.arg(&url);

    let status = cmd.status().with_context(|| format!("failed to run curl for {url}"))?;
    if !status.success() {
        // Retry with -k if corporate SSL interception certificate is unknown
        let mut retry_cmd = std::process::Command::new(if cfg!(windows) { "curl.exe" } else { "curl" });
        retry_cmd.args(["-f", "-L", "-k", "-o"]);
        retry_cmd.arg(dest);
        retry_cmd.arg(&url);
        let retry_status = retry_cmd.status().with_context(|| format!("curl fallback failed for {url}"))?;
        anyhow::ensure!(
            retry_status.success(),
            "failed to download {filename} from {url} via both hf-hub and curl"
        );
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