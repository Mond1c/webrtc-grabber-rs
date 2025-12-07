use anyhow::{Context, Result};
use serde::Deserialize;
use std::fs;

#[derive(Debug, Deserialize, Clone)]
pub struct SfuConfig {
    pub server: ServerConfig,
    pub ice_servers: Vec<String>,
    pub codecs: CodecsConfig,
    #[serde(default = "default_performance")]
    pub performance: PerformanceConfig,
}

fn default_performance() -> PerformanceConfig {
    PerformanceConfig::default()
}

#[derive(Debug, Deserialize, Clone)]
pub struct PerformanceConfig {
    #[serde(default = "default_broadcast_capacity")]
    pub broadcast_channel_capacity: usize,

    #[serde(default = "default_max_publishers")]
    pub max_publishers: usize,

    #[serde(default = "default_max_subscribers_per_publisher")]
    pub max_subscribers_per_publisher: usize,
}

fn default_broadcast_capacity() -> usize {
    1000
}
fn default_max_publishers() -> usize {
    1000
}
fn default_max_subscribers_per_publisher() -> usize {
    100
}

impl Default for PerformanceConfig {
    fn default() -> Self {
        Self {
            broadcast_channel_capacity: default_broadcast_capacity(),
            max_publishers: default_max_publishers(),
            max_subscribers_per_publisher: default_max_subscribers_per_publisher(),
        }
    }
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
        let config: SfuConfig =
            serde_yaml::from_str(&content).context("Failed to parse YAML config")?;
        Ok(config)
    }

    pub fn validate_credentials(&self, _creds: &str) -> bool {
        true // Placeholder
    }
}
