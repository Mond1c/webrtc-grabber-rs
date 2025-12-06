use std::{collections::HashMap, sync::Arc};

use dashmap::DashMap;
use sfu_core::{PublisherRequest, PublisherResponse, PublisherUpdateRequest, PublisherUpdateResponse, Sfu, SubscriberRequest, SubscriberResponse, SubscriberUpdateRequest, SubscriberUpdateResponse};
use anyhow::{Context, Result, anyhow};
use sfu_proto::SfuMetrics;
use tokio::sync::Mutex;
use tracing::{info, warn};
use webrtc::{api::{API, APIBuilder, interceptor_registry::register_default_interceptors, media_engine::MediaEngine}, ice_transport::{ice_candidate::RTCIceCandidate, ice_server::RTCIceServer}, interceptor::registry::Registry, peer_connection::{RTCPeerConnection, configuration::RTCConfiguration}, rtp_transceiver::rtp_codec::{RTCRtpCodecCapability, RTCRtpCodecParameters, RTPCodecType}, track::track_local::{TrackLocal, track_local_static_rtp::TrackLocalStaticRTP}};

use crate::{broadcaster::TrackBroadcaster, config::SfuConfig};


struct PublisherSession {
    pc: Arc<RTCPeerConnection>,
    broadcasters: Arc<Mutex<HashMap<String, Arc<TrackBroadcaster>>>>,
}

struct SubscriberSession {
    pc: Arc<RTCPeerConnection>,
    publisher_id: String,
    subscribed_track_ids: Vec<String>,
}

pub struct LocalSfu {
    id: String,
    api: Arc<API>,
    config: SfuConfig,

    publishers: DashMap<String, Arc<PublisherSession>>,
    subscribers: DashMap<String, Arc<SubscriberSession>>,
}

impl LocalSfu {
    pub fn new(id: String, config: SfuConfig) -> Result<Self> {

        let mut media_engine = MediaEngine::default();
        media_engine.register_default_codecs();

        Self::register_codecs_from_config(&mut media_engine, &config)?;

        let mut registry = Registry::new();
        registry = register_default_interceptors(registry, &mut media_engine)?;

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
        })
    }

    fn register_codecs_from_config(media_engine: &mut MediaEngine, config: &SfuConfig) -> Result<()> {
        for codec in &config.codecs.audio {
            let capability = RTCRtpCodecCapability {
                mime_type: codec.mime.clone(),
                clock_rate: codec.clock_rate,
                channels: codec.channels.unwrap_or(2),
                sdp_fmtp_line: codec.sdp_fmtp.clone().unwrap_or_default(),
                ..Default::default()
            };

            media_engine.register_codec(
                RTCRtpCodecParameters {
                    capability,
                    payload_type: codec.payload_type,
                    ..Default::default()
                },
                RTPCodecType::Audio,
            )?;
        }

        for codec in &config.codecs.video {
            let capability = RTCRtpCodecCapability {
                mime_type: codec.mime.clone(),
                clock_rate: codec.clock_rate,
                sdp_fmtp_line: codec.sdp_fmtp.clone().unwrap_or_default(),
                ..Default::default()
            };

            media_engine.register_codec(
                RTCRtpCodecParameters {
                    capability,
                    payload_type: codec.payload_type,
                    ..Default::default()
                },
            RTPCodecType::Video,
            )?;
        }

        Ok(())
    }

    fn get_rtc_config(&self) -> RTCConfiguration {
        let ice_servers = self.config.ice_servers.iter()
            .map(|url| RTCIceServer { urls: vec![url.clone()], ..Default::default() })
            .collect();
        
        RTCConfiguration {
            ice_servers,
            ..Default::default()
        }
    }
}

#[async_trait::async_trait]
impl Sfu for LocalSfu {
    fn id(&self) ->  &str {
        &self.id
    }

    async fn add_publisher(&self, req: PublisherRequest) -> Result<PublisherResponse> {
        info!("Adding publisher: {}", req.publisher_id);
        
        let pc = self
            .api
            .new_peer_connection(self.get_rtc_config())
            .await
            .context("Failed to create PeerConnection for publisher")?;

        let pc = Arc::new(pc);

        let broadcasters = Arc::new(Mutex::new(HashMap::new()));
        let broadcasters_clone = broadcasters.clone();
        let pub_id = req.publisher_id.clone();

        pc.on_track(Box::new(move |track, _, _| {
            let broadcasters = broadcasters_clone.clone();
            let pub_id = pub_id.clone();

            Box::pin(async move {
                let track_id = track.id();
                let kind = track.kind();
                info!("Publisher {} added track: {} ({})", pub_id, track_id, kind);

                let broadcaster = Arc::new(TrackBroadcaster::new(track));

                let mut lock = broadcasters.lock().await;
                lock.insert(track_id, broadcaster);
            })
        }));

        pc.set_remote_description(req.offer)
            .await
            .context("Failed to set remote description")?;

        let answer = pc
            .create_answer(None)
            .await
            .context("Failed to create answer")?;
        
        pc.set_local_description(answer.clone())
            .await
            .context("Failed to set local description")?;

        let session = Arc::new(PublisherSession {
            pc: pc.clone(),
            broadcasters,
        });
        
        self.publishers.insert(req.publisher_id.clone(), session);

        Ok(PublisherResponse {
            answer,
            publisher_id: req.publisher_id,
        })
    }

