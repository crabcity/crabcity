<script lang="ts">
	import { sendMessage, connectionStatus, hasPendingInput } from '$lib/stores/websocket';
	import { isActive } from '$lib/stores/claude';
	import { currentInstanceId } from '$lib/stores/instances';
	import { quickAddTask, stagedTask, clearStagedTask, commitStagedTask } from '$lib/stores/tasks';
	import { onMount } from 'svelte';

	let message = $state('');
	let inputEl: HTMLTextAreaElement;

	let isDisconnected = $derived($connectionStatus === 'disconnected' || $connectionStatus === 'error');
	let isReconnecting = $derived($connectionStatus === 'connecting' || $connectionStatus === 'reconnecting');
	let showBanner = $derived(isDisconnected || isReconnecting || $hasPendingInput);

	// Queue flash confirmation
	let queueFlash = $state(false);

	// Watch for staged tasks — populate input when one arrives
	$effect(() => {
		const task = $stagedTask;
		if (task) {
			message = task.body || task.title;
			// Focus the input so the user can review and send
			requestAnimationFrame(() => {
				inputEl?.focus();
				if (inputEl) autoResize(inputEl);
			});
		}
	});

	// Voice input state
	let speechSupported = $state(false);
	let isListening = $state(false);
	let recognition: SpeechRecognition | null = null;
	// Track the message content before voice input started, so we can append interim results
	let messageBeforeVoice = '';

	// Check for Web Speech API support and initialize
	onMount(() => {
		const SpeechRecognition = window.SpeechRecognition || window.webkitSpeechRecognition;
		if (SpeechRecognition) {
			speechSupported = true;
			recognition = new SpeechRecognition();
			recognition.continuous = true;
			recognition.interimResults = true;
			recognition.lang = 'en-US';

			recognition.onresult = (event) => {
				const transcript = Array.from(event.results)
					.map(result => result[0].transcript)
					.join(' ');

				// Always show current transcript (interim or final) appended to pre-voice message
				const separator = messageBeforeVoice && !messageBeforeVoice.endsWith(' ') ? ' ' : '';
				message = messageBeforeVoice + separator + transcript;

				// When final, update the base so next voice session builds from here
				if (event.results[event.results.length - 1].isFinal) {
					messageBeforeVoice = message;
				}
			};

			recognition.onerror = (event) => {
				console.error('Speech recognition error:', event.error);
				isListening = false;
			};

			recognition.onend = () => {
				isListening = false;
			};
		}

		return () => {
			if (recognition) {
				recognition.abort();
			}
		};
	});

	function toggleVoiceInput() {
		if (!recognition) return;

		if (isListening) {
			recognition.stop();
			isListening = false;
		} else {
			// Capture current message so we can append voice transcript to it
			messageBeforeVoice = message;
			recognition.start();
			isListening = true;
		}
	}

	function handleSubmit() {
		if (!message.trim()) return;

		// Stop voice input if active
		if (isListening && recognition) {
			recognition.stop();
			isListening = false;
		}

		// If a task was staged, append a structural tag and send with task_id
		if ($stagedTask) {
			const tag = `\n[task:#${$stagedTask.id}]`;
			const taggedMessage = message + tag;
			sendMessage(taggedMessage, $stagedTask.id);
			commitStagedTask(taggedMessage);
		} else {
			sendMessage(message);
		}

		message = '';
		messageBeforeVoice = '';

		// Refocus input
		inputEl?.focus();
	}

	function handleAddToQueue() {
		if (!message.trim() || !$currentInstanceId) return;

		if (isListening && recognition) {
			recognition.stop();
			isListening = false;
		}

		quickAddTask($currentInstanceId, message.trim());
		message = '';
		messageBeforeVoice = '';

		// Flash confirmation
		queueFlash = true;
		setTimeout(() => { queueFlash = false; }, 400);

		inputEl?.focus();
	}

	function handleKeydown(e: KeyboardEvent) {
		if (e.key === 'Enter' && !e.shiftKey && (e.metaKey || e.ctrlKey)) {
			// Cmd/Ctrl+Shift handled below — but Cmd+Enter sends normally
		}
		if (e.key === 'Enter' && e.shiftKey && (e.metaKey || e.ctrlKey)) {
			e.preventDefault();
			handleAddToQueue();
			return;
		}
		if (e.key === 'Enter' && !e.shiftKey) {
			e.preventDefault();
			handleSubmit();
		}
	}

	// Auto-resize textarea - reactively tracks message content
	function autoResize(el: HTMLTextAreaElement) {
		el.style.height = 'auto';
		el.style.height = Math.min(el.scrollHeight, 200) + 'px';
	}

	$effect(() => {
		// Track message to re-run when content changes (including deletions)
		// eslint-disable-next-line @typescript-eslint/no-unused-expressions
		message;
		if (inputEl) {
			autoResize(inputEl);
		}
	});
