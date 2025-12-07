use dashmap::DashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, RwLock};
use tokio::{sync::broadcast, task::JoinHandle};
use tracing::{error, info, trace, warn};
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::track::track_local::TrackLocalWriter;
use webrtc::track::track_remote::TrackRemote;
use webrtc::{
    rtp::packet::Packet,
    track::track_local::{track_local_static_rtp::TrackLocalStaticRTP, TrackLocal},
};

pub struct TrackBroadcaster {
    pub id: String,
    pub kind: String,
    pub mime_type: String,
    pub ssrc: u32,
    tx: broadcast::Sender<Arc<Packet>>,
    read_task: JoinHandle<()>,
    subscribers: Arc<DashMap<String, JoinHandle<()>>>,
    peer_connection: Arc<RTCPeerConnection>,
    last_pli_time: Arc<RwLock<Option<Instant>>>,
    pli_request_tx: mpsc::UnboundedSender<()>,
    pli_task: JoinHandle<()>,
}

impl TrackBroadcaster {
    pub fn new(
        source_track: Arc<TrackRemote>,
        peer_connection: Arc<RTCPeerConnection>,
        mime_type: String,
        channel_capacity: usize,
    ) -> Self {
        let id = source_track.id().to_string();
        let kind = source_track.kind().to_string();
        let ssrc = source_track.ssrc();

        let (tx, _) = broadcast::channel(channel_capacity);
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

        let (pli_request_tx, mut pli_request_rx) = mpsc::unbounded_channel::<()>();
        let pc_for_pli = Arc::clone(&peer_connection);
        let pli_track_id = id.clone();
        let pli_kind = kind.clone();
        let last_pli_time = Arc::new(RwLock::new(None::<Instant>));
        let last_pli_clone = Arc::clone(&last_pli_time);

        let pli_task = tokio::spawn(async move {
            while pli_request_rx.recv().await.is_some() {
                if pli_kind != "video" {
                    continue;
                }

                let now = Instant::now();
                {
                    let last_time = last_pli_clone.read().await;
                    if let Some(last) = *last_time {
                        if now.duration_since(last) < Duration::from_millis(500) {
                            trace!("PLI request throttled for track {}", pli_track_id);
                            continue;
                        }
                    }
                }

                *last_pli_clone.write().await = Some(now);

                use webrtc::rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication;

                let pli = PictureLossIndication {
                    sender_ssrc: 0,
                    media_ssrc: ssrc,
                };

                if let Err(e) = pc_for_pli.write_rtcp(&[Box::new(pli)]).await {
                    warn!("Failed to send PLI for track {}: {}", pli_track_id, e);
                } else {
                    info!("Sent PLI for track {} (SSRC: {})", pli_track_id, ssrc);
                }
            }
        });

        Self {
            id,
            kind,
            mime_type,
            ssrc,
            tx,
            read_task,
            subscribers: Arc::new(DashMap::new()),
            peer_connection,
            last_pli_time,
            pli_request_tx,
            pli_task,
        }
    }

    pub fn request_keyframe(&self) {
        let _ = self.pli_request_tx.send(());
    }

    fn request_keyframe_with_retries(&self) {
        if self.kind != "video" {
            return;
        }

        let pli_tx = self.pli_request_tx.clone();

        tokio::spawn(async move {
            for i in 0..3 {
                let _ = pli_tx.send(());
                info!("Sent PLI request #{} for new subscriber", i + 1);

                if i < 2 {
                    tokio::time::sleep(Duration::from_millis(300)).await;
                }
            }
        });
    }

    pub fn subscriber_count(&self) -> usize {
        self.subscribers.len()
    }

    pub async fn add_subscriber(&self, track: Arc<TrackLocalStaticRTP>) {
        let mut rx = self.tx.subscribe();
        let track_id = track.id().to_string();
        let map_key = track_id.clone();
        let pli_tx = self.pli_request_tx.clone();

        let join_handle = tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(pkt) => {
                        if let Err(e) = track.write_rtp(&pkt).await {
                            if e == webrtc::Error::ErrClosedPipe
                                || e == webrtc::Error::ErrConnectionClosed
                            {
                                trace!("Subscriber {} disconnected gracefully", track_id);
                            } else {
                                warn!("Error writing to subscriber {}: {}", track_id, e);
                            }
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        warn!(
                            "Subscriber {} lagging, dropped {} packets - requesting keyframe",
                            track_id, skipped
                        );

                        if skipped > 10 {
                            let _ = pli_tx.send(());
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        break;
                    }
                }
            }
        });

        self.subscribers.insert(map_key, join_handle);

        self.request_keyframe_with_retries();
    }

    pub async fn remove_subscriber(&self, track_id: &str) {
        if let Some((_, handle)) = self.subscribers.remove(track_id) {
            handle.abort();
            trace!(
                "Removed subscriber {} from broadcaster {}",
                track_id,
                self.id
            );
        }
    }
}

impl Drop for TrackBroadcaster {
    fn drop(&mut self) {
        self.read_task.abort();
        self.pli_task.abort();

        for entry in self.subscribers.iter() {
            entry.value().abort();
        }
    }
}
