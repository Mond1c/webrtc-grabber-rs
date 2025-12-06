use std::{collections::HashMap, sync::Arc};
use tokio::{sync::{Mutex, broadcast}, task::JoinHandle};
use webrtc::{rtp::packet::Packet, track::track_local::{TrackLocal, track_local_static_rtp::TrackLocalStaticRTP}};
use webrtc::track::track_remote::TrackRemote;
use webrtc::track::track_local::TrackLocalWriter;
use tracing::{error, trace, warn};

const BROADCAST_CHANNEL_CAPACITY: usize = 1000;

pub struct TrackBroadcaster {
    pub id: String,
    pub kind: String,
    tx: broadcast::Sender<Arc<Packet>>,
    #[allow(dead_code)]
    read_task: JoinHandle<()>,
    subscribers: Arc<Mutex<HashMap<String, JoinHandle<()>>>>,
}

impl TrackBroadcaster {
    pub fn new(source_track: Arc<TrackRemote>) -> Self {
        let id = source_track.id().to_string();
        let kind = source_track.kind().to_string();

        let (tx, _) = broadcast::channel(BROADCAST_CHANNEL_CAPACITY);
        let tx_clone = tx.clone();

        let source_id = id.clone();

        let read_task = tokio::spawn(async move {
            loop {
                match source_track.read_rtp().await {
                    Ok((pkt, _)) => {
                        let _ = tx_clone.send(Arc::new(pkt));
                    }
                    Err(webrtc::Error::ErrClosedPipe) | Err(webrtc::Error::ErrConnectionClosed) => {
                        trace!("Source track {} closed", source_id);
                        break;
                    }
                    Err(e) => {
                        error!("Error reading from track {}: {}", source_id, e);
                        break;
                    }
                }
            }
        });

        Self {
            id,
            kind,
            tx,
            read_task,
            subscribers: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn add_subscriber(
        &self,
        track: Arc<TrackLocalStaticRTP>,
    ) {
        let mut rx = self.tx.subscribe();
        let track_id = track.id().to_string();
        let map_key = track_id.clone();

        let join_handle = tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(pkt) => {
                        if let Err(e) = track.write_rtp(&pkt).await {
                            if e == webrtc::Error::ErrClosedPipe || e == webrtc::Error::ErrConnectionClosed {
                                trace!("Subscriber {} disconnected gracefully", track_id);
                            } else {
                                warn!("Error writing to subscriber {}: {}", track_id, e);
                            }
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        warn!("Subscriber {} lagging, dropped {} packets", track_id, skipped);
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        break;
                    }
                }
            }
        });

        let mut subs = self.subscribers.lock().await;
        subs.insert(map_key, join_handle);
    }

    pub async fn remove_subscriber(&self, track_id: &str) {
        let mut subs = self.subscribers.lock().await;
        if let Some(handle) = subs.remove(track_id) {
            handle.abort();
            trace!("Removed subscriber {} from broadcaster {}", track_id, self.id);
        }
    }
}