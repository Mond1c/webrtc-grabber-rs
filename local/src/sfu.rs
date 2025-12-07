use anyhow::{Context, Result};
use dashmap::DashMap;
use sfu_core::{
    PublisherRequest, PublisherResponse, PublisherUpdateRequest, PublisherUpdateResponse, Sfu,
    SubscriberRequest, SubscriberResponse, SubscriberUpdateRequest, SubscriberUpdateResponse,
};
use sfu_proto::SfuMetrics;
use std::sync::Arc;
use tracing::{info, warn};
use webrtc::{
    api::{
        interceptor_registry::register_default_interceptors, media_engine::MediaEngine, APIBuilder,
        API,
    },
    ice_transport::{ice_candidate::RTCIceCandidateInit, ice_server::RTCIceServer},
    interceptor::registry::Registry,
    peer_connection::{
        configuration::RTCConfiguration, peer_connection_state::RTCPeerConnectionState,
        RTCPeerConnection,
    },
    rtp_transceiver::rtp_codec::{RTCRtpCodecCapability, RTCRtpCodecParameters, RTPCodecType},
    track::track_local::{track_local_static_rtp::TrackLocalStaticRTP, TrackLocal},
};

use crate::error::{Result as SfuResult, SfuError};
use crate::{
    broadcaster::TrackBroadcaster,
    config::SfuConfig,
    session::{PublisherSession, SubscriberSession},
};

pub struct LocalSfu {
    id: String,
    api: Arc<API>,
    config: SfuConfig,
    publishers: DashMap<String, Arc<PublisherSession>>,
    subscribers: DashMap<String, Arc<SubscriberSession>>,
    metrics: Arc<DashMap<String, usize>>,
}

impl LocalSfu {
    pub fn new(id: String, config: SfuConfig) -> SfuResult<Self> {
        let mut media_engine = MediaEngine::default();
        let _ = media_engine.register_default_codecs();

        Self::register_codecs_from_config(&mut media_engine, &config)?;

        let mut registry = Registry::new();
        registry = register_default_interceptors(registry, &mut media_engine).map_err(|e| {
            SfuError::Configuration(format!("Failed to register interceptors: {}", e))
        })?;

        let api = APIBuilder::new()
            .with_media_engine(media_engine)
            .with_interceptor_registry(registry)
            .build();

        Ok(Self {
            id,
            api: Arc::new(api),
            config,
            publishers: DashMap::new(),
            subscribers: DashMap::new(),
            metrics: Arc::new(DashMap::new()),
        })
    }

    fn register_codecs_from_config(
        media_engine: &mut MediaEngine,
        config: &SfuConfig,
    ) -> SfuResult<()> {
        for codec in &config.codecs.audio {
            let capability = RTCRtpCodecCapability {
                mime_type: codec.mime.clone(),
                clock_rate: codec.clock_rate,
                channels: codec.channels.unwrap_or(2),
                sdp_fmtp_line: codec.sdp_fmtp.clone().unwrap_or_default(),
                ..Default::default()
            };

            media_engine
                .register_codec(
                    RTCRtpCodecParameters {
                        capability,
                        payload_type: codec.payload_type,
                        ..Default::default()
                    },
                    RTPCodecType::Audio,
                )
                .map_err(|e| {
                    SfuError::Configuration(format!("Failed to register audio codec: {}", e))
                })?;
        }

        for codec in &config.codecs.video {
            let capability = RTCRtpCodecCapability {
                mime_type: codec.mime.clone(),
                clock_rate: codec.clock_rate,
                sdp_fmtp_line: codec.sdp_fmtp.clone().unwrap_or_default(),
                ..Default::default()
            };

            media_engine
                .register_codec(
                    RTCRtpCodecParameters {
                        capability,
                        payload_type: codec.payload_type,
                        ..Default::default()
                    },
                    RTPCodecType::Video,
                )
                .map_err(|e| {
                    SfuError::Configuration(format!("Failed to register video codec: {}", e))
                })?;
        }

        Ok(())
    }

    fn build_rtc_config(&self) -> RTCConfiguration {
        let ice_servers = self
            .config
            .ice_servers
            .iter()
            .map(|url| RTCIceServer {
                urls: vec![url.clone()],
                ..Default::default()
            })
            .collect();

        RTCConfiguration {
            ice_servers,
            ..Default::default()
        }
    }

