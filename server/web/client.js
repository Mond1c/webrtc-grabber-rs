// WebRTC SFU Multi-Viewer Client
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

        // Keep only last 100 entries
        while (this.element.children.length > 100) {
            this.element.removeChild(this.element.firstChild);
        }
    }

    error(message) {
        this.log(message, 'error');
    }

    success(message) {
        this.log(message, 'success');
    }

    warning(message) {
        this.log(message, 'warning');
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

            if (!navigator.mediaDevices) {
                throw new Error('navigator.mediaDevices is not available. Please use HTTPS.');
            }

            // Get media stream
            if (sourceType === 'webcam') {
                this.stream = await navigator.mediaDevices.getUserMedia({
                    video: { width: { ideal: 1280 }, height: { ideal: 720 }, frameRate: { ideal: 30, max: 30 } },
                    audio: true
                });
            } else {
                if (!navigator.mediaDevices.getDisplayMedia) {
                    throw new Error('Screen sharing is not supported in this browser');
                }
                this.stream = await navigator.mediaDevices.getDisplayMedia({
                    video: { frameRate: { ideal: 30, max: 30 } },
                    audio: true
                });
            }

            this.logger.success('Media captured successfully');

            // Connect WebSocket
            this.ws = new WebSocket(`${WS_URL}/grabber/${name}`);

            this.ws.onopen = () => {
                this.logger.success(`Publisher WebSocket connected for ${name}`);
            };

            this.ws.onmessage = async (event) => {
                const msg = JSON.parse(event.data);
                await this.handleMessage(msg);
            };

            this.ws.onerror = (error) => {
                this.logger.error(`Publisher WebSocket error: ${error}`);
            };

            this.ws.onclose = () => {
                this.logger.log('Publisher WebSocket closed');
                this.stop();
            };

        } catch (error) {
            this.logger.error(`Failed to start publisher: ${error.message}`);
            throw error;
        }
    }

    async handleMessage(msg) {
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
        }
    }

    async initPeerConnection(config) {
        this.logger.log('Initializing publisher peer connection');

        this.pc = new RTCPeerConnection({ iceServers: config.iceServers });

        this.stream.getTracks().forEach(track => {
            this.pc.addTrack(track, this.stream);
            this.logger.log(`Added ${track.kind} track to publisher`);
        });

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
            this.logger.log(`Publisher connection state: ${this.pc.connectionState}`);
            if (this.pc.connectionState === 'connected') {
                this.logger.success('Publisher peer connection established!');
            }
        };

        const offer = await this.pc.createOffer();
        await this.pc.setLocalDescription(offer);

        this.ws.send(JSON.stringify({
            event: 'OFFER',
            offer: { type: offer.type, sdp: offer.sdp }
        }));

        this.logger.log('Publisher offer sent');
    }

    async handleAnswer(answer) {
        try {
            await this.pc.setRemoteDescription({ type: answer.type, sdp: answer.sdp });
            this.logger.success('Publisher remote description set');
            this.remoteDescriptionSet = true;

            for (const candidate of this.pendingIceCandidates) {
                await this.pc.addIceCandidate(candidate);
            }
            this.pendingIceCandidates = [];
        } catch (error) {
            this.logger.error(`Failed to set publisher remote description: ${error.message}`);
        }
    }

    async handleServerIce(ice) {
        if (this.pc && ice && ice.candidate) {
            if (this.remoteDescriptionSet) {
                await this.pc.addIceCandidate(ice.candidate);
            } else {
                this.pendingIceCandidates.push(ice.candidate);
            }
        }
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
        this.logger.log('Publisher stopped');
    }
}

class Viewer {
    constructor(peerName, logger, onStatusChange) {
        this.peerName = peerName;
        this.logger = logger;
        this.onStatusChange = onStatusChange;
        this.ws = null;
        this.pc = null;
        this.authenticated = false;
        this.pendingIceCandidates = [];
        this.remoteDescriptionSet = false;
        this.videoElement = null;
        this.statsInterval = null;
    }

    async start(videoElement) {
        try {
            this.videoElement = videoElement;
            this.updateStatus('connecting');
            this.logger.log(`Starting viewer for ${this.peerName}`);

            this.ws = new WebSocket(`${WS_URL}/player`);

            this.ws.onopen = () => {
                this.logger.success(`Viewer WebSocket connected for ${this.peerName}`);
            };

            this.ws.onmessage = async (event) => {
                const msg = JSON.parse(event.data);
                await this.handleMessage(msg);
            };

            this.ws.onerror = (error) => {
                this.logger.error(`Viewer WebSocket error for ${this.peerName}: ${error}`);
                this.updateStatus('error');
            };

            this.ws.onclose = () => {
                this.logger.log(`Viewer WebSocket closed for ${this.peerName}`);
                this.updateStatus('disconnected');
            };

        } catch (error) {
            this.logger.error(`Failed to start viewer for ${this.peerName}: ${error.message}`);
            this.updateStatus('error');
            throw error;
        }
    }

