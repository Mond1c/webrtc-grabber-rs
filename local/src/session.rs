use crate::broadcaster::TrackBroadcaster;
use dashmap::DashMap;
use std::sync::Arc;
use webrtc::peer_connection::RTCPeerConnection;

pub struct PublisherSession {
    pub pc: Arc<RTCPeerConnection>,
    pub broadcasters: Arc<DashMap<String, Arc<TrackBroadcaster>>>,
}

impl PublisherSession {
    pub fn new(pc: Arc<RTCPeerConnection>) -> Self {
        Self {
            pc,
            broadcasters: Arc::new(DashMap::new()),
        }
    }

    pub fn get_broadcaster(&self, track_id: &str) -> Option<Arc<TrackBroadcaster>> {
        self.broadcasters
            .get(track_id)
            .map(|b| Arc::clone(b.value()))
    }

    pub fn add_broadcaster(&self, track_id: String, broadcaster: Arc<TrackBroadcaster>) {
        self.broadcasters.insert(track_id, broadcaster);
    }

    pub fn get_all_broadcasters(&self) -> Vec<(String, Arc<TrackBroadcaster>)> {
        self.broadcasters
            .iter()
            .map(|entry| (entry.key().clone(), Arc::clone(entry.value())))
            .collect()
    }
}

impl Drop for PublisherSession {
    fn drop(&mut self) {
        let pc = Arc::clone(&self.pc);
        tokio::spawn(async move {
            if let Err(e) = pc.close().await {
                tracing::warn!("Error closing publisher peer connection: {:?}", e);
            }
        });
    }
}

pub struct SubscriberSession {
    pub pc: Arc<RTCPeerConnection>,
    pub publisher_id: String,
    pub track_mapping: Vec<(String, String)>,
}

impl SubscriberSession {
    pub fn new(
        pc: Arc<RTCPeerConnection>,
        publisher_id: String,
        track_mapping: Vec<(String, String)>,
    ) -> Self {
        Self {
            pc,
            publisher_id,
            track_mapping,
        }
    }
}

impl Drop for SubscriberSession {
    fn drop(&mut self) {
        let pc = Arc::clone(&self.pc);
        tokio::spawn(async move {
            if let Err(e) = pc.close().await {
                tracing::warn!("Error closing subscriber peer connection: {:?}", e);
            }
        });
    }
}