</script>

<div class="input-container" class:disconnected={isDisconnected} class:reconnecting={isReconnecting}>
	{#if $stagedTask}
		<div class="staged-banner">
			<svg class="staged-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
				<path d="M9 5H7a2 2 0 00-2 2v12a2 2 0 002 2h10a2 2 0 002-2V7a2 2 0 00-2-2h-2M9 5a2 2 0 002 2h2a2 2 0 002-2M9 5a2 2 0 012-2h2a2 2 0 012 2" />
			</svg>
			<span class="staged-label">Task:</span>
			<span class="staged-title">{$stagedTask.title}</span>
			<button class="staged-dismiss" onclick={clearStagedTask} aria-label="Dismiss task" title="Dismiss (keep editing)">
				<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
					<path d="M6 18L18 6M6 6l12 12" />
				</svg>
			</button>
		</div>
	{/if}
	{#if showBanner}
		<div class="status-banner" class:warning={isDisconnected} class:info={isReconnecting && !isDisconnected}>
			{#if isDisconnected}
				<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
					<path d="M18.364 5.636a9 9 0 11-12.728 0M12 9v4m0 4h.01" />
				</svg>
				<span>Disconnected</span>
			{:else if isReconnecting}
				<svg class="spinner-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
					<path d="M12 2v4m0 12v4m-7.07-3.93l2.83-2.83m8.48 8.48l2.83-2.83M2 12h4m12 0h4M4.93 4.93l2.83 2.83m8.48 8.48l2.83 2.83" />
				</svg>
				<span>Reconnecting...</span>
			{/if}
			{#if $hasPendingInput}
				<span class="pending-badge">Message queued — will send when connected</span>
			{/if}
		</div>
	{/if}
	<div class="input-row">
		<textarea
			bind:this={inputEl}
			bind:value={message}
			onkeydown={handleKeydown}
			oninput={() => inputEl && autoResize(inputEl)}
			placeholder={isDisconnected ? "Type here — will send when reconnected..." : "Message Claude..."}
			rows="1"
		></textarea>
		{#if speechSupported}
			<button
				class="voice-btn"
				class:listening={isListening}
				onclick={toggleVoiceInput}
				aria-label={isListening ? 'Stop voice input' : 'Start voice input'}
				title={isListening ? 'Stop listening' : 'Voice input'}
			>
				<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
					{#if isListening}
						<!-- Stop icon when listening -->
						<rect x="6" y="6" width="12" height="12" rx="1" />
					{:else}
						<!-- Microphone icon -->
						<path d="M12 1a3 3 0 0 0-3 3v8a3 3 0 0 0 6 0V4a3 3 0 0 0-3-3z" />
						<path d="M19 10v2a7 7 0 0 1-14 0v-2" />
						<line x1="12" y1="19" x2="12" y2="23" />
						<line x1="8" y1="23" x2="16" y2="23" />
					{/if}
				</svg>
			</button>
		{/if}
		{#if $isActive}
			<button
				class="queue-btn"
				class:flash={queueFlash}
				onclick={handleAddToQueue}
				disabled={!message.trim()}
				aria-label="Add to queue (Cmd+Shift+Enter)"
				title="Add to queue (⌘⇧↵)"
			>
				<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
					<path d="M4 6h16M4 12h16M4 18h10" />
					<path d="M19 15l3 3-3 3" />
				</svg>
			</button>
		{/if}
		<button class="send-btn" onclick={handleSubmit} disabled={!message.trim()} aria-label="Send message">
			<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
				<path d="M22 2L11 13M22 2l-7 20-4-9-9-4 20-7z" />
			</svg>
		</button>
	</div>
</div>

<style>
	.input-container {
		display: flex;
		flex-direction: column;
		gap: 0;
		padding: 0;
		background: linear-gradient(180deg, var(--surface-600) 0%, var(--surface-700) 100%);
		border-top: 1px solid var(--surface-border);
	}

	.input-container.disconnected {
		border-top-color: var(--status-red-border);
	}

	.input-container.reconnecting {
		border-top-color: var(--tint-selection);
	}

	.status-banner {
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 10px 16px;
		font-size: 11px;
		font-weight: 600;
		letter-spacing: 0.05em;
		text-transform: uppercase;
	}

	.status-banner.warning {
		background: var(--status-red-tint);
		color: var(--status-red-text);
		border-bottom: 1px solid var(--status-red-border);
	}

	.status-banner.info {
		background: var(--tint-active-strong);
		color: var(--amber-400);
		border-bottom: 1px solid var(--tint-focus);
	}

	.status-banner svg {
		width: 14px;
		height: 14px;
		flex-shrink: 0;
	}

	.spinner-icon {
		animation: spin 1s linear infinite;
	}

	@keyframes spin {
		from { transform: rotate(0deg); }
		to { transform: rotate(360deg); }
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

	.input-row {
		display: flex;
		align-items: flex-end;
		gap: 10px;
		padding: 14px 16px;
	}

	textarea {
		flex: 1;
		padding: 12px 14px;
		background: var(--surface-800);
		border: 1px solid var(--surface-border);
		border-radius: 4px;
		color: var(--text-primary);
		font-size: 13px;
		font-family: inherit;
		line-height: 1.5;
		resize: none;
		min-height: 44px;
		max-height: 200px;
		transition: all 0.15s ease;
	}

	textarea:focus {
		outline: none;
		border-color: var(--amber-600);
		box-shadow: var(--elevation-low);
	}

	textarea::placeholder {
		color: var(--text-muted);
	}

	.voice-btn,
	.send-btn {
		display: flex;
		align-items: center;
		justify-content: center;
		width: 44px;
		height: 44px;
		background: linear-gradient(180deg, var(--surface-500) 0%, var(--surface-600) 100%);
		border: 1px solid var(--surface-border);
		border-radius: 4px;
		color: var(--text-secondary);
		cursor: pointer;
		transition: all 0.15s ease;
		flex-shrink: 0;
	}

	.send-btn {
		border-color: var(--amber-600);
		color: var(--amber-400);
	}

	.voice-btn:hover {
		background: linear-gradient(180deg, var(--surface-400) 0%, var(--surface-500) 100%);
		border-color: var(--amber-600);
		color: var(--amber-400);
	}

	.voice-btn.listening {
		background: linear-gradient(180deg, var(--status-red-strong) 0%, var(--status-red-tint) 100%);
		border-color: var(--status-red);
		color: var(--status-red-text);
		animation: pulse-glow 1.5s ease-in-out infinite;
	}

	@keyframes pulse-glow {
		0%, 100% { box-shadow: 0 0 8px var(--status-red-border); }
		50% { box-shadow: 0 0 16px var(--status-red-strong); }
	}

	.voice-btn svg,
	.send-btn svg {
		width: 18px;
		height: 18px;
	}

	.send-btn:hover:not(:disabled) {
		background: linear-gradient(180deg, var(--surface-400) 0%, var(--surface-500) 100%);
		border-color: var(--amber-500);
		color: var(--amber-300);
		box-shadow: var(--elevation-high);
	}

	.send-btn:disabled {
		background: var(--surface-700);
		border-color: var(--surface-border);
		color: var(--text-muted);
		cursor: not-allowed;
	}

	.queue-btn {
		display: flex;
		align-items: center;
		justify-content: center;
		width: 44px;
		height: 44px;
		background: linear-gradient(180deg, var(--surface-500) 0%, var(--surface-600) 100%);
		border: 1px solid var(--tint-focus);
		border-radius: 4px;
		color: var(--amber-500);
		cursor: pointer;
		transition: all 0.15s ease;
		flex-shrink: 0;
	}

	.queue-btn svg {
		width: 18px;
		height: 18px;
	}

	.queue-btn:hover:not(:disabled) {
		background: linear-gradient(180deg, var(--surface-400) 0%, var(--surface-500) 100%);
		border-color: var(--amber-500);
		color: var(--amber-300);
		box-shadow: var(--elevation-low);
	}

	.queue-btn:disabled {
		background: var(--surface-700);
		border-color: var(--surface-border);
		color: var(--text-muted);
		cursor: not-allowed;
	}

	.queue-btn.flash {
		animation: queue-flash 0.4s ease-out;
	}

	@keyframes queue-flash {
		0% { background: var(--amber-600); border-color: var(--amber-400); }
		100% { background: linear-gradient(180deg, var(--surface-500) 0%, var(--surface-600) 100%); }
	}

	/* Mobile responsive */
	@media (max-width: 639px) {
		.input-row {
			padding: 10px 12px;
			gap: 8px;
		}

		textarea {
			padding: 10px 12px;
			font-size: 16px; /* Prevents iOS zoom on focus */
		}

		.voice-btn,
		.send-btn,
		.queue-btn {
			width: 48px;
			height: 48px;
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
	}

	/* === Staged task banner === */
	.staged-banner {
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 8px 16px;
		background: var(--tint-active-strong);
		border-bottom: 1px solid var(--amber-600);
		animation: staged-slide-in 0.2s ease-out;
	}

	@keyframes staged-slide-in {
		from { opacity: 0; max-height: 0; }
		to { opacity: 1; max-height: 48px; }
	}

	.staged-icon {
		width: 14px;
		height: 14px;
		color: var(--amber-500);
		flex-shrink: 0;
	}

	.staged-label {
		font-size: 10px;
		font-weight: 700;
		letter-spacing: 0.08em;
		text-transform: uppercase;
		color: var(--amber-400);
		flex-shrink: 0;
	}

	.staged-title {
		font-size: 11px;
		font-weight: 500;
		color: var(--text-primary);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		flex: 1;
		min-width: 0;
	}

	.staged-dismiss {
		display: flex;
		align-items: center;
		justify-content: center;
		width: 20px;
		height: 20px;
		background: none;
		border: 1px solid transparent;
		border-radius: 3px;
		color: var(--text-muted);
		cursor: pointer;
		flex-shrink: 0;
		transition: all 0.15s ease;
	}

	.staged-dismiss:hover {
		border-color: var(--surface-border-light);
		color: var(--text-secondary);
	}

	.staged-dismiss svg {
		width: 12px;
		height: 12px;
	}

	/* Analog overrides */
	:global([data-theme="analog"]) .staged-banner {
		background-color: var(--tint-active-strong);
		background-image: var(--grain-fine);
		background-blend-mode: multiply;
		border-bottom-width: 2px;
	}

	:global([data-theme="analog"]) .staged-label {
		font-family: 'Source Serif 4', Georgia, serif;
		font-style: italic;
		text-transform: none;
		letter-spacing: 0;
	}

	/* Safe area insets for mobile notches/home indicators */
	@supports (padding-bottom: env(safe-area-inset-bottom)) {
		.input-container {
			padding-bottom: env(safe-area-inset-bottom);
		}
	}
</style>
