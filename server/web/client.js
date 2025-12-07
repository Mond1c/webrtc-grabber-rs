// WebRTC SFU Client
const WS_URL = `ws://${window.location.host}`;

class Logger {
    constructor(element) {
        this.element = element;
    }

    log(message, type = 'info') {
        const entry = document.createElement('div');
        entry.className = `log-entry ${type}`;
        entry.textContent = `[${new Date().toLocaleTimeString()}] ${message}`;
        this.element.appendChild(entry);
        this.element.scrollTop = this.element.scrollHeight;
    }

    error(message) {
        this.log(message, 'error');
    }

    success(message) {
        this.log(message, 'success');
    }
}

class Publisher {
    constructor(logger) {
        this.logger = logger;
        this.ws = null;
        this.pc = null;
        this.stream = null;
        this.pendingIceCandidates = [];
        this.remoteDescriptionSet = false;
    }

    async start(name, sourceType) {
        try {
            this.logger.log(`Starting publisher: ${name}`);

            // Check if APIs are available
            if (!navigator.mediaDevices) {
                throw new Error('navigator.mediaDevices is not available. Please use HTTPS.');
            }

            // Get media stream
            if (sourceType === 'webcam') {
                this.stream = await navigator.mediaDevices.getUserMedia({
                    video: {
                        width: { ideal: 1280 },
                        height: { ideal: 720 },
                        aspectRatio: 16 / 9,
                        frameRate: { ideal: 30, max: 30 }
                    },
                    audio: true
                });
            } else {
                // Screen share - check if getDisplayMedia is available
                if (!navigator.mediaDevices.getDisplayMedia) {
                    throw new Error('Screen sharing is not supported in this browser');
                }

                this.stream = await navigator.mediaDevices.getDisplayMedia({
                    video: {
                        displaySurface: "window",
                        frameRate: { ideal: 30, max: 30 }
                    },
                    audio: true
                });
            }

            document.getElementById('localVideo').srcObject = this.stream;
            this.logger.success('Media captured successfully');

            // Connect WebSocket
            this.ws = new WebSocket(`${WS_URL}/grabber/${name}`);

            this.ws.onopen = () => {
                this.logger.success('WebSocket connected');
            };

            this.ws.onmessage = async (event) => {
                const msg = JSON.parse(event.data);
                await this.handleMessage(msg);
            };

            this.ws.onerror = (error) => {
                this.logger.error(`WebSocket error: ${error}`);
            };

            this.ws.onclose = () => {
                this.logger.log('WebSocket closed');
                this.stop();
            };

        } catch (error) {
            this.logger.error(`Failed to start: ${error.message}`);
            throw error;
        }
    }

    async handleMessage(msg) {
        this.logger.log(`Received: ${msg.event}`);

        switch (msg.event) {
            case 'INIT_PEER':
                await this.initPeerConnection(msg.initPeer.pcConfig);
                break;

            case 'ANSWER':
                await this.handleAnswer(msg.answer);
                break;

            case 'SERVER_ICE':
                await this.handleServerIce(msg.ice);
                break;

            case 'PONG':
                // Heartbeat response
                break;

            default:
                this.logger.log(`Unknown event: ${msg.event}`);
        }
    }

    async initPeerConnection(config) {
        this.logger.log('Initializing peer connection');

        this.pc = new RTCPeerConnection({
            iceServers: config.iceServers
        });

        // Add tracks to peer connection
        this.stream.getTracks().forEach(track => {
            this.pc.addTrack(track, this.stream);
            this.logger.log(`Added track: ${track.kind}`);
        });

        // ICE candidate handling
        this.pc.onicecandidate = (event) => {
            if (event.candidate) {
                this.ws.send(JSON.stringify({
                    event: 'GRABBER_ICE',
                    ice: {
                        candidate: {
                            candidate: event.candidate.candidate,
                            sdp_mid: event.candidate.sdpMid,
                            sdpMLineIndex: event.candidate.sdpMLineIndex,
                            username_fragment: event.candidate.usernameFragment
                        }
                    }
                }));
            }
        };

        this.pc.onconnectionstatechange = () => {
            this.logger.log(`Connection state: ${this.pc.connectionState}`);
            if (this.pc.connectionState === 'connected') {
                this.logger.success('Peer connection established!');
                this.startStats();
            }
        };

        // Create and send offer
        const offer = await this.pc.createOffer();
        await this.pc.setLocalDescription(offer);

        this.ws.send(JSON.stringify({
            event: 'OFFER',
            offer: {
                type: offer.type,
                sdp: offer.sdp
            }
        }));

        this.logger.log('Offer sent');
    }