    async fn update_publisher(&self, req: PublisherUpdateRequest) -> Result<PublisherUpdateResponse> {
        let pub_session = self
            .publishers
            .get(&req.publisher_id)
            .ok_or_else(|| anyhow!("Publisher not found: {}", req.publisher_id))?;

        let pc = &pub_session.pc;

        pc.set_remote_description(req.offer)
            .await
            .context("Update: set remote description failed")?;
        
        let answer = pc
            .create_answer(None)
            .await
            .context("Update: create answer failed")?;
        
        pc.set_local_description(answer.clone())
            .await
            .context("Update: set local description failed")?;

        Ok(PublisherUpdateResponse { answer })
    }

    async fn remove_publisher(&self, publisher_id: &str) -> Result<()> {
        if let Some((_, session)) = self.publishers.remove(publisher_id) {
            info!("Removing publisher: {}", publisher_id);
            session.pc.close().await?;
        }
        Ok(())
    }

    async fn add_subscriber(&self, req: SubscriberRequest) -> Result<SubscriberResponse> {
        let pub_session = self
            .publishers
            .get(&req.publisher_id)
            .ok_or_else(|| anyhow!("Publisher '{}' not found for subscriber '{}'", req.publisher_id, req.subscriber_id))?;

        info!(
            "Adding subscriber {} to publisher {}",
            req.subscriber_id, req.publisher_id
        );

        let pc = self
            .api
            .new_peer_connection(self.get_rtc_config())
            .await
            .context("Failed to create sub PC")?;

        let broadcasters_lock = pub_session.broadcasters.lock().await;
        let mut created_track_ids = Vec::new();

        for (original_track_id, broadcaster) in broadcasters_lock.iter() {
            let local_track_id = original_track_id.clone();
            
            let local_track = Arc::new(TrackLocalStaticRTP::new(
                RTCRtpCodecCapability {
                    mime_type: broadcaster.kind.clone(),
                    ..Default::default()
                },
                local_track_id.clone(),
                format!("stream-{}", req.publisher_id),
            ));

            pc.add_track(Arc::clone(&local_track) as Arc<dyn TrackLocal + Send + Sync>)
                .await
                .context("Failed to add track to subscriber PC")?;

            broadcaster.add_subscriber(local_track).await;

            created_track_ids.push(local_track_id);
        }

        drop(broadcasters_lock);

        pc.set_remote_description(req.offer).await?;
        let answer = pc.create_answer(None).await?;
        pc.set_local_description(answer.clone()).await?;

        let sub_session = Arc::new(SubscriberSession {
            pc: Arc::new(pc),
            publisher_id: req.publisher_id.clone(),
            subscribed_track_ids: created_track_ids,
        });

        self.subscribers.insert(req.subscriber_id, sub_session);

        Ok(SubscriberResponse { answer })
    }

    async fn remove_subscriber(&self, subscriber_id: &str) -> Result<()> {
        if let Some((_, session)) = self.subscribers.remove(subscriber_id) {
            info!("Removing subscriber: {}", subscriber_id);
            
            if let Err(e) = session.pc.close().await {
                warn!("Error closing subscriber PC: {:?}", e);
            }

            if let Some(pub_session) = self.publishers.get(&session.publisher_id) {
                let broadcasters = pub_session.broadcasters.lock().await;
                
                for track_id in &session.subscribed_track_ids {
                   if let Some(broadcaster) = broadcasters.get(track_id) {
                       broadcaster.remove_subscriber(track_id).await; 
                   }
                }
            }
        }
        Ok(())
    }

    async fn add_publisher_ice(&self, publisher_id: &str, candidate: RTCIceCandidate) -> Result<()> {
        if let Some(session) = self.publishers.get(publisher_id) {
            session.pc.add_ice_candidate(candidate.to_json()?).await?;
        }
        Ok(())
    }

    async fn add_subscriber_ice(&self, subscriber_id: &str, candidate: RTCIceCandidate) -> Result<()> {
        if let Some(session) = self.subscribers.get(subscriber_id) {
            session.pc.add_ice_candidate(candidate.to_json()?).await?;
        }
        Ok(())
    }

    async fn get_metrics(&self) -> Result<SfuMetrics> {
        Ok(SfuMetrics::default()) // Placeholder implementation
    }

    async fn health_check(&self) -> Result<()> {
        Ok(()) // placeholder implementation
    }

    async fn update_subscriber(&self, _req: SubscriberUpdateRequest) -> Result<SubscriberUpdateResponse> {
        Ok(SubscriberUpdateResponse { success: true })
    }
}