    fn check_publisher_limit(&self) -> SfuResult<()> {
        if self.publishers.len() >= self.config.performance.max_publishers {
            return Err(SfuError::Internal(format!(
                "Maximum publisher limit reached: {}",
                self.config.performance.max_publishers
            )));
        }
        Ok(())
    }

    fn check_subscriber_limit(&self, publisher_id: &str) -> SfuResult<()> {
        let subscriber_count = self
            .subscribers
            .iter()
            .filter(|entry| entry.value().publisher_id == publisher_id)
            .count();

        if subscriber_count >= self.config.performance.max_subscribers_per_publisher {
            return Err(SfuError::Internal(format!(
                "Maximum subscriber limit reached for publisher {}: {}",
                publisher_id, self.config.performance.max_subscribers_per_publisher
            )));
        }
        Ok(())
    }

    async fn setup_connection_state_handler(
        &self,
        pc: &Arc<RTCPeerConnection>,
        peer_id: String,
        peer_type: &str,
    ) {
        let peer_id_clone = peer_id.clone();
        let peer_type_str = peer_type.to_string();

        pc.on_peer_connection_state_change(Box::new(move |state: RTCPeerConnectionState| {
            let id = peer_id_clone.clone();
            let ptype = peer_type_str.clone();
            Box::pin(async move {
                match state {
                    RTCPeerConnectionState::Connected => {
                        info!("{} {} connected", ptype, id);
                    }
                    RTCPeerConnectionState::Disconnected => {
                        warn!("{} {} disconnected", ptype, id);
                    }
                    RTCPeerConnectionState::Failed => {
                        warn!("{} {} connection failed", ptype, id);
                    }
                    RTCPeerConnectionState::Closed => {
                        info!("{} {} connection closed", ptype, id);
                    }
                    _ => {}
                }
            })
        }));
    }

    fn update_metrics(&self, key: &str, delta: isize) {
        self.metrics
            .entry(key.to_string())
            .and_modify(|v| *v = ((*v as isize) + delta).max(0) as usize)
            .or_insert((delta.max(0)) as usize);
    }
}

#[async_trait::async_trait]
impl Sfu for LocalSfu {
    fn id(&self) -> &str {
        &self.id
    }

    async fn add_publisher(&self, req: PublisherRequest) -> Result<PublisherResponse> {
        info!("Adding publisher: {}", req.publisher_id);

        self.check_publisher_limit()
            .context("Publisher limit check failed")?;

        let pc = Arc::new(
            self.api
                .new_peer_connection(self.build_rtc_config())
                .await
                .map_err(|e| SfuError::PeerConnectionCreation(e.to_string()))?,
        );

        self.setup_connection_state_handler(&pc, req.publisher_id.clone(), "Publisher")
            .await;

        if let Some(ice_tx) = req.ice_candidate_tx {
            pc.on_ice_candidate(Box::new(move |candidate| {
                let ice_tx = ice_tx.clone();
                Box::pin(async move {
                    if let Some(candidate) = candidate {
                        if let Ok(init) = candidate.to_json() {
                            let _ = ice_tx.send(init);
                        }
                    }
                })
            }));
        }

        let session = Arc::new(PublisherSession::new(Arc::clone(&pc)));
        let session_clone = Arc::clone(&session);
        let pub_id = req.publisher_id.clone();
        let channel_capacity = self.config.performance.broadcast_channel_capacity;
        let pc_for_pli = Arc::clone(&pc);

        pc.on_track(Box::new(move |track, receiver, _| {
            let session = Arc::clone(&session_clone);
            let pub_id = pub_id.clone();
            let pc_for_broadcaster = Arc::clone(&pc_for_pli);

            Box::pin(async move {
                let track_id = track.id();
                let kind = track.kind();

                let params = receiver.get_parameters().await;
                let (mime_type, codec_capability) = if let Some(codec) = params.codecs.first() {
                    (codec.capability.mime_type.clone(), codec.capability.clone())
                } else {
                    let default_mime = match kind.to_string().as_str() {
                        "video" => "video/VP8".to_string(),
                        "audio" => "audio/opus".to_string(),
                        _ => format!("{}/unknown", kind),
                    };
                    let default_capability = RTCRtpCodecCapability {
                        mime_type: default_mime.clone(),
                        ..Default::default()
                    };
                    (default_mime, default_capability)
                };

                info!(
                    "Publisher {} added track: {} ({}, codec: {}, fmtp: '{}')",
                    pub_id, track_id, kind, mime_type, codec_capability.sdp_fmtp_line
                );

                let broadcaster = Arc::new(TrackBroadcaster::new(
                    track,
                    pc_for_broadcaster,
                    mime_type,
                    codec_capability,
                    channel_capacity,
                ));
                session.add_broadcaster(track_id.to_string(), broadcaster);
            })
        }));

        pc.set_remote_description(req.offer)
            .await
            .map_err(|e| SfuError::SetRemoteDescription(e.to_string()))?;

        let answer = pc
            .create_answer(None)
            .await
            .map_err(|e| SfuError::CreateAnswer(e.to_string()))?;

        pc.set_local_description(answer.clone())
            .await
            .map_err(|e| SfuError::SetLocalDescription(e.to_string()))?;

        self.publishers.insert(req.publisher_id.clone(), session);
        self.update_metrics("publishers", 1);

        Ok(PublisherResponse {
            answer,
            publisher_id: req.publisher_id,
        })
    }