    async handleAnswer(answer) {
        try {
            this.logger.log('Setting remote description');
            this.logger.log(`Answer type: ${answer.type}, SDP length: ${answer.sdp?.length || 0}`);

            await this.pc.setRemoteDescription({
                type: answer.type,
                sdp: answer.sdp
            });

            this.logger.success('Remote description set');
            this.remoteDescriptionSet = true;

            // Process any pending ICE candidates
            this.logger.log(`Processing ${this.pendingIceCandidates.length} queued ICE candidates`);
            for (const candidate of this.pendingIceCandidates) {
                try {
                    await this.pc.addIceCandidate(candidate);
                    this.logger.success('Queued server ICE candidate added');
                } catch (error) {
                    this.logger.error(`Failed to add queued ICE candidate: ${error.message}`);
                }
            }
            this.pendingIceCandidates = [];
        } catch (error) {
            this.logger.error(`Failed to set remote description: ${error.message}`);
            console.error('setRemoteDescription error:', error);
        }
    }

    async handleServerIce(ice) {
        if (this.pc && ice && ice.candidate) {
            this.logger.log(`Received server ICE candidate`);

            if (this.remoteDescriptionSet) {
                // Remote description is already set, add candidate immediately
                try {
                    await this.pc.addIceCandidate(ice.candidate);
                    this.logger.success('Server ICE candidate added');
                } catch (error) {
                    this.logger.error(`Failed to add server ICE candidate: ${error.message}`);
                }
            } else {
                // Queue the candidate until remote description is set
                this.logger.log('Queueing ICE candidate until remote description is set');
                this.pendingIceCandidates.push(ice.candidate);
            }
        }
    }

    startStats() {
        const statsEl = document.getElementById('publishStats');
        statsEl.style.display = 'grid';

        setInterval(async () => {
            if (!this.pc) return;

            const stats = await this.pc.getStats();
            let bitrate = 0;
            let packets = 0;

            stats.forEach(report => {
                if (report.type === 'outbound-rtp') {
                    bitrate += (report.bytesSent * 8) / 1000;
                    packets += report.packetsSent || 0;
                }
            });

            document.getElementById('pubBitrate').textContent = Math.round(bitrate);
            document.getElementById('pubPackets').textContent = packets;
        }, 1000);
    }

    stop() {
        if (this.stream) {
            this.stream.getTracks().forEach(track => track.stop());
            this.stream = null;
        }

        if (this.pc) {
            this.pc.close();
            this.pc = null;
        }

        if (this.ws) {
            this.ws.close();
            this.ws = null;
        }

        document.getElementById('localVideo').srcObject = null;
        document.getElementById('publishStats').style.display = 'none';
        this.logger.log('Publisher stopped');
    }
}

class Viewer {
    constructor(logger) {
        this.logger = logger;
        this.ws = null;
        this.pc = null;
        this.authenticated = false;
        this.pendingIceCandidates = [];
        this.remoteDescriptionSet = false;
    }

    async start(peerName) {
        try {
            this.logger.log('Starting viewer');

            // Connect WebSocket
            this.ws = new WebSocket(`${WS_URL}/player`);

            this.ws.onopen = () => {
                this.logger.success('WebSocket connected');
            };

            this.ws.onmessage = async (event) => {
                const msg = JSON.parse(event.data);
                await this.handleMessage(msg, peerName);
            };

            this.ws.onerror = (error) => {
                this.logger.error(`WebSocket error: ${error}`);
            };

            this.ws.onclose = () => {
                this.logger.log('WebSocket closed');
                this.stop();
            };

        } catch (error) {
            this.logger.error(`Failed to start viewer: ${error.message}`);
            throw error;
        }
    }