    async handleMessage(msg) {
        switch (msg.event) {
            case 'AUTH_REQUEST':
                this.ws.send(JSON.stringify({
                    event: 'AUTH',
                    playerAuth: { credential: 'test' }
                }));
                break;

            case 'AUTH_FAILED':
                this.logger.error(`Authentication failed for ${this.peerName}`);
                this.updateStatus('error');
                break;

            case 'INIT_PEER':
                this.authenticated = true;
                await this.initPeerConnection(msg.initPeer.pcConfig);
                break;

            case 'ANSWER':
                await this.handleAnswer(msg.offer);
                break;

            case 'SERVER_ICE':
                await this.handleServerIce(msg.ice);
                break;

            case 'OFFER_FAILED':
                this.logger.error(`Offer failed for ${this.peerName} - peer may not exist`);
                this.updateStatus('error');
                break;
        }
    }

    async initPeerConnection(config) {
        this.logger.log(`Initializing peer connection for ${this.peerName}`);

        this.pc = new RTCPeerConnection({ iceServers: config.iceServers });

        this.pc.ontrack = (event) => {
            this.logger.success(`Received ${event.track.kind} track from ${this.peerName}`);
            if (this.videoElement && !this.videoElement.srcObject) {
                this.videoElement.srcObject = event.streams[0];
            }
        };

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
            const state = this.pc.connectionState;
            this.logger.log(`${this.peerName} connection state: ${state}`);

            if (state === 'connected') {
                this.logger.success(`Peer connection established with ${this.peerName}!`);
                this.updateStatus('connected');
                this.startStats();
            } else if (state === 'failed' || state === 'closed') {
                this.updateStatus('error');
            } else if (state === 'disconnected') {
                this.updateStatus('disconnected');
            }
        };

        this.pc.addTransceiver('video', { direction: 'recvonly' });
        this.pc.addTransceiver('audio', { direction: 'recvonly' });

        const offer = await this.pc.createOffer();
        await this.pc.setLocalDescription(offer);

        this.ws.send(JSON.stringify({
            event: 'OFFER',
            offer: {
                type: offer.type,
                sdp: offer.sdp,
                peerName: this.peerName
            }
        }));

        this.logger.log(`Offer sent for ${this.peerName}`);
    }

    async handleAnswer(answer) {
        try {
            await this.pc.setRemoteDescription({ type: answer.type, sdp: answer.sdp });
            this.logger.success(`Remote description set for ${this.peerName}`);
            this.remoteDescriptionSet = true;

            for (const candidate of this.pendingIceCandidates) {
                await this.pc.addIceCandidate(candidate);
            }
            this.pendingIceCandidates = [];
        } catch (error) {
            this.logger.error(`Failed to set remote description for ${this.peerName}: ${error.message}`);
            this.updateStatus('error');
        }
    }

    async handleServerIce(ice) {
        if (this.pc && ice && ice.candidate) {
            if (this.remoteDescriptionSet) {
                await this.pc.addIceCandidate(ice.candidate);
            } else {
                this.pendingIceCandidates.push(ice.candidate);
            }
        }
    }

    startStats() {
        if (this.statsInterval) {
            clearInterval(this.statsInterval);
        }

        this.statsInterval = setInterval(async () => {
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

            this.onStatusChange(this.peerName, 'stats', { bitrate: Math.round(bitrate), packets });
        }, 1000);
    }

    updateStatus(status) {
        this.onStatusChange(this.peerName, 'status', status);
    }

    stop() {
        if (this.statsInterval) {
            clearInterval(this.statsInterval);
            this.statsInterval = null;
        }
        if (this.pc) {
            this.pc.close();
            this.pc = null;
        }
        if (this.ws) {
            this.ws.close();
            this.ws = null;
        }
        if (this.videoElement) {
            this.videoElement.srcObject = null;
        }
        this.logger.log(`Viewer stopped for ${this.peerName}`);
    }
}

// UI Controller
class UIController {
    constructor() {
        this.logger = new Logger(document.getElementById('logs'));
        this.publisher = null;
        this.viewers = new Map();
        this.currentPage = 'dashboard';
        this.setupEventListeners();
        this.setupNavigation();
    }