    async fn update_publisher(
        &self,
        req: PublisherUpdateRequest,
    ) -> Result<PublisherUpdateResponse> {
        let pub_session = self
            .publishers
            .get(&req.publisher_id)
            .ok_or_else(|| SfuError::PublisherNotFound(req.publisher_id.clone()))?;

        let pc = &pub_session.pc;

        pc.set_remote_description(req.offer)
            .await
            .map_err(|e| SfuError::SetRemoteDescription(e.to_string()))?;

        let answer = pc
            .create_answer(None)
            .await
            .map_err(|e| SfuError::CreateAnswer(e.to_string()))?;

        pc.set_local_description(answer.clone())
            .await
            .map_err(|e| SfuError::SetLocalDescription(e.to_string()))?;

        Ok(PublisherUpdateResponse { answer })
    }

    async fn remove_publisher(&self, publisher_id: &str) -> Result<()> {
        if let Some((_, _session)) = self.publishers.remove(publisher_id) {
            info!("Removing publisher: {}", publisher_id);
            self.update_metrics("publishers", -1);
        }
        Ok(())
    }

    async fn add_subscriber(&self, req: SubscriberRequest) -> Result<SubscriberResponse> {
        self.check_subscriber_limit(&req.publisher_id)
            .context("Subscriber limit check failed")?;

        let pub_session = self
            .publishers
            .get(&req.publisher_id)
            .ok_or_else(|| SfuError::PublisherNotFound(req.publisher_id.clone()))?;

        info!(
            "Adding subscriber {} to publisher {}",
            req.subscriber_id, req.publisher_id
        );

        let pc = Arc::new(
            self.api
                .new_peer_connection(self.build_rtc_config())
                .await
                .map_err(|e| SfuError::PeerConnectionCreation(e.to_string()))?,
        );

        self.setup_connection_state_handler(&pc, req.subscriber_id.clone(), "Subscriber")
            .await;

        if let Some(ice_tx) = req.ice_candidate_tx {
            pc.on_ice_candidate(Box::new(move |candidate| {
                let ice_tx = ice_tx.clone();
                Box::pin(async move {
                    if let Some(candidate) = candidate {
                        if let Ok(init) = candidate.to_json() {
                            let _ = ice_tx.send(init);
                        }
                    }
                })
            }));
        }

        let broadcasters = pub_session.get_all_broadcasters();
        let mut track_mapping = Vec::with_capacity(broadcasters.len());

        for (original_track_id, broadcaster) in broadcasters {
            let local_track_id = format!("{}-{}", original_track_id, req.subscriber_id);

            let local_track = Arc::new(TrackLocalStaticRTP::new(
                broadcaster.codec_capability.clone(),
                local_track_id.clone(),
                format!("stream-{}", req.publisher_id),
            ));

            let rtp_sender = pc
                .add_track(Arc::clone(&local_track) as Arc<dyn TrackLocal + Send + Sync>)
                .await
                .map_err(|e| SfuError::AddTrack(e.to_string()))?;

            let broadcaster_for_rtcp = Arc::clone(&broadcaster);
            let track_kind = broadcaster.kind.clone();
            tokio::spawn(async move {
                use webrtc::rtcp::payload_feedbacks::full_intra_request::FullIntraRequest;
                use webrtc::rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication;

                let mut rtcp_buf = vec![0u8; 1500];
                while let Ok((packets, _)) = rtp_sender.read(&mut rtcp_buf).await {
                    if track_kind != "video" {
                        continue;
                    }

                    for packet in packets {
                        if packet
                            .as_any()
                            .downcast_ref::<PictureLossIndication>()
                            .is_some()
                            || packet.as_any().downcast_ref::<FullIntraRequest>().is_some()
                        {
                            broadcaster_for_rtcp.request_keyframe();
                            break;
                        }
                    }
                }
            });

            broadcaster.add_subscriber(local_track).await;
            track_mapping.push((original_track_id, local_track_id));
        }

        pc.set_remote_description(req.offer)
            .await
            .map_err(|e| SfuError::SetRemoteDescription(e.to_string()))?;

        let answer = pc
            .create_answer(None)
            .await
            .map_err(|e| SfuError::CreateAnswer(e.to_string()))?;

        pc.set_local_description(answer.clone())
            .await
            .map_err(|e| SfuError::SetLocalDescription(e.to_string()))?;

        let sub_session = Arc::new(SubscriberSession::new(
            pc,
            req.publisher_id.clone(),
            track_mapping,
        ));

        self.subscribers.insert(req.subscriber_id, sub_session);
        self.update_metrics("subscribers", 1);

        Ok(SubscriberResponse { answer })
    }

