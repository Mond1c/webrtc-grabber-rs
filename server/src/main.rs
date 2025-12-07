use std::sync::Arc;
use anyhow::Result;
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

use sfu_core::Sfu;
use sfu_local::{LocalSfu, SfuConfig};
use webrtc_grabber_rs_server::{AppState, start_server};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,webrtc_grabber_rs_server=debug,sfu_local=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    info!("Starting WebRTC SFU Server");

    let config = SfuConfig::load("config.yaml").unwrap_or_else(|_| {
        info!("Using default configuration");
        create_default_config()
    });

    let bind_addr = config.server.bind_address.clone();

    let sfu = LocalSfu::new("local-sfu-1".to_string(), config.clone())?;
    info!("SFU instance created with ID: {}", sfu.id());

    let state = Arc::new(AppState::new(Box::new(sfu), config));

    start_server(&bind_addr, state).await?;

    Ok(())
}

fn create_default_config() -> SfuConfig {
    use sfu_local::config::{CodecItem, CodecsConfig, PerformanceConfig, ServerConfig};

    SfuConfig {
        server: ServerConfig {
            bind_address: "0.0.0.0:8080".to_string(),
            enable_metrics: true,
        },
        ice_servers: vec![
            "stun:stun.l.google.com:19302".to_string(),
        ],
        codecs: CodecsConfig {
            audio: vec![
                CodecItem {
                    mime: "audio/opus".to_string(),
                    payload_type: 111,
                    clock_rate: 48000,
                    channels: Some(2),
                    sdp_fmtp: Some("minptime=10;useinbandfec=1".to_string()),
                },
            ],
            video: vec![
                CodecItem {
                    mime: "video/VP8".to_string(),
                    payload_type: 96,
                    clock_rate: 90000,
                    channels: None,
                    sdp_fmtp: None,
                },
                CodecItem {
                    mime: "video/H264".to_string(),
                    payload_type: 102,
                    clock_rate: 90000,
                    channels: None,
                    sdp_fmtp: Some("level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e01f".to_string()),
                },
            ],
        },
        performance: PerformanceConfig {
            broadcast_channel_capacity: 1000,
            max_publishers: 100,
            max_subscribers_per_publisher: 50,
        },
    }
}
