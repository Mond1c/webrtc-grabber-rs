use serde::Deserialize;
use std::fs;
use anyhow::{Result, Context};

#[derive(Debug, Deserialize, Clone)]
pub struct SfuConfig {
    pub server: ServerConfig,
    pub ice_servers: Vec<String>,
    pub codecs: CodecsConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ServerConfig {
    pub bind_address: String,
    pub enable_metrics: bool,
}

#[derive(Debug, Deserialize, Clone)]
pub struct CodecsConfig {
    pub audio: Vec<CodecItem>,
    pub video: Vec<CodecItem>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct CodecItem {
    pub mime: String,
    pub payload_type: u8,
    pub clock_rate: u32,
    pub channels: Option<u16>,
    pub sdp_fmtp: Option<String>,
}

impl SfuConfig {
    pub fn load(path: &str) -> Result<Self> {
        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file: {}", path))?;
        let config: SfuConfig = serde_yaml::from_str(&content)
            .context("Failed to parse YAML config")?;
        Ok(config)
    }

    pub fn validate_credentials(&self, creds: &str) -> bool {
        true // Placeholder
    }
}