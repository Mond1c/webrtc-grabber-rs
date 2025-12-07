use anyhow::{Context, Result};
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::MediaEngine;
use webrtc::api::APIBuilder;
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::media::Sample;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability;
use webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample;
use webrtc::track::track_local::TrackLocal;

#[derive(Debug, Serialize, Deserialize)]
struct GrabberMessage {
    event: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    grabber_auth: Option<GrabberAuth>,
    #[serde(skip_serializing_if = "Option::is_none")]
    offer: Option<OfferMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    answer: Option<OfferMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ice: Option<IceMessage>,
}

#[derive(Debug, Serialize, Deserialize)]
struct GrabberAuth {
    credential: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct OfferMessage {
    #[serde(rename = "type")]
    type_: String,
    sdp: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct IceMessage {
    candidate: RTCIceCandidateInit,
}

pub struct WebRTCPublisher {
    ws_url: String,
    credential: String,
    pc: Option<Arc<RTCPeerConnection>>,
    video_track: Option<Arc<TrackLocalStaticSample>>,
}

impl WebRTCPublisher {
    pub fn new(ws_url: String, credential: String) -> Self {
        Self {
            ws_url,
            credential,
            pc: None,
            video_track: None,
        }
    }

    pub async fn connect_and_publish(
        &mut self,
        _width: u32,
        _height: u32,
    ) -> Result<mpsc::UnboundedSender<Vec<u8>>> {

        let (ws_stream, _) = connect_async(&self.ws_url)
            .await
            .context("Failed to connect to WebSocket")?;

        let (mut ws_tx, mut ws_rx) = ws_stream.split();

        let auth_msg = GrabberMessage {
            event: "AUTH".to_string(),
            grabber_auth: Some(GrabberAuth {
                credential: self.credential.clone(),
            }),
            offer: None,
            answer: None,
            ice: None,
        };

        ws_tx
            .send(Message::Text(serde_json::to_string(&auth_msg)?))
            .await
            .context("Failed to send auth")?;

        while let Some(msg) = ws_rx.next().await {
            let msg = msg.context("WebSocket error")?;
            if let Message::Text(text) = msg {
                let parsed: GrabberMessage = serde_json::from_str(&text)?;
                if parsed.event == "INIT_PEER" {
                    break;
                }
            }
        }

        let mut media_engine = MediaEngine::default();

        use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecParameters;

        let fmtp = "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e01f;x-google-max-bitrate=15000;x-google-min-bitrate=1000;x-google-start-bitrate=5000".to_owned();

        media_engine.register_codec(
            RTCRtpCodecParameters {
                capability: RTCRtpCodecCapability {
                    mime_type: "video/H264".to_owned(),
                    clock_rate: 90000,
                    channels: 0,
                    sdp_fmtp_line: fmtp,
                    rtcp_feedback: vec![],
                },
                payload_type: 102,
                ..Default::default()
            },
            webrtc::rtp_transceiver::rtp_codec::RTPCodecType::Video,
        )?;

        let mut registry = webrtc::interceptor::registry::Registry::new();
        registry = register_default_interceptors(registry, &mut media_engine)?;

        let api = APIBuilder::new()
            .with_media_engine(media_engine)
            .with_interceptor_registry(registry)
            .build();

        let config = RTCConfiguration {
            ice_servers: vec![RTCIceServer {
                urls: vec![],
                ..Default::default()
            }],
            ..Default::default()
        };

        let pc = Arc::new(api.new_peer_connection(config).await?);

        let video_track = Arc::new(TrackLocalStaticSample::new(
            RTCRtpCodecCapability {
                mime_type: "video/H264".to_owned(),
                ..Default::default()
            },
            "video".to_owned(),
            "webcam".to_owned(),
        ));

        pc.add_track(Arc::clone(&video_track) as Arc<dyn TrackLocal + Send + Sync>)
            .await?;

        let ws_tx_clone = Arc::new(tokio::sync::Mutex::new(ws_tx));
        let ws_tx_for_ice = Arc::clone(&ws_tx_clone);

        pc.on_ice_candidate(Box::new(move |candidate| {
            let ws_tx = Arc::clone(&ws_tx_for_ice);
            Box::pin(async move {
                if let Some(candidate) = candidate {
                    if let Ok(init) = candidate.to_json() {
                        let ice_msg = GrabberMessage {
                            event: "GRABBER_ICE".to_string(),
                            grabber_auth: None,
                            offer: None,
                            answer: None,
                            ice: Some(IceMessage { candidate: init }),
                        };

                        if let Ok(json) = serde_json::to_string(&ice_msg) {
                            let _ = ws_tx.lock().await.send(Message::Text(json)).await;
                        }
                    }
                }
            })
        }));

        use webrtc::peer_connection::offer_answer_options::RTCOfferOptions;
        let offer = pc
            .create_offer(Some(RTCOfferOptions {
                ..Default::default()
            }))
            .await?;

        pc.set_local_description(offer.clone()).await?;

        let offer_msg = GrabberMessage {
            event: "OFFER".to_string(),
            grabber_auth: None,
            offer: Some(OfferMessage {
                type_: "offer".to_string(),
                sdp: offer.sdp,
            }),
            answer: None,
            ice: None,
        };

        ws_tx_clone
            .lock()
            .await
            .send(Message::Text(serde_json::to_string(&offer_msg)?))
            .await?;

        let mut answer_received = false;
        while let Some(msg) = ws_rx.next().await {
            let msg = msg.context("WebSocket error")?;
            if let Message::Text(text) = msg {
                let parsed: GrabberMessage = serde_json::from_str(&text)?;

                match parsed.event.as_str() {
                    "ANSWER" => {
                        if let Some(answer_data) = parsed.answer {
                            let answer = RTCSessionDescription::answer(answer_data.sdp)?;
                            pc.set_remote_description(answer).await?;
                            answer_received = true;
                            break;
                        }
                    }
                    "SERVER_ICE" => {
                        if let Some(ice_data) = parsed.ice {
                            pc.add_ice_candidate(ice_data.candidate).await?;
                        }
                    }
                    "OFFER_FAILED" => {
                        anyhow::bail!("Server rejected offer: OFFER_FAILED");
                    }
                    _ => {}
                }
            }
        }

        if !answer_received {
            anyhow::bail!("Connection closed before receiving answer");
        }

        let (frame_tx, mut frame_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let video_track_clone = Arc::clone(&video_track);

        tokio::spawn(async move {
            let frame_duration = std::time::Duration::from_micros(33_333);

            while let Some(frame_data) = frame_rx.recv().await {
                let sample = Sample {
                    data: frame_data.into(),
                    duration: frame_duration,
                    ..Default::default()
                };

                if video_track_clone.write_sample(&sample).await.is_err() {
                    break;
                }
            }
        });

        tokio::spawn(async move {
            while let Some(msg) = ws_rx.next().await {
                if let Ok(Message::Text(_text)) = msg {}
            }
        });

        self.pc = Some(pc);
        self.video_track = Some(video_track);

        Ok(frame_tx)
    }
}