    setupNavigation() {
        // Mobile menu toggle
        const mobileMenuBtn = document.getElementById('mobileMenuBtn');
        const sidebar = document.getElementById('sidebar');
        const sidebarOverlay = document.getElementById('sidebarOverlay');

        const toggleMobileMenu = () => {
            sidebar.classList.toggle('mobile-open');
            sidebarOverlay.classList.toggle('active');
        };

        if (mobileMenuBtn) {
            mobileMenuBtn.addEventListener('click', toggleMobileMenu);
        }

        if (sidebarOverlay) {
            sidebarOverlay.addEventListener('click', toggleMobileMenu);
        }

        // Navigation items
        const navItems = document.querySelectorAll('.nav-item');
        navItems.forEach(item => {
            item.addEventListener('click', (e) => {
                e.preventDefault();
                const pageText = item.querySelector('span:last-child').textContent.toLowerCase();

                // Map navigation text to page IDs
                const pageMap = {
                    'dashboard': 'dashboard',
                    'streams': 'streams',
                    'peers': 'peers',
                    'configuration': 'configuration',
                    'analytics': 'analytics'
                };

                const pageId = pageMap[pageText];
                if (pageId) {
                    this.switchPage(pageId);
                }

                // Update active state
                navItems.forEach(n => n.classList.remove('active'));
                item.classList.add('active');

                // Close mobile menu after selection
                if (window.innerWidth <= 768) {
                    sidebar.classList.remove('mobile-open');
                    sidebarOverlay.classList.remove('active');
                }
            });
        });
    }

    switchPage(pageId) {
        this.currentPage = pageId;

        // Hide all page sections
        const sections = document.querySelectorAll('.page-section');
        sections.forEach(section => section.classList.remove('active'));

        // Show the selected page
        const targetPage = document.getElementById(`page-${pageId}`);
        if (targetPage) {
            targetPage.classList.add('active');
        }

        // Update the title bar
        const pageTitle = document.querySelector('.page-title');
        const pageSubtitle = document.querySelector('.page-subtitle');

        const titles = {
            'dashboard': { title: 'Stream Dashboard', subtitle: 'Monitor and manage your WebRTC streams' },
            'streams': { title: 'Active Streams', subtitle: 'View all active video streams' },
            'peers': { title: 'Connected Peers', subtitle: 'Manage connected peers and publishers' },
            'configuration': { title: 'Configuration', subtitle: 'System settings and preferences' },
            'analytics': { title: 'Analytics', subtitle: 'Performance metrics and statistics' }
        };

        if (titles[pageId]) {
            pageTitle.textContent = titles[pageId].title;
            pageSubtitle.textContent = titles[pageId].subtitle;
        }

        // Page-specific actions
        if (pageId === 'peers') {
            this.loadPeersTable();
        } else if (pageId === 'analytics') {
            this.updateAnalyticsStats();
        }
    }

    setupEventListeners() {
        document.getElementById('startPublish').addEventListener('click', () => this.startPublishing());
        document.getElementById('stopPublish').addEventListener('click', () => this.stopPublishing());
        document.getElementById('watchAll').addEventListener('click', () => this.watchAll());
        document.getElementById('stopAll').addEventListener('click', () => this.stopAll());
        document.getElementById('refreshPeers').addEventListener('click', () => this.refreshPeers());

        // Peers table refresh button
        const refreshPeersTableBtn = document.getElementById('refreshPeersTable');
        if (refreshPeersTableBtn) {
            refreshPeersTableBtn.addEventListener('click', () => this.loadPeersTable());
        }

        // Auto-refresh peers every 5 seconds
        setInterval(() => {
            this.refreshPeers(true);
            this.fetchPeerCount();
        }, 5000);

        // Initial stats update
        this.fetchPeerCount();
        this.updateGlobalStats();

        this.logger.log('WebRTC SFU Admin Panel initialized');
        this.logger.log(`Server: ${WS_URL}`);
    }

    async startPublishing() {
        const peerName = document.getElementById('publisherName').value || 'user';
        const sourceType = document.getElementById('sourceType').value;

        // Format: {peerName}-{kind}
        const name = `${peerName}-${sourceType}`;

        try {
            this.publisher = new Publisher(this.logger);
            await this.publisher.start(name, sourceType);

            document.getElementById('startPublish').disabled = true;
            document.getElementById('stopPublish').disabled = false;
            this.logger.success(`Publishing as ${name}`);
        } catch (error) {
            this.logger.error(`Failed to start publishing: ${error.message}`);
        }
    }

