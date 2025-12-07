use std::sync::Arc;

use sfu_core::Sfu;
use sfu_local::config::SfuConfig;

use crate::{protocol, storage::Storage};

pub struct AppState {
    pub sfu: Box<dyn Sfu + Send + Sync>,
    pub storage: Storage,
    pub config: Arc<SfuConfig>,
}

impl AppState {
    pub fn new(sfu: Box<dyn Sfu + Send + Sync>, config: SfuConfig) -> Self {
        Self {
            sfu,
            storage: Storage::new(),
            config: Arc::new(config),
        }
    }

    pub fn get_client_rtc_config(&self) -> protocol::JsonRtcConfiguration {
        let ice_servers = self
            .config
            .ice_servers
            .iter()
            .map(|url| protocol::JsonIceServer {
                urls: vec![url.clone()],
                username: None,
                credential: None,
            })
            .collect();

        protocol::JsonRtcConfiguration { ice_servers }
    }
}