    async fn remove_subscriber(&self, subscriber_id: &str) -> Result<()> {
        if let Some((_, session)) = self.subscribers.remove(subscriber_id) {
            info!("Removing subscriber: {}", subscriber_id);

            if let Some(pub_session) = self.publishers.get(&session.publisher_id) {
                for (original_track_id, local_track_id) in &session.track_mapping {
                    if let Some(broadcaster) = pub_session.get_broadcaster(original_track_id) {
                        broadcaster.remove_subscriber(local_track_id).await;
                    }
                }
            }

            self.update_metrics("subscribers", -1);
        }
        Ok(())
    }

    async fn add_publisher_ice(
        &self,
        publisher_id: &str,
        candidate: RTCIceCandidateInit,
    ) -> Result<()> {
        let session = self
            .publishers
            .get(publisher_id)
            .ok_or_else(|| SfuError::PublisherNotFound(publisher_id.to_string()))?;

        info!("Adding ICE candidate for publisher {}", publisher_id);

        session
            .pc
            .add_ice_candidate(candidate)
            .await
            .map_err(|e| SfuError::AddIceCandidate(e.to_string()))?;

        Ok(())
    }

    async fn add_subscriber_ice(
        &self,
        subscriber_id: &str,
        candidate: RTCIceCandidateInit,
    ) -> Result<()> {
        let session = self
            .subscribers
            .get(subscriber_id)
            .ok_or_else(|| SfuError::SubscriberNotFound(subscriber_id.to_string()))?;

        info!("Adding ICE candidate for subscriber {}", subscriber_id);

        session
            .pc
            .add_ice_candidate(candidate)
            .await
            .map_err(|e| SfuError::AddIceCandidate(e.to_string()))?;

        Ok(())
    }

    async fn get_metrics(&self) -> Result<SfuMetrics> {
        let total_tracks = self
            .publishers
            .iter()
            .map(|entry| entry.broadcasters.len())
            .sum::<usize>() as i32;

        let metrics = SfuMetrics {
            instance_id: self.id.clone(),
            timestamp_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64,
            cpu_usage: 0.0, // TODO: Implement actual CPU monitoring
            memory_usage: 0,
            memory_total: 0,
            go_routines: 0,    // N/A for Rust
            uptime_seconds: 0, // TODO: Track startup time
            publisher_count: self.publishers.len() as i32,
            subscriber_count: self.subscribers.len() as i32,
            track_count: total_tracks,
            total_bitrate_bps: 0, // TODO: Track actual bitrate
            bytes_received: 0,
            bytes_sent: 0,
            packets_received: 0,
            packets_sent: 0,
            packets_lost: 0,
            rtt_ms: 0,
            nack_count: 0,
            pli_count: 0,
            fir_count: 0,
        };
        Ok(metrics)
    }

    async fn health_check(&self) -> Result<()> {
        Ok(())
    }

    async fn update_subscriber(
        &self,
        _req: SubscriberUpdateRequest,
    ) -> Result<SubscriberUpdateResponse> {
        Ok(SubscriberUpdateResponse { success: true })
    }
}

impl Drop for LocalSfu {
    fn drop(&mut self) {
        info!("LocalSfu {} shutting down", self.id);
    }
}
