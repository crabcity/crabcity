<script lang="ts">
	import { onMount, onDestroy, tick } from 'svelte';
	import { get } from 'svelte/store';
	import { sendInput, sendResize, sendTerminalVisible, sendTerminalHidden, hasPendingInput, connectionStatus, requestTerminalLock, releaseTerminalLock, instancePresence } from '$lib/stores/websocket';
	import { currentTerminalHasOutput, consumeTerminalOutput } from '$lib/stores/terminal';
	import { currentInstanceId } from '$lib/stores/instances';
	import { currentTerminalLock, iHoldLock, isLockedByOther } from '$lib/stores/terminalLock';
	import { theme } from '$lib/stores/settings';

	const phosphorTheme = {
		background: '#0a0806',
		foreground: '#fdba74',
		cursor: '#fb923c',
		cursorAccent: '#0a0806',
		selectionBackground: 'rgba(251, 146, 60, 0.3)',
		black: '#15110d',
		red: '#ef4444',
		green: '#22c55e',
		yellow: '#fbbf24',
		blue: '#60a5fa',
		magenta: '#a78bfa',
		cyan: '#22d3ee',
		white: '#fdba74'
	};

	const analogTheme = {
		background: '#faf7f0',
		foreground: '#1a1714',
		cursor: '#6e3b1a',
		cursorAccent: '#faf7f0',
		selectionBackground: 'rgba(220, 190, 100, 0.3)',
		black: '#1a1714',
		red: '#943030',
		green: '#2a6e3a',
		yellow: '#7a6520',
		blue: '#2e4a6e',
		magenta: '#5a3060',
		cyan: '#2a5a5a',
		white: '#faf7f0'
	};

	let terminalEl: HTMLDivElement;
	let terminal: import('@xterm/xterm').Terminal | null = null;
	let fitAddon: import('@xterm/addon-fit').FitAddon | null = null;
	let resizeObserver: ResizeObserver | null = null;
	let outputUnsubscribe: (() => void) | null = null;
	let isReady = $state(false);
	let error = $state<string | null>(null);

	// Derived state for showing status banner
	let isDisconnected = $derived($connectionStatus === 'disconnected' || $connectionStatus === 'error');
	let isReconnecting = $derived($connectionStatus === 'connecting' || $connectionStatus === 'reconnecting');
	let showStatusBanner = $derived(isDisconnected || isReconnecting || $hasPendingInput);

	// Multi-user lock state
	let presence = $derived($currentInstanceId ? $instancePresence.get($currentInstanceId) ?? [] : []);
	let isMultiUser = $derived(presence.length > 1);
	let showLockBanner = $derived($isLockedByOther || $iHoldLock);

	// Set up output subscription after terminal is ready
	function setupOutputSubscription() {
		if (outputUnsubscribe) return;

		// Subscribe to the derived store that signals when output is available
		outputUnsubscribe = currentTerminalHasOutput.subscribe((hasOutput) => {
			if (!hasOutput || !terminal) return;

			const instanceId = get(currentInstanceId);
			if (!instanceId) return;

			const buffer = consumeTerminalOutput(instanceId);

			if (buffer.shouldClear) {
				terminal.clear();
			}

			// Check viewport position BEFORE writing so writes don't change the answer
			const wasAtBottom = isAtBottom();

			for (const chunk of buffer.chunks) {
				terminal.write(chunk);
			}

			// Auto-scroll only if the viewport was already at the bottom
			if (wasAtBottom) {
				terminal.scrollToBottom();
			}
		});
	}

	// Check if terminal is scrolled to bottom
	function isAtBottom(): boolean {
		if (!terminal) return true;
		const viewport = terminal.element?.querySelector('.xterm-viewport');
		if (!viewport) return true;
		const { scrollTop, scrollHeight, clientHeight } = viewport;
		// Consider "at bottom" if within 5px
		return scrollHeight - scrollTop - clientHeight < 5;
	}

	onMount(() => {
		initTerminal();
	});

	async function initTerminal() {
		try {
			// Wait for DOM to be ready - retry a few times as bind:this is async in Svelte 5
			let attempts = 0;
			while (!terminalEl && attempts < 10) {
				await tick();
				await new Promise((resolve) => setTimeout(resolve, 50));
				attempts++;
			}

			if (!terminalEl) {
				throw new Error('Terminal container not available after retries');
			}

			const { Terminal } = await import('@xterm/xterm');
			const { FitAddon } = await import('@xterm/addon-fit');
			const { WebLinksAddon } = await import('@xterm/addon-web-links');
			const { ClipboardAddon } = await import('@xterm/addon-clipboard');
			await import('@xterm/xterm/css/xterm.css');

			const currentTheme = get(theme);

			terminal = new Terminal({
				cursorBlink: true,
				fontSize: 13,
				fontFamily: "'JetBrains Mono', 'SF Mono', Monaco, 'Cascadia Code', monospace",
				allowProposedApi: true, // Required for clipboard addon
				theme: currentTheme === 'analog' ? analogTheme : phosphorTheme
			});

			fitAddon = new FitAddon();
			terminal.loadAddon(fitAddon);
			terminal.loadAddon(new WebLinksAddon());
			terminal.loadAddon(new ClipboardAddon());

			terminal.open(terminalEl);

			// Delay fit to ensure container has dimensions
			requestAnimationFrame(() => {
				fitAddon?.fit();
				isReady = true;

				// Now that terminal is ready, set up output subscription
				setupOutputSubscription();

				// Register this terminal in server-side dimension negotiation
				if (terminal) {
					sendTerminalVisible(terminal.rows, terminal.cols);
				}
			});

			terminal.onData((data) => {
				// Terminal lock gating: only allow input when appropriate
				if (isMultiUser) {
					if ($isLockedByOther) {
						// Blocked — another user holds the lock
						return;
					}
					if (!$iHoldLock && !$currentTerminalLock?.holder) {
						// Lock unclaimed — auto-acquire on first keystroke
						requestTerminalLock();
					}
				}
				sendInput(data);
				// Scroll to bottom when user types
				terminal?.scrollToBottom();
			});

			resizeObserver = new ResizeObserver(() => {
				if (fitAddon && terminal && isReady && document.visibilityState === 'visible') {
					fitAddon.fit();
					sendResize(terminal.rows, terminal.cols);
				}
			});
			resizeObserver.observe(terminalEl);

			// Write welcome message
			terminal.writeln('\x1b[90m--- Terminal connected ---\x1b[0m');
			terminal.writeln('');
		} catch (e) {
			console.error('Failed to initialize terminal:', e);
			error = e instanceof Error ? e.message : 'Failed to load terminal';
		}
	}

	// React to theme changes — swap xterm color palette
	const themeUnsubscribe = theme.subscribe((t) => {
		if (!terminal) return;
		terminal.options.theme = t === 'analog' ? analogTheme : phosphorTheme;
	});

	onDestroy(() => {
		// Unregister from server-side dimension negotiation before cleanup
		sendTerminalHidden();

		themeUnsubscribe();
		outputUnsubscribe?.();
		resizeObserver?.disconnect();
		terminal?.dispose();
		terminal = null;
		fitAddon = null;
	});

	export function clear() {
		terminal?.clear();
	}

	export function write(data: string) {
		terminal?.write(data);
	}