    stopPublishing() {
        if (this.publisher) {
            this.publisher.stop();
            this.publisher = null;
        }
        document.getElementById('startPublish').disabled = false;
        document.getElementById('stopPublish').disabled = true;
    }

    async watchAll() {
        const watchPeersInput = document.getElementById('watchPeers').value.trim();
        let peers = [];

        if (watchPeersInput) {
            // Use user-specified peers
            peers = watchPeersInput.split(',').map(p => p.trim()).filter(p => p);
            this.logger.log(`Watching specified peers: ${peers.join(', ')}`);
        } else {
            // Fetch all available peers
            try {
                const response = await fetch('/api/peers');
                const data = await response.json();
                peers = data.peers.map(p => p.name);
                this.logger.log(`Watching all available peers: ${peers.join(', ')}`);
            } catch (error) {
                this.logger.error(`Failed to fetch peers: ${error.message}`);
                return;
            }
        }

        if (peers.length === 0) {
            this.logger.warning('No peers to watch');
            return;
        }

        // Stop existing viewers not in the new list
        for (const [peerName, viewer] of this.viewers.entries()) {
            if (!peers.includes(peerName)) {
                viewer.stop();
                this.viewers.delete(peerName);
                this.removeVideoCard(peerName);
            }
        }

        // Start viewers for new peers
        for (const peerName of peers) {
            if (!this.viewers.has(peerName)) {
                this.addVideoCard(peerName);
                const videoElement = document.getElementById(`video-${peerName}`);
                const viewer = new Viewer(peerName, this.logger, (name, type, data) => this.handleViewerUpdate(name, type, data));
                this.viewers.set(peerName, viewer);
                await viewer.start(videoElement);
            }
        }
    }

    stopAll() {
        for (const [peerName, viewer] of this.viewers.entries()) {
            viewer.stop();
            this.removeVideoCard(peerName);
        }
        this.viewers.clear();
        this.logger.log('Stopped all viewers');
    }

    async refreshPeers(silent = false) {
        try {
            if (!silent) this.logger.log('Fetching peer list...');

            const response = await fetch('/api/peers');
            const data = await response.json();

            if (!silent) {
                this.logger.success(`Found ${data.peers.length} peer(s): ${data.peers.map(p => p.name).join(', ')}`);
            }
        } catch (error) {
            if (!silent) {
                this.logger.error(`Failed to fetch peers: ${error.message}`);
            }
        }
    }

    addVideoCard(peerName) {
        const grid = document.getElementById('videoGrid');

        // Remove empty state if exists
        const emptyState = grid.querySelector('.empty-state');
        if (emptyState) {
            emptyState.remove();
        }

        const card = document.createElement('div');
        card.className = 'video-card';
        card.id = `card-${peerName}`;
        card.innerHTML = `
            <div class="video-header">
                <div class="video-title">🎥 ${peerName}</div>
                <div class="status-badge connecting" id="status-${peerName}">
                    <div class="status-dot"></div>
                    Connecting
                </div>
            </div>
            <div class="video-container">
                <video id="video-${peerName}" autoplay playsinline controls></video>
            </div>
            <div class="video-stats">
                <div class="video-stat">
                    <div class="video-stat-value" id="bitrate-${peerName}">0</div>
                    <div class="video-stat-label">kbps</div>
                </div>
                <div class="video-stat">
                    <div class="video-stat-value" id="packets-${peerName}">0</div>
                    <div class="video-stat-label">Packets</div>
                </div>
            </div>
            <div class="video-controls">
                <button class="btn btn-danger" onclick="uiController.stopViewer('${peerName}')">
                    <span>⏹️</span>
                    <span>Disconnect</span>
                </button>
            </div>
        `;
        grid.appendChild(card);

        // Update global stats
        this.updateGlobalStats();
    }

    removeVideoCard(peerName) {
        const card = document.getElementById(`card-${peerName}`);
        if (card) {
            card.remove();
        }

        // Add empty state if no cards left
        const grid = document.getElementById('videoGrid');
        if (grid.children.length === 0) {
            grid.innerHTML = `
                <div class="empty-state">
                    <div class="empty-state-icon">📺</div>
                    <h3>No Active Streams</h3>
                    <p>Click "Watch All" to start viewing available streams</p>
                </div>
            `;
        }

        // Update global stats
        this.updateGlobalStats();
    }