    async handleMessage(msg, peerName) {
        this.logger.log(`Received: ${msg.event}`);

        switch (msg.event) {
            case 'AUTH_REQUEST':
                // Send auth (using dummy credentials for testing)
                this.ws.send(JSON.stringify({
                    event: 'AUTH',
                    playerAuth: {
                        credential: 'test'
                    }
                }));
                break;

            case 'AUTH_FAILED':
                this.logger.error('Authentication failed');
                break;

            case 'INIT_PEER':
                this.authenticated = true;
                await this.initPeerConnection(msg.initPeer.pcConfig, peerName);
                break;

            case 'ANSWER':
                await this.handleAnswer(msg.offer);
                break;

            case 'SERVER_ICE':
                await this.handleServerIce(msg.ice);
                break;

            case 'OFFER_FAILED':
                this.logger.error('Offer failed - peer may not exist');
                break;

            case 'PONG':
                // Heartbeat response
                break;

            default:
                this.logger.log(`Unknown event: ${msg.event}`);
        }
    }

    async initPeerConnection(config, peerName) {
        this.logger.log('Initializing peer connection');

        this.pc = new RTCPeerConnection({
            iceServers: config.iceServers
        });

        // Handle incoming tracks
        this.pc.ontrack = (event) => {
            this.logger.success(`Received ${event.track.kind} track`);
            const video = document.getElementById('remoteVideo');
            if (!video.srcObject) {
                video.srcObject = event.streams[0];
            }
        };

        // ICE candidate handling
        this.pc.onicecandidate = (event) => {
            if (event.candidate) {
                this.ws.send(JSON.stringify({
                    event: 'PLAYER_ICE',
                    ice: {
                        candidate: {
                            candidate: event.candidate.candidate,
                            sdp_mid: event.candidate.sdpMid,
                            sdpMLineIndex: event.candidate.sdpMLineIndex,
                            username_fragment: event.candidate.usernameFragment
                        }
                    }
                }));
            }
        };

        this.pc.onconnectionstatechange = () => {
            this.logger.log(`Connection state: ${this.pc.connectionState}`);
            if (this.pc.connectionState === 'connected') {
                this.logger.success('Peer connection established!');
                this.startStats();
            }
        };

        // Add transceiver for receiving
        this.pc.addTransceiver('video', { direction: 'recvonly' });
        this.pc.addTransceiver('audio', { direction: 'recvonly' });

        // Create and send offer
        const offer = await this.pc.createOffer();
        await this.pc.setLocalDescription(offer);

        this.ws.send(JSON.stringify({
            event: 'OFFER',
            offer: {
                type: offer.type,
                sdp: offer.sdp,
                peerName: peerName
            }
        }));

        this.logger.log(`Offer sent for peer: ${peerName}`);
    }

    async handleAnswer(answer) {
        try {
            this.logger.log('Setting remote description');
            this.logger.log(`Answer type: ${answer.type}, SDP length: ${answer.sdp?.length || 0}`);

            await this.pc.setRemoteDescription({
                type: answer.type,
                sdp: answer.sdp
            });

            this.logger.success('Remote description set');
            this.remoteDescriptionSet = true;

            // Process any pending ICE candidates
            this.logger.log(`Processing ${this.pendingIceCandidates.length} queued ICE candidates`);
            for (const candidate of this.pendingIceCandidates) {
                try {
                    await this.pc.addIceCandidate(candidate);
                    this.logger.success('Queued server ICE candidate added');
                } catch (error) {
                    this.logger.error(`Failed to add queued ICE candidate: ${error.message}`);
                }
            }
            this.pendingIceCandidates = [];
        } catch (error) {
            this.logger.error(`Failed to set remote description: ${error.message}`);
            console.error('setRemoteDescription error:', error);
        }
    }

    async handleServerIce(ice) {
        if (this.pc && ice && ice.candidate) {
            this.logger.log(`Received server ICE candidate`);

            if (this.remoteDescriptionSet) {
                // Remote description is already set, add candidate immediately
                try {
                    await this.pc.addIceCandidate(ice.candidate);
                    this.logger.success('Server ICE candidate added');
                } catch (error) {
                    this.logger.error(`Failed to add server ICE candidate: ${error.message}`);
                }
            } else {
                // Queue the candidate until remote description is set
                this.logger.log('Queueing ICE candidate until remote description is set');
                this.pendingIceCandidates.push(ice.candidate);
            }
        }
    }