</script>

<div class="terminal-wrapper">
	{#if error}
		<div class="error">
			<span class="error-icon">!</span>
			{error}
		</div>
	{:else if !isReady}
		<div class="loading">
			<span class="spinner"></span>
			Loading terminal...
		</div>
	{/if}
	{#if showStatusBanner && isReady}
		<div class="status-banner" class:warning={isDisconnected} class:info={isReconnecting && !isDisconnected}>
			{#if isDisconnected}
				<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
					<path d="M18.364 5.636a9 9 0 11-12.728 0M12 9v4m0 4h.01" />
				</svg>
				<span>Disconnected</span>
			{:else if isReconnecting}
				<svg class="spinner-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
					<path
						d="M12 2v4m0 12v4m-7.07-3.93l2.83-2.83m8.48 8.48l2.83-2.83M2 12h4m12 0h4M4.93 4.93l2.83 2.83m8.48 8.48l2.83 2.83"
					/>
				</svg>
				<span>Reconnecting...</span>
			{/if}
			{#if $hasPendingInput}
				<span class="pending-badge">Input queued</span>
			{/if}
		</div>
	{/if}
	{#if showLockBanner && isReady}
		<div class="lock-banner" class:locked-by-other={$isLockedByOther} class:i-hold={$iHoldLock}>
			{#if $isLockedByOther}
				<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
					<rect x="3" y="11" width="18" height="11" rx="2" ry="2" />
					<path d="M7 11V7a5 5 0 0110 0v4" />
				</svg>
				<span>Terminal controlled by <strong>{$currentTerminalLock?.holder?.display_name}</strong></span>
				<button class="lock-action-btn" onclick={requestTerminalLock}>Take Control</button>
			{:else if $iHoldLock}
				<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
					<rect x="3" y="11" width="18" height="11" rx="2" ry="2" />
					<path d="M7 11V7a5 5 0 0110 0v4" />
				</svg>
				<span>You have terminal control</span>
				<button class="lock-action-btn release" onclick={releaseTerminalLock}>Release</button>
			{/if}
		</div>
	{/if}
	<div class="terminal-container" class:hidden={!isReady || error} bind:this={terminalEl}></div>
</div>

<style>
	.terminal-wrapper {
		width: 100%;
		height: 100%;
		position: relative;
		background: var(--surface-900);
	}

	.loading,
	.error {
		position: absolute;
		inset: 0;
		display: flex;
		align-items: center;
		justify-content: center;
		gap: 12px;
		color: var(--text-muted);
		font-size: 12px;
		letter-spacing: 0.05em;
		text-transform: uppercase;
	}

	.error {
		color: var(--status-red);
	}

	.error-icon {
		width: 24px;
		height: 24px;
		background: var(--status-red-strong);
		border: 1px solid var(--status-red-border);
		border-radius: 4px;
		display: flex;
		align-items: center;
		justify-content: center;
		font-weight: bold;
		color: var(--status-red);
	}

	.spinner {
		width: 14px;
		height: 14px;
		border: 2px solid var(--surface-border);
		border-top-color: var(--amber-500);
		border-radius: 50%;
		animation: spin 0.8s linear infinite;
	}

	@keyframes spin {
		to {
			transform: rotate(360deg);
		}
	}

	.terminal-container {
		width: 100%;
		height: 100%;
	}

	.terminal-container.hidden {
		visibility: hidden;
	}

	.status-banner {
		position: absolute;
		top: 0;
		left: 0;
		right: 0;
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 10px 14px;
		font-size: 11px;
		font-weight: 600;
		letter-spacing: 0.05em;
		text-transform: uppercase;
		z-index: 10;
		backdrop-filter: blur(4px);
	}

	.status-banner.warning {
		background: var(--status-red-tint);
		border-bottom: 1px solid var(--status-red-border);
		color: var(--status-red-text);
	}

	.status-banner.info {
		background: var(--tint-active-strong);
		border-bottom: 1px solid var(--tint-focus);
		color: var(--amber-400);
	}

	.status-banner svg {
		width: 14px;
		height: 14px;
		flex-shrink: 0;
	}

	.spinner-icon {
		animation: spin 1s linear infinite;
	}

	.pending-badge {
		margin-left: auto;
		padding: 4px 10px;
		background: var(--tint-focus);
		border: 1px solid var(--tint-selection);
		border-radius: 4px;
		font-size: 10px;
		font-weight: 600;
		color: var(--amber-400);
	}

	.lock-banner {
		position: absolute;
		top: 0;
		left: 0;
		right: 0;
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 8px 14px;
		font-size: 11px;
		font-weight: 600;
		letter-spacing: 0.05em;
		z-index: 10;
		backdrop-filter: blur(4px);
	}

	.lock-banner.locked-by-other {
		background: var(--status-red-tint);
		border-bottom: 1px solid var(--status-red-border);
		color: var(--status-red-text);
	}

	.lock-banner.i-hold {
		background: var(--status-green-tint);
		border-bottom: 1px solid var(--status-green-border);
		color: var(--status-green-text);
	}

	.lock-banner svg {
		width: 14px;
		height: 14px;
		flex-shrink: 0;
	}

	.lock-action-btn {
		margin-left: auto;
		padding: 4px 10px;
		background: var(--tint-focus);
		border: 1px solid var(--tint-selection);
		border-radius: 4px;
		font-size: 10px;
		font-weight: 600;
		font-family: inherit;
		letter-spacing: 0.05em;
		text-transform: uppercase;
		color: var(--amber-400);
		cursor: pointer;
		transition: all 0.15s ease;
	}

	.lock-action-btn:hover {
		background: var(--tint-selection);
		border-color: var(--tint-selection);
	}

	.lock-action-btn.release {
		background: var(--status-green-tint);
		border-color: var(--status-green-border);
		color: var(--status-green-text);
	}

	.lock-action-btn.release:hover {
		background: var(--status-green-border);
		border-color: var(--status-green);
	}

	.terminal-container :global(.xterm) {
		padding: 10px;
		height: 100%;
	}

	.terminal-container :global(.xterm-viewport) {
		background-color: transparent !important;
	}

	/* Terminal cursor glow */
	.terminal-container :global(.xterm-cursor-block) {
		box-shadow: 0 0 8px var(--amber-500);
	}

	/* Mobile responsive */
	@media (max-width: 639px) {
		.terminal-container :global(.xterm) {
			padding: 6px;
		}

		.status-banner {
			padding: 8px 12px;
			font-size: 10px;
			flex-wrap: wrap;
		}

		.pending-badge {
			margin-left: 0;
			margin-top: 6px;
			width: 100%;
			text-align: center;
		}

		.loading,
		.error {
			font-size: 11px;
			gap: 10px;
		}
	}
</style>