    handleViewerUpdate(peerName, type, data) {
        if (type === 'status') {
            const statusEl = document.getElementById(`status-${peerName}`);
            if (statusEl) {
                const statusText = data.charAt(0).toUpperCase() + data.slice(1);
                statusEl.className = `status-badge ${data}`;
                statusEl.innerHTML = `<div class="status-dot"></div>${statusText}`;
            }
        } else if (type === 'stats') {
            const bitrateEl = document.getElementById(`bitrate-${peerName}`);
            const packetsEl = document.getElementById(`packets-${peerName}`);
            if (bitrateEl) bitrateEl.textContent = data.bitrate;
            if (packetsEl) packetsEl.textContent = data.packets;

            // Update global stats
            this.updateGlobalStats();
        }
    }

    updateGlobalStats() {
        // Update total streams
        document.getElementById('totalStreams').textContent = this.viewers.size;

        // Calculate total bitrate
        let totalBitrate = 0;
        for (const peerName of this.viewers.keys()) {
            const bitrateEl = document.getElementById(`bitrate-${peerName}`);
            if (bitrateEl) {
                totalBitrate += parseInt(bitrateEl.textContent) || 0;
            }
        }
        document.getElementById('totalBitrate').textContent = totalBitrate;
    }

    async fetchPeerCount() {
        try {
            const response = await fetch('/api/peers');
            const data = await response.json();
            document.getElementById('totalPeers').textContent = data.peers.length;
        } catch (error) {
            // Silently fail
        }
    }

    stopViewer(peerName) {
        const viewer = this.viewers.get(peerName);
        if (viewer) {
            viewer.stop();
            this.viewers.delete(peerName);
            this.removeVideoCard(peerName);
            this.logger.log(`Stopped viewer for ${peerName}`);
        }
    }

    async loadPeersTable() {
        try {
            const response = await fetch('/api/peers');
            const data = await response.json();
            const tbody = document.getElementById('peersTableBody');

            if (data.peers.length === 0) {
                tbody.innerHTML = `
                    <tr>
                        <td colspan="5" style="text-align: center; padding: 40px; color: var(--text-muted);">
                            No peers connected
                        </td>
                    </tr>
                `;
                return;
            }

            tbody.innerHTML = data.peers.map(peer => `
                <tr>
                    <td><strong>${peer.name}</strong></td>
                    <td>
                        <span class="peer-badge publisher">Publisher</span>
                    </td>
                    <td>${peer.tracks.length}</td>
                    <td>
                        <span class="status-badge connected">
                            <div class="status-dot"></div>
                            Connected
                        </span>
                    </td>
                    <td>
                        <button class="btn btn-primary" style="padding: 6px 12px; font-size: 12px;" onclick="uiController.watchPeer('${peer.name}')">
                            <span>👁️</span>
                            <span>Watch</span>
                        </button>
                    </td>
                </tr>
            `).join('');
        } catch (error) {
            this.logger.error(`Failed to load peers table: ${error.message}`);
        }
    }

    async watchPeer(peerName) {
        // Switch to streams page
        this.switchPage('streams');

        // Navigate to streams in the menu
        const navItems = document.querySelectorAll('.nav-item');
        navItems.forEach(item => {
            const text = item.querySelector('span:last-child').textContent.toLowerCase();
            if (text === 'streams') {
                navItems.forEach(n => n.classList.remove('active'));
                item.classList.add('active');
            }
        });

        // Watch the specific peer
        if (!this.viewers.has(peerName)) {
            this.addVideoCard(peerName);
            const videoElement = document.getElementById(`video-${peerName}`);
            const viewer = new Viewer(peerName, this.logger, (name, type, data) => this.handleViewerUpdate(name, type, data));
            this.viewers.set(peerName, viewer);
            await viewer.start(videoElement);
        }
    }

    updateAnalyticsStats() {
        // Update analytics stats with current data
        document.getElementById('analyticsStreams').textContent = this.viewers.size;
        document.getElementById('analyticsPeaks').textContent = this.viewers.size;

        let totalBitrate = 0;
        for (const peerName of this.viewers.keys()) {
            const bitrateEl = document.getElementById(`bitrate-${peerName}`);
            if (bitrateEl) {
                totalBitrate += parseInt(bitrateEl.textContent) || 0;
            }
        }
        const avgBitrate = this.viewers.size > 0 ? Math.round(totalBitrate / this.viewers.size) : 0;
        document.getElementById('analyticsAvgBitrate').textContent = avgBitrate;

        // Uptime (placeholder)
        document.getElementById('analyticsUptime').textContent = '24h';
    }
}

// Initialize
const uiController = new UIController();
