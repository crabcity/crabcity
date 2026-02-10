    let currentSocket = null;
    let currentTerminal = null;
    let fitAddon = null;
    let currentInstanceId = null;
    let currentInstancePort = null;
    let instances = new Map();
    let instanceCommands = new Map(); // Store command for each instance
    let terminalBuffers = new Map(); // Store recent raw terminal output per instance in memory
    let terminalStates = new Map(); // Store terminal state per instance
    let conversationData = new Map(); // Store parsed conversation data per instance
    let drawerOpen = false; // Track drawer state
    let drawerWidth = localStorage.getItem('drawer_width') || 400; // Saved drawer width
    let conversationTurns = []; // Current conversation turns
    let currentWs = null; // Current WebSocket connection

    // Handle WebSocket message - unified handler for all connections
    function handleWsMessage(msg, instanceId) {
        switch (msg.type) {
            case 'Output':
                if (currentTerminal) {
                    currentTerminal.write(msg.data);
                    accumulateOutput(instanceId, msg.data);
                }
                break;

            case 'ConversationFull':
                // Full conversation received - replace all turns
                conversationTurns = msg.turns || [];
                if (drawerOpen) {
                    displayConversation(conversationTurns, conversationTurns.length > 0);
                }
                break;

            case 'ConversationUpdate':
                // Incremental update - append new turns
                if (msg.turns && msg.turns.length > 0) {
                    conversationTurns.push(...msg.turns);
                    if (drawerOpen) {
                        displayConversation(conversationTurns, true);
                    }
                }
                break;

            case 'SessionAmbiguous':
                // Multiple sessions found - show picker dialog
                showSessionPicker(msg.candidates);
                break;
        }
    }

    // Show a dialog for the user to pick which session belongs to this instance
    function showSessionPicker(candidates) {
        // Create modal overlay
        const overlay = document.createElement('div');
        overlay.className = 'fixed inset-0 bg-black/50 flex items-center justify-center z-50';
        overlay.id = 'session-picker-overlay';

        const modal = document.createElement('div');
        modal.className = 'bg-gray-800 rounded-lg p-6 max-w-lg w-full mx-4 shadow-xl';

        let html = `
            <h3 class="text-lg font-semibold text-white mb-4">Select Session</h3>
            <p class="text-gray-400 text-sm mb-4">Multiple conversations were found. Please select the one that belongs to this instance:</p>
            <div class="space-y-2 max-h-64 overflow-y-auto">
        `;

        for (const candidate of candidates) {
            const startTime = candidate.started_at
                ? new Date(candidate.started_at).toLocaleString()
                : 'Unknown';
            const preview = candidate.preview
                ? candidate.preview.substring(0, 80) + (candidate.preview.length > 80 ? '...' : '')
                : 'No messages yet';

            html += `
                <button
                    class="w-full text-left p-3 rounded bg-gray-700 hover:bg-gray-600 transition-colors"
                    onclick="selectSession('${candidate.session_id}')"
                >
                    <div class="text-white text-sm font-medium">${startTime}</div>
                    <div class="text-gray-400 text-xs mt-1">${candidate.message_count} messages</div>
                    <div class="text-gray-500 text-xs mt-1 truncate">${preview}</div>
                </button>
            `;
        }

        html += '</div>';
        modal.innerHTML = html;
        overlay.appendChild(modal);
        document.body.appendChild(overlay);
    }

    // Called when user selects a session
    window.selectSession = function(sessionId) {
        // Remove the picker overlay
        const overlay = document.getElementById('session-picker-overlay');
        if (overlay) overlay.remove();

        // Send selection to server
        if (currentWs && currentWs.readyState === WebSocket.OPEN) {
            currentWs.send(JSON.stringify({ type: 'SessionSelect', session_id: sessionId }));
        }
    };
    let defaultCommand = 'claude'; // Default command for new instances
    const MEMORY_CHUNK_SIZE = 100 * 1024; // 100KB chunks in memory
    const STORAGE_CHUNK_SIZE = 500 * 1024; // 500KB chunks to localStorage

    // Save terminal state to memory (not localStorage due to size)
    function saveTerminalState(instanceId) {
        if (!currentTerminal || !instanceId) return;

        // Store raw output accumulation in memory
        if (!terminalBuffers.has(instanceId)) {
            terminalBuffers.set(instanceId, []);
        }

        // Store terminal dimensions and cursor info
        terminalStates.set(instanceId, {
            cols: currentTerminal.cols,
            rows: currentTerminal.rows,
            scrollback: currentTerminal.buffer.active.baseY,
            cursorX: currentTerminal.buffer.active.cursorX,
            cursorY: currentTerminal.buffer.active.cursorY,
            timestamp: Date.now()
        });
    }

    // Get or initialize buffer for instance
    function getOrInitBuffer(instanceId) {
        if (!terminalBuffers.has(instanceId)) {
            // Check if we have metadata in localStorage to know the chunk count
            let chunkCount = 0;
            try {
                const metaKey = `terminal_meta_${instanceId}`;
                const metaStr = localStorage.getItem(metaKey);
                if (metaStr) {
                    const metadata = JSON.parse(metaStr);
                    chunkCount = metadata.chunkCount || 0;
                }
            } catch (e) {
                // Ignore
            }

            terminalBuffers.set(instanceId, {
                memory: [],      // Recent data in memory
                memorySize: 0,   // Track memory buffer size
                chunkCount: chunkCount    // Track number of chunks in localStorage
            });
        }
        return terminalBuffers.get(instanceId);
    }

    // Accumulate raw output data with infinite scrollback
    function accumulateOutput(instanceId, data) {
        if (!instanceId || !data) return;

        const buffer = getOrInitBuffer(instanceId);
        buffer.memory.push(data);
        buffer.memorySize += data.length;

        // When memory buffer exceeds threshold, offload to localStorage
        if (buffer.memorySize > MEMORY_CHUNK_SIZE) {
            offloadToStorage(instanceId);
        }
    }

    // Offload older data to localStorage
    function offloadToStorage(instanceId) {
        const buffer = getOrInitBuffer(instanceId);
        if (!buffer || buffer.memory.length === 0) return;

        // Concatenate memory buffer into a chunk
        const chunk = buffer.memory.join('');

        // Store chunk in localStorage
        try {
            const chunkKey = `terminal_chunk_${instanceId}_${buffer.chunkCount}`;
            localStorage.setItem(chunkKey, chunk);

            // Update metadata
            const metaKey = `terminal_meta_${instanceId}`;
            const metadata = {
                chunkCount: buffer.chunkCount + 1,
                lastUpdate: Date.now()
            };
            localStorage.setItem(metaKey, JSON.stringify(metadata));

            // Clear memory buffer and increment chunk counter
            buffer.memory = [];
            buffer.memorySize = 0;
            buffer.chunkCount++;

        } catch (e) {
            console.warn('Failed to offload to localStorage:', e);
            // If localStorage is full, keep most recent data in memory
            // and drop oldest chunks from localStorage
            tryCleanupOldestChunk(instanceId);
        }
    }

    // Try to free up space by removing oldest chunk
    function tryCleanupOldestChunk(instanceId) {
        try {
            // Find and remove the oldest chunk for this instance
            for (let i = 0; i < localStorage.length; i++) {
                const key = localStorage.key(i);
                if (key && key.startsWith(`terminal_chunk_${instanceId}_`)) {
                    localStorage.removeItem(key);
                    console.log('Removed oldest chunk to free space:', key);
                    // Try offloading again
                    offloadToStorage(instanceId);
                    break;
                }
            }
        } catch (e) {
            console.error('Failed to cleanup old chunks:', e);
        }
    }

    // Restore terminal by replaying all output from storage and memory
    function restoreTerminalState(instanceId) {
        if (!currentTerminal || !instanceId) return;

        // Clear terminal first
        currentTerminal.clear();

        // First, load and replay all chunks from localStorage
        const metaKey = `terminal_meta_${instanceId}`;
        try {
            const metaStr = localStorage.getItem(metaKey);
            if (metaStr) {
                const metadata = JSON.parse(metaStr);

                // Load all chunks in order
                for (let i = 0; i < metadata.chunkCount; i++) {
                    const chunkKey = `terminal_chunk_${instanceId}_${i}`;
                    const chunk = localStorage.getItem(chunkKey);
                    if (chunk) {
                        currentTerminal.write(chunk);
                    }
                }
            }
        } catch (e) {
            console.warn('Failed to restore from localStorage:', e);
        }

        // Then replay any data still in memory
        const buffer = getOrInitBuffer(instanceId);
        if (buffer && buffer.memory && buffer.memory.length > 0) {
            for (const data of buffer.memory) {
                currentTerminal.write(data);
            }
        }

        // Restore terminal dimensions if they changed
        const state = terminalStates.get(instanceId);
        if (state && fitAddon) {
            fitAddon.fit();
        }
    }


    async function refreshInstances() {
        try {
            const response = await fetch('/api/instances');
            const data = await response.json();

            // Update instances map
            instances.clear();
            data.forEach(inst => {
                instances.set(inst.id, inst);
            });

            // Update sidebar - use correct ID and HTML structure
            const list = document.getElementById('instance-list');
            if (!list) {
                console.error('Instance list element not found');
                return;
            }
            list.innerHTML = '';

            data.forEach(instance => {
                const card = document.createElement('div');
                card.className = 'instance-card bg-gray-700 rounded p-3 cursor-pointer hover:bg-gray-600 transition-colors';
                if (instance.id === currentInstanceId) {
                    card.classList.add('bg-gray-600', 'ring-2', 'ring-crab-accent');
                }
                card.dataset.instanceId = instance.id;
                card.dataset.port = instance.wrapper_port;

                // Show first 8 chars of ID or the name
                const displayName = instance.name || instance.id.substring(0, 8) + '...';

                card.innerHTML = `
                    <div class="flex items-center justify-between mb-1">
                        <span class="font-medium">${displayName}</span>
                        <span class="${instance.running ? 'text-green-500' : 'text-gray-500'}">
                            ${instance.running ? '●' : '○'}
                        </span>
                    </div>
                    <div class="text-xs text-gray-400">
                        ${instance.command}
                    </div>
                    <button class="delete-btn mt-2 text-xs text-red-400 hover:text-red-300" data-instance-id="${instance.id}">
                        🗑️ Delete
                    </button>
                `;
                list.appendChild(card);
            });

            attachTabListeners();
        } catch (error) {
            console.error('Failed to refresh instances:', error);
        }
    }

    function attachTabListeners() {
        // Card click to connect - use instance-card class
        document.querySelectorAll('.instance-card').forEach(card => {
            card.addEventListener('click', (e) => {
                // Don't connect if clicking on delete button
                if (e.target.classList.contains('delete-btn')) {
                    return;
                }

                const id = card.dataset.instanceId;
                const port = card.dataset.port;
                connectToInstance(id, port);

                // Update active state
                document.querySelectorAll('.instance-card').forEach(c => {
                    c.classList.remove('bg-gray-600', 'ring-2', 'ring-crab-accent');
                    c.classList.add('bg-gray-700');
                });
                card.classList.remove('bg-gray-700');
                card.classList.add('bg-gray-600', 'ring-2', 'ring-crab-accent');
            });
        });

        // Delete buttons - use data-instance-id
        document.querySelectorAll('.delete-btn').forEach(btn => {
            btn.addEventListener('click', async (e) => {
                e.stopPropagation();
                const id = btn.dataset.instanceId;
                await deleteInstance(id);
            });
        });
    }

    // Navigation between views
    function switchView(viewName) {
        // Hide all views
        document.querySelectorAll('.view').forEach(v => {
            v.classList.add('hidden');
        });

        // Show selected view
        const view = document.getElementById(`${viewName}-view`);
        if (view) {
            view.classList.remove('hidden');
        }

        // Update nav tabs
        document.querySelectorAll('.nav-tab').forEach(t => {
            t.classList.remove('bg-crab-accent', 'text-white', 'border-crab-accent');
            t.classList.add('bg-transparent', 'text-gray-400', 'border-crab-blue');
        });
        const activeTab = document.getElementById(`${viewName}-tab`);
        if (activeTab) {
            activeTab.classList.remove('bg-transparent', 'text-gray-400', 'border-crab-blue');
            activeTab.classList.add('bg-crab-accent', 'text-white', 'border-crab-accent');
        }

        // Update stats if switching to settings
        if (viewName === 'settings') {
            updateSettingsStats();
        }
    }

    // Update settings page statistics
    function updateSettingsStats() {
        const totalInstances = instances.size;
        let activeInstances = 0;
        let storageUsed = 0;

        for (const instance of instances.values()) {
            if (instance.running) activeInstances++;
        }

        // Calculate storage used
        try {
            for (let i = 0; i < localStorage.length; i++) {
                const key = localStorage.key(i);
                if (key && key.startsWith('terminal_')) {
                    const value = localStorage.getItem(key);
                    storageUsed += value ? value.length : 0;
                }
            }
        } catch (e) {}

        document.getElementById('total-instances').textContent = totalInstances;
        document.getElementById('active-instances').textContent = activeInstances;
        document.getElementById('storage-used').textContent = formatBytes(storageUsed);

        // Set default command value
        document.getElementById('default-command-input').value = defaultCommand;

        // Set working directory
        document.getElementById('working-dir-input').value = window.location.pathname.split('/').slice(0, -1).join('/') || '/';
    }

    function formatBytes(bytes) {
        if (bytes === 0) return '0 B';
        const k = 1024;
        const sizes = ['B', 'KB', 'MB', 'GB'];
        const i = Math.floor(Math.log(bytes) / Math.log(k));
        return Math.round(bytes / Math.pow(k, i) * 100) / 100 + ' ' + sizes[i];
    }

    async function createInstanceWithSettings(name, command) {
        try {
            const response = await fetch('/api/instances', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    command: command || defaultCommand,
                    name: name || null,
                    working_dir: null
                })
            });

            if (response.ok) {
                const instance = await response.json();
                await refreshInstances();

                // Switch to terminal view and connect
                switchView('terminal');
                connectToInstance(instance.id, instance.wrapper_port);

                // Clear form
                document.getElementById('instance-name-input').value = '';
                document.getElementById('instance-command-input').value = '';
            } else {
                const errorText = await response.text();
                alert(`Failed to create instance: ${errorText}`);
            }
        } catch (error) {
            console.error('Failed to create instance:', error);
            alert(`Failed to create instance: ${error.message}`);
        }
    }

    // Instant create instance - just go!
    async function createInstance() {
        try {
            const response = await fetch('/api/instances', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    command: defaultCommand,
                    name: null,
                    working_dir: null
                })
            });

            if (response.ok) {
                const instance = await response.json();
                instanceCommands.set(instance.id, defaultCommand);
                await refreshInstances();

                // Switch to terminal and connect immediately
                switchView('terminal');
                connectToInstance(instance.id, instance.wrapper_port);
            } else {
                const errorText = await response.text();
                console.error('Failed to create instance:', errorText);
                // Try fallback to bash if claude failed
                if (defaultCommand === 'claude' && errorText.includes('spawn')) {
                    console.log('Claude not found, falling back to bash...');
                    defaultCommand = 'bash';
                    localStorage.setItem('crab_city_default_command', 'bash');
                    createInstance(); // Retry with bash
                }
            }
        } catch (error) {
            console.error('Failed to create instance:', error);
        }
    }

    // Restart instance with new command
    async function restartInstance() {
        if (!currentInstanceId) return;

        const newCommand = document.getElementById('instance-command-edit').value;
        if (!newCommand) return;

        // Save current state
        saveTerminalState(currentInstanceId);
        offloadToStorage(currentInstanceId);

        // Delete current instance
        await fetch(`/api/instances/${currentInstanceId}`, { method: 'DELETE' });

        // Create new instance with same name but new command
        const currentInstance = instances.get(currentInstanceId);
        const instanceName = currentInstance ? currentInstance.name : null;

        try {
            const response = await fetch('/api/instances', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    command: newCommand,
                    name: instanceName,
                    working_dir: null
                })
            });

            if (response.ok) {
                const instance = await response.json();
                instanceCommands.set(instance.id, newCommand);
                await refreshInstances();
                connectToInstance(instance.id, instance.wrapper_port);
            }
        } catch (error) {
            console.error('Failed to restart instance:', error);
        }
    }

    async function deleteInstance(id) {
        try {
            const response = await fetch(`/api/instances/${id}`, {
                method: 'DELETE'
            });

            if (response.ok) {
                if (id === currentInstanceId) {
                    disconnectFromInstance();
                }
                await refreshInstances();
            }
        } catch (error) {
            console.error('Failed to delete instance:', error);
        }
    }

    function initTerminal() {
        if (currentTerminal) {
            currentTerminal.dispose();
            currentTerminal = null;
        }

        currentTerminal = new Terminal({
            cursorBlink: true,
            fontSize: 14,
            fontFamily: "'SF Mono', Monaco, 'Cascadia Code', monospace",
            theme: {
                background: '#000000',
                foreground: '#c0caf5',
                cursor: '#c0caf5',
                cursorAccent: '#000000',
                selection: '#364a82',
            }
        });

        fitAddon = new FitAddon.FitAddon();
        currentTerminal.loadAddon(fitAddon);

        const webLinksAddon = new WebLinksAddon.WebLinksAddon();
        currentTerminal.loadAddon(webLinksAddon);

        const terminalDiv = document.getElementById('terminal');
        terminalDiv.innerHTML = '';
        currentTerminal.open(terminalDiv);
        fitAddon.fit();

        // Handle terminal input
        currentTerminal.onData(data => {
            if (currentSocket && currentSocket.readyState === WebSocket.OPEN) {
                currentSocket.send(JSON.stringify({ type: 'Input', data: data }));
            }
        });

        // Handle resize
        window.addEventListener('resize', () => {
            if (fitAddon) {
                fitAddon.fit();
                sendResize();
            }
        });
    }

    async function connectToInstance(id, port) {
        // Save current terminal state before switching
        if (currentInstanceId && currentInstanceId !== id) {
            saveTerminalState(currentInstanceId);
            // Force offload to storage when switching tabs
            offloadToStorage(currentInstanceId);
        }

        // Disconnect from current instance
        disconnectFromInstance();

        currentInstanceId = id;
        currentInstancePort = port;

        const instance = instances.get(id);
        if (!instance) {
            console.error('Instance not found:', id);
            return;
        }

        // Store the command for this instance
        const instanceCommand = instance.command || instanceCommands.get(id) || defaultCommand;
        instanceCommands.set(id, instanceCommand);

        // Update UI
        document.getElementById('terminal-title').textContent = `${instance.name} (${instanceCommand})`;
        document.getElementById('terminal-info').innerHTML = `
            <span>Port: ${port}</span>
            <span id="connection-status" class="text-yellow-400">🟡 Connecting...</span>
        `;

        // Show control buttons
        document.getElementById('refresh-btn').style.display = 'flex';
        document.getElementById('restart-btn').style.display = 'flex';
        document.getElementById('instance-command-edit').style.display = 'flex';

        // Set command in input (use the instanceCommand we already have)
        document.getElementById('instance-command-edit').value = instanceCommand;

        // Show context button for Claude instances
        if (instanceCommand.includes('claude')) {
            document.getElementById('toggle-drawer-btn').style.display = 'flex';
            // Auto-open drawer for Claude after a delay
            // Conversation will be sent via WebSocket automatically
            setTimeout(() => {
                if (instanceCommand.includes('claude') && !drawerOpen) {
                    toggleDrawer();
                }
            }, 800);
        } else {
            document.getElementById('toggle-drawer-btn').style.display = 'none';
            if (drawerOpen) {
                toggleDrawer(true);
            }
        }

        // Hide empty state, show terminal
        const emptyState = document.getElementById('empty-state');
        const terminalEl = document.getElementById('terminal');
        if (emptyState) {
            emptyState.style.display = 'none';
        }
        if (terminalEl) {
            terminalEl.style.display = 'block';
        }

        // Initialize terminal if needed
        if (!currentTerminal) {
            initTerminal();
        }

        // Try to restore saved state first
        restoreTerminalState(id);

        // Connect to wrapper WebSocket (not the instance WebSocket)
        const wrapperWsUrl = `ws://localhost:${port}/api/ws`;
        const wrapperSocket = new WebSocket(wrapperWsUrl);

        wrapperSocket.onopen = () => {
            console.log('Connected to wrapper at port', port);
            const statusEl = document.getElementById('connection-status');
            if (statusEl) {
                statusEl.className = 'text-green-400';
                statusEl.textContent = '🟢 Connected (direct)';
            }

            // Reset reconnect attempts on successful connection
            reconnectAttempts = 0;

            // Don't fetch output if we restored from buffer - user can refresh if needed
            // fetchWrapperOutput(port);
        };

        wrapperSocket.onmessage = (event) => {
            try {
                const msg = JSON.parse(event.data);
                handleWsMessage(msg, currentInstanceId);
            } catch (e) {
                // Raw data fallback
                if (currentTerminal) {
                    currentTerminal.write(event.data);
                    accumulateOutput(currentInstanceId, event.data);
                }
            }
        };

        wrapperSocket.onerror = (error) => {
            console.error('Wrapper WebSocket error:', error);
            const statusEl = document.getElementById('connection-status');
            if (statusEl) {
                statusEl.className = 'text-orange-400';
                statusEl.textContent = '⚠️ Direct connection error';
            }
        };

        wrapperSocket.onclose = () => {
            console.log('Disconnected from wrapper, trying proxy connection');
            const statusEl = document.getElementById('connection-status');
            if (statusEl) {
                statusEl.className = 'text-yellow-400';
                statusEl.textContent = '🟡 Switching to proxy...';
            }

            // Try to reconnect to our proxy endpoint instead
            connectViaProxy(id);
        };

        currentSocket = wrapperSocket;
    }

    let reconnectAttempts = 0;
    const maxReconnectAttempts = 5;
    let reconnectTimeout = null;

    function connectViaProxy(id, isReconnect = false) {
        // Connect via our WebSocket proxy endpoint
        const wsUrl = `ws://${window.location.host}/api/instances/${id}/ws`;
        const socket = new WebSocket(wsUrl);
        currentWs = socket; // Track current WebSocket for session selection

        socket.onopen = () => {
            console.log('Connected via proxy to instance:', id);
            reconnectAttempts = 0; // Reset on successful connection
            const statusEl = document.getElementById('connection-status');
            if (statusEl) {
                statusEl.className = 'text-green-400';
                statusEl.textContent = '🟢 Connected (proxy)';
            }
        };

        socket.onmessage = (event) => {
            try {
                const msg = JSON.parse(event.data);
                handleWsMessage(msg, id);
            } catch (e) {
                // Raw data fallback
                if (currentTerminal) {
                    currentTerminal.write(event.data);
                    accumulateOutput(id, event.data);
                }
            }
        };

        socket.onerror = (error) => {
            console.error('Proxy WebSocket error:', error);
            const statusEl = document.getElementById('connection-status');
            if (statusEl) {
                statusEl.className = 'text-orange-400';
                statusEl.textContent = '⚠️ Error';
            }
        };

        socket.onclose = () => {
            console.log('Disconnected from proxy');
            const statusEl = document.getElementById('connection-status');

            // Only show disconnected and attempt reconnect if this is the current instance
            if (id === currentInstanceId && reconnectAttempts < maxReconnectAttempts) {
                reconnectAttempts++;
                if (statusEl) {
                    statusEl.className = 'text-yellow-400';
                    statusEl.textContent = `🟡 Reconnecting (${reconnectAttempts}/${maxReconnectAttempts})...`;
                }

                // Clear any existing reconnect timeout
                if (reconnectTimeout) {
                    clearTimeout(reconnectTimeout);
                }

                // Exponential backoff: 1s, 2s, 4s, 8s, 16s
                const delay = Math.min(1000 * Math.pow(2, reconnectAttempts - 1), 16000);
                reconnectTimeout = setTimeout(() => {
                    if (id === currentInstanceId) {
                        console.log(`Attempting to reconnect via proxy (attempt ${reconnectAttempts})`);
                        connectViaProxy(id, true);
                    }
                }, delay);
            } else if (statusEl) {
                statusEl.className = 'text-red-400';
                statusEl.textContent = '🔴 Disconnected';
            }
        };

        currentSocket = socket;
    }

    async function fetchWrapperOutput(port) {
        try {
            const response = await fetch(`http://localhost:${port}/api/output`);
            const data = await response.json();
            if (data.lines && currentTerminal) {
                data.lines.forEach(line => {
                    currentTerminal.writeln(line);
                });
            }
        } catch (error) {
            console.error('Failed to fetch wrapper output:', error);
        }
    }

    // Refresh the terminal by syncing with remote PTY state
    async function refreshTerminal() {
        if (!currentInstanceId || !currentTerminal) return;

        try {
            // Fetch recent output from the instance
            const response = await fetch(`/api/instances/${currentInstanceId}/output`);
            if (response.ok) {
                const data = await response.json();

                // Clear terminal and local buffers
                currentTerminal.clear();
                clearInstanceData(currentInstanceId);

                // Replay the remote output to sync state
                if (data.lines && data.lines.length > 0) {
                    // The first element contains all raw PTY data
                    const rawOutput = data.lines[0];
                    if (rawOutput) {
                        currentTerminal.write(rawOutput);
                        // Also accumulate this in our buffer for consistency
                        accumulateOutput(currentInstanceId, rawOutput);
                    }
                }

                console.log('Terminal refreshed from remote state');
            } else {
                console.error('Failed to fetch remote output');
            }
        } catch (error) {
            console.error('Failed to refresh terminal:', error);
        }
    }

    // Clear all data for an instance
    function clearInstanceData(instanceId) {
        if (!instanceId) return;

        // Clear memory buffer
        terminalBuffers.delete(instanceId);

        // Clear localStorage chunks
        try {
            const metaKey = `terminal_meta_${instanceId}`;
            const metaStr = localStorage.getItem(metaKey);
            if (metaStr) {
                const metadata = JSON.parse(metaStr);
                // Remove all chunks
                for (let i = 0; i < metadata.chunkCount; i++) {
                    localStorage.removeItem(`terminal_chunk_${instanceId}_${i}`);
                }
            }
            localStorage.removeItem(metaKey);
        } catch (e) {
            console.warn('Failed to clear localStorage:', e);
        }

        // Clear state
        terminalStates.delete(instanceId);
    }

    function sendResize() {
        if (!currentTerminal || !currentInstancePort) return;

        const cols = currentTerminal.cols;
        const rows = currentTerminal.rows;

        fetch(`http://localhost:${currentInstancePort}/api/resize`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ rows, cols })
        }).catch(err => console.error('Failed to resize PTY:', err));
    }

    function disconnectFromInstance() {
        // Save state before disconnecting
        if (currentInstanceId) {
            saveTerminalState(currentInstanceId);
            // Force offload to storage when disconnecting
            offloadToStorage(currentInstanceId);
        }

        if (currentSocket) {
            currentSocket.close();
            currentSocket = null;
        }

        // Reset conversation state
        conversationTurns = [];
        conversationInitialized = false;

        // Hide control buttons
        document.getElementById('refresh-btn').style.display = 'none';
        document.getElementById('restart-btn').style.display = 'none';
        document.getElementById('instance-command-edit').style.display = 'none';
        document.getElementById('toggle-drawer-btn').style.display = 'none';
        document.getElementById('conversation-drawer').style.display = 'none';
        drawerOpen = false;

        currentInstanceId = null;
        currentInstancePort = null;
    }

    // Toggle conversation drawer with animation
    function toggleDrawer(forceClose = false) {
        const drawer = document.getElementById('conversation-drawer');
        const isClaudeInstance = instanceCommands.get(currentInstanceId)?.includes('claude');

        if (!isClaudeInstance && !forceClose) {
            // Don't show drawer for non-Claude instances
            drawer.style.display = 'none';
            drawerOpen = false;
            return;
        }

        if (forceClose || drawerOpen) {
            // Close the drawer
            drawer.classList.add('closing');
            setTimeout(() => {
                drawer.style.display = 'none';
                drawer.classList.remove('closing');
                drawerOpen = false;
                // Resize terminal after drawer closes
                if (fitAddon) {
                    fitAddon.fit();
                    sendResize();
                }
            }, 300);
        } else {
            // Open the drawer
            drawerOpen = true;
            drawer.style.display = 'flex';
            drawer.style.width = drawerWidth + 'px';
            // Display current conversation (updates come via WebSocket)
            displayConversation(conversationTurns, conversationTurns.length > 0);
            // Resize terminal immediately after drawer opens
            setTimeout(() => {
                if (fitAddon) {
                    fitAddon.fit();
                    sendResize();
                }
            }, 100);
        }
    }

    // Setup drawer resize functionality
    function setupDrawerResize() {
        const handle = document.getElementById('drawer-resize-handle');
        const drawer = document.getElementById('conversation-drawer');
        const terminalContainer = document.getElementById('terminal-container');
        let isResizing = false;
        let startX = 0;
        let startWidth = 0;

        if (!handle || !drawer) {
            console.error('Drawer resize elements not found');
            return;
        }

        handle.addEventListener('mousedown', (e) => {
            isResizing = true;
            startX = e.clientX;
            startWidth = drawer.offsetWidth;
            handle.classList.add('dragging');
            document.body.style.cursor = 'col-resize';
            e.preventDefault();
        });

        document.addEventListener('mousemove', (e) => {
            if (!isResizing) return;

            const deltaX = startX - e.clientX;
            const containerWidth = terminalContainer ? terminalContainer.parentElement.offsetWidth : window.innerWidth;
            const newWidth = Math.min(Math.max(startWidth + deltaX, 200), containerWidth * 0.6);

            drawer.style.width = newWidth + 'px';
            drawerWidth = newWidth;

            // Resize terminal during drag
            if (fitAddon) {
                fitAddon.fit();
                sendResize();
            }
        });

        document.addEventListener('mouseup', () => {
            if (isResizing) {
                isResizing = false;
                handle.classList.remove('dragging');
                document.body.style.cursor = '';

                // Save the width preference
                localStorage.setItem('drawer_width', drawerWidth);

                // Final resize after drag ends
                if (fitAddon) {
                    fitAddon.fit();
                    sendResize();
                }
            }
        });
    }

    // Display conversation in drawer (notebook style)
    function displayConversation(turns, hasConversation) {
        const drawerContent = document.getElementById('drawer-content');

        if (!turns || turns.length === 0) {
            if (hasConversation === false) {
                drawerContent.innerHTML = `
                    <div class="flex items-center justify-center h-full text-gray-500">
                        <div class="text-center">
                            <div class="text-4xl mb-3 opacity-50">○</div>
                            <p class="text-sm">Waiting for conversation...</p>
                        </div>
                    </div>
                `;
            } else {
                drawerContent.innerHTML = '<div class="flex items-center justify-center h-full text-gray-500"><p>No messages</p></div>';
            }
            return;
        }

        let html = '<div class="notebook">';

        for (const turn of turns.slice(-50)) {
            const isUser = turn.role === 'User';
            const cellType = isUser ? 'user' : 'assistant';
            const typeLabel = isUser ? 'User' : 'Claude';
            const timestamp = formatTimestamp(turn.timestamp);
            const tools = turn.tools || [];

            // Format tool badges
            let toolsHtml = '';
            if (tools.length > 0) {
                const toolBadges = tools.map(t => `<span class="tool-badge">${escapeHtml(t)}</span>`).join('');
                toolsHtml = `<div class="cell-tools">${toolBadges}</div>`;
            }

            // Convert newlines to <br> for display
            const contentHtml = escapeHtml(turn.content).replace(/\n/g, '<br>');

            html += `
                <div class="cell ${isUser ? 'user-cell' : 'assistant-cell'}">
                    <div class="cell-header">
                        <span class="cell-type ${cellType}">${typeLabel}</span>
                        <span class="cell-meta">${timestamp}</span>
                    </div>
                    <div class="cell-content">${contentHtml}</div>
                    ${toolsHtml}
                </div>
            `;
        }

        html += '</div>';
        drawerContent.innerHTML = html;

        // Auto-scroll to bottom
        drawerContent.scrollTop = drawerContent.scrollHeight;
    }

    function escapeHtml(text) {
        const div = document.createElement('div');
        div.textContent = text;
        return div.innerHTML;
    }

    function formatTimestamp(isoString) {
        if (!isoString) return '';
        try {
            const date = new Date(isoString);
            const now = new Date();
            const isToday = date.toDateString() === now.toDateString();

            const timeStr = date.toLocaleTimeString('en-US', {
                hour: '2-digit',
                minute: '2-digit',
                second: '2-digit',
                hour12: false
            });

            if (isToday) {
                return timeStr;
            } else {
                const dateStr = date.toLocaleDateString('en-US', {
                    month: 'short',
                    day: 'numeric'
                });
                return `${dateStr} ${timeStr}`;
            }
        } catch (e) {
            return '';
        }
    }

    // Initialize
    document.addEventListener('DOMContentLoaded', () => {
        // Terminal controls - add null checks for all elements
        const newInstanceBtn = document.getElementById('new-instance-btn');
        if (newInstanceBtn) {
            newInstanceBtn.addEventListener('click', createInstance);
        }

        const refreshBtn = document.getElementById('refresh-btn');
        if (refreshBtn) {
            refreshBtn.addEventListener('click', refreshTerminal);
        }

        const restartBtn = document.getElementById('restart-btn');
        if (restartBtn) {
            restartBtn.addEventListener('click', restartInstance);
        }

        const toggleDrawerBtn = document.getElementById('toggle-drawer-btn');
        if (toggleDrawerBtn) {
            toggleDrawerBtn.addEventListener('click', () => toggleDrawer());
        }

        // Drawer controls
        const drawerCloseBtn = document.getElementById('drawer-close-btn');
        if (drawerCloseBtn) {
            drawerCloseBtn.addEventListener('click', () => toggleDrawer(true));
        }

        setupDrawerResize();

        // Handle Enter key in command input
        const commandEdit = document.getElementById('instance-command-edit');
        if (commandEdit) {
            commandEdit.addEventListener('keypress', (e) => {
                if (e.key === 'Enter') {
                    restartInstance();
                }
            });
        }


        // Load saved settings
        const savedCommand = localStorage.getItem('crab_city_default_command');
        if (savedCommand) {
            defaultCommand = savedCommand;
        }

        // Attach listeners to initially rendered instances
        attachTabListeners();

        refreshInstances();

        // Auto-refresh every 10 seconds
        setInterval(refreshInstances, 10000);

        // Save to storage when page is about to unload
        window.addEventListener('beforeunload', () => {
            if (currentInstanceId) {
                offloadToStorage(currentInstanceId);
            }
        });
    });