    startStats() {
        const statsEl = document.getElementById('watchStats');
        statsEl.style.display = 'grid';

        setInterval(async () => {
            if (!this.pc) return;

            const stats = await this.pc.getStats();
            let bitrate = 0;
            let packets = 0;

            stats.forEach(report => {
                if (report.type === 'inbound-rtp') {
                    bitrate += (report.bytesReceived * 8) / 1000;
                    packets += report.packetsReceived || 0;
                }
            });

            document.getElementById('subBitrate').textContent = Math.round(bitrate);
            document.getElementById('subPackets').textContent = packets;
        }, 1000);
    }

    stop() {
        if (this.pc) {
            this.pc.close();
            this.pc = null;
        }

        if (this.ws) {
            this.ws.close();
            this.ws = null;
        }

        document.getElementById('remoteVideo').srcObject = null;
        document.getElementById('watchStats').style.display = 'none';
        this.logger.log('Viewer stopped');
    }
}

// UI Controllers
const logger = new Logger(document.getElementById('logs'));
let publisher = null;
let viewer = null;

// Publisher controls
document.getElementById('startPublish').addEventListener('click', async () => {
    const name = document.getElementById('publisherName').value;
    const sourceType = document.getElementById('sourceType').value;

    if (!name) {
        alert('Please enter a stream name');
        return;
    }

    try {
        publisher = new Publisher(logger);
        await publisher.start(name, sourceType);

        document.getElementById('startPublish').disabled = true;
        document.getElementById('stopPublish').disabled = false;
        document.getElementById('publishStatus').innerHTML = '<div class="status success">Publishing...</div>';
    } catch (error) {
        document.getElementById('publishStatus').innerHTML = `<div class="status error">Error: ${error.message}</div>`;
    }
});

document.getElementById('stopPublish').addEventListener('click', () => {
    if (publisher) {
        publisher.stop();
        publisher = null;
    }

    document.getElementById('startPublish').disabled = false;
    document.getElementById('stopPublish').disabled = true;
    document.getElementById('publishStatus').innerHTML = '';
});

// Viewer controls
document.getElementById('startWatch').addEventListener('click', async () => {
    const peerName = document.getElementById('peerSelect').value;

    if (!peerName) {
        alert('Please select a peer');
        return;
    }

    try {
        viewer = new Viewer(logger);
        await viewer.start(peerName);

        document.getElementById('startWatch').disabled = true;
        document.getElementById('stopWatch').disabled = false;
        document.getElementById('watchStatus').innerHTML = '<div class="status success">Watching...</div>';
    } catch (error) {
        document.getElementById('watchStatus').innerHTML = `<div class="status error">Error: ${error.message}</div>`;
    }
});

document.getElementById('stopWatch').addEventListener('click', () => {
    if (viewer) {
        viewer.stop();
        viewer = null;
    }

    document.getElementById('startWatch').disabled = false;
    document.getElementById('stopWatch').disabled = true;
    document.getElementById('watchStatus').innerHTML = '';
});

// Refresh peers list
document.getElementById('refreshPeers').addEventListener('click', async () => {
    try {
        logger.log('Fetching peer list...');
        const response = await fetch('/api/peers');
        const data = await response.json();

        const select = document.getElementById('peerSelect');
        const currentValue = select.value;

        // Clear existing options except first
        while (select.options.length > 1) {
            select.remove(1);
        }

        // Add peers
        data.peers.forEach(peer => {
            const option = document.createElement('option');
            option.value = peer.name;
            option.textContent = `${peer.name} (${peer.connections} connections)`;
            select.appendChild(option);
        });

        // Restore selection if it still exists
        if (currentValue && Array.from(select.options).some(opt => opt.value === currentValue)) {
            select.value = currentValue;
        }

        logger.success(`Found ${data.peers.length} peer(s)`);
    } catch (error) {
        logger.error(`Failed to fetch peers: ${error.message}`);
    }
});

// Auto-refresh peers every 5 seconds
setInterval(() => {
    document.getElementById('refreshPeers').click();
}, 5000);

// Initial log
logger.log('WebRTC SFU Client initialized');
logger.log(`Server: ${WS_URL}`);
