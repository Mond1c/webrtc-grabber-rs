use axum::extract::ws::{Message, WebSocket};
use axum::extract::{ConnectInfo, Path, State, WebSocketUpgrade};
use axum::response::IntoResponse;
use futures::StreamExt;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{error, info, instrument, warn};
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;

use sfu_core::PublisherRequest;

use crate::error::{Result, SignallingError};
use crate::protocol::{self, GrabberMessage};
use crate::state::AppState;
use crate::websocket::WsSession;

pub async fn ws_grabber_handler(
    ws: WebSocketUpgrade,
    Path(name): Path<String>,
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| async move {
        if let Err(e) = handle_grabber_connection(socket, addr, name, state).await {
            error!("Grabber connection error from {}: {:?}", addr, e);
        }
    })
}

#[instrument(skip(socket, state), fields(name = %name, ip = %addr))]
async fn handle_grabber_connection(
    socket: WebSocket,
    addr: SocketAddr,
    name: String,
    state: Arc<AppState>,
) -> Result<()> {
    let session_id = format!("grabber-{}", addr);
    info!("Grabber connecting");

    let (session, mut receiver) = WsSession::new(socket, session_id.clone());

    state.storage.add_peer(name.clone(), session_id.clone());

    session.send_json(&GrabberMessage {
        event: "INIT_PEER".to_string(),
        init_peer: Some(protocol::GrabberInitPeerMessage {
            pc_config: state.get_client_rtc_config(),
            ping_interval: 5000,
        }),
        ..Default::default()
    })?;

    info!("Grabber '{}' initialized", name);

    while let Some(result) = receiver.next().await {
        match result {
            Ok(Message::Text(text)) => {
                if let Err(e) = handle_grabber_message(&session, &text, &state).await {
                    warn!("Error processing grabber message: {}", e);
                }
            }
            Ok(Message::Close(_)) => {
                info!("Grabber closed connection");
                break;
            }
            Ok(Message::Ping(_)) => {
                let _ = session.send_text(format!("{{\"event\":\"PONG\"}}"));
            }
            Err(e) => {
                warn!("WebSocket error: {}", e);
                break;
            }
            _ => {}
        }
    }

    info!("Grabber '{}' disconnected", name);
    state.storage.remove_peer_by_socket_id(&session_id);
    let _ = state.sfu.remove_publisher(&session_id).await;

    Ok(())
}

async fn handle_grabber_message(session: &WsSession, text: &str, state: &AppState) -> Result<()> {
    let msg: GrabberMessage = serde_json::from_str(text)
        .map_err(|e| SignallingError::InvalidMessageFormat(e.to_string()))?;

    match msg.event.as_str() {
        "PING" => handle_ping(session, msg, state),
        "OFFER" | "OFFER_ANSWER" => handle_publisher_offer(session, msg, state).await,
        "GRABBER_ICE" => handle_grabber_ice(session, msg, state).await,
        _ => {
            warn!("Unknown grabber event: {}", msg.event);
            Ok(())
        }
    }
}

fn handle_ping(session: &WsSession, msg: GrabberMessage, state: &AppState) -> Result<()> {
    if let Some(ping) = msg.ping {
        state.storage.update_ping(
            &session.id,
            ping.connections_count.unwrap_or(0),
            ping.stream_types.unwrap_or_default(),
        );
    }
    Ok(())
}

async fn handle_publisher_offer(
    session: &WsSession,
    msg: GrabberMessage,
    state: &AppState,
) -> Result<()> {
    let offer_data = msg
        .offer
        .or(msg.answer)
        .ok_or_else(|| SignallingError::InvalidMessageFormat("Missing offer data".to_string()))?;

    let offer = RTCSessionDescription::offer(offer_data.sdp)
        .map_err(|e| SignallingError::InvalidMessageFormat(format!("Invalid SDP offer: {}", e)))?;

    let (ice_tx, mut ice_rx) = mpsc::unbounded_channel();
    let session_for_ice = session.clone();

    tokio::spawn(async move {
        while let Some(candidate) = ice_rx.recv().await {
            let _ = session_for_ice.send_json(&GrabberMessage {
                event: "SERVER_ICE".to_string(),
                ice: Some(protocol::IceMessage {
                    candidate,
                    peer_id: None,
                }),
                ..Default::default()
            });
        }
    });

    let req = PublisherRequest {
        session_id: session.id.clone(),
        publisher_id: session.id.clone(),
        offer,
        ice_candidate_tx: Some(ice_tx),
    };

    match state.sfu.add_publisher(req).await {
        Ok(res) => {
            session.send_json(&GrabberMessage {
                event: "ANSWER".to_string(),
                answer: Some(protocol::OfferMessage {
                    type_: "answer".to_string(),
                    sdp: res.answer.sdp,
                    peer_id: None,
                    peer_name: None,
                    stream_type: None,
                }),
                ..Default::default()
            })?;
            info!("Publisher '{}' added successfully", session.id);
            Ok(())
        }
        Err(e) => {
            error!("SFU add publisher error: {}", e);
            session.send_json(&GrabberMessage {
                event: "OFFER_FAILED".to_string(),
                ..Default::default()
            })?;
            Err(SignallingError::SfuError(e))
        }
    }
}

async fn handle_grabber_ice(
    session: &WsSession,
    msg: GrabberMessage,
    state: &AppState,
) -> Result<()> {
    let ice_msg = msg
        .ice
        .ok_or_else(|| SignallingError::InvalidMessageFormat("Missing ICE data".to_string()))?;

    state
        .sfu
        .add_publisher_ice(&session.id, ice_msg.candidate)
        .await
        .map_err(SignallingError::SfuError)?;

    Ok(())
}
