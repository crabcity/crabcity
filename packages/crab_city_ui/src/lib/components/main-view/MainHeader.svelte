<script lang="ts">
	import { currentInstance, currentInstanceId, isClaudeInstance, showTerminal, setTerminalMode, setCustomName } from '$lib/stores/instances';
	import { sendRefresh, connectionStatus, instancePresence } from '$lib/stores/websocket';
	import { currentTerminalLock } from '$lib/stores/terminalLock';
	import { isActive, isThinking, isToolExecuting, currentTool } from '$lib/stores/claude';
	import { currentVerb, baudRate, activityLevel } from '$lib/stores/activity';
	import { openSidebar, isDesktop, isMobile } from '$lib/stores/ui';
	import { toggleExplorer, isExplorerOpen } from '$lib/stores/files';
	import { toggleChat, isChatOpen, totalUnread } from '$lib/stores/chat';
	import { isTaskPanelOpen, toggleTaskPanel, currentInstanceTaskCount } from '$lib/stores/tasks';

	let editingName = $state(false);
	let editNameValue = $state('');

	function startEditingName() {
		if (!$currentInstance) return;
		editNameValue = $currentInstance.custom_name ?? $currentInstance.name;
		editingName = true;
	}

	function commitNameEdit() {
		if (!editingName || !$currentInstance) return;
		const trimmed = editNameValue.trim();
		const newName = (!trimmed || trimmed === $currentInstance.name) ? null : trimmed;
		setCustomName($currentInstance.id, newName);
		editingName = false;
	}

	function handleNameKeydown(event: KeyboardEvent) {
		if (event.key === 'Enter') {
			event.preventDefault();
			commitNameEdit();
		} else if (event.key === 'Escape') {
			event.preventDefault();
			editingName = false;
		}
	}

	let presence = $derived($currentInstanceId ? $instancePresence.get($currentInstanceId) ?? [] : []);

	function getStatusColor(status: string): string {
		switch (status) {
			case 'connected': return '#10b981';
			case 'connecting': case 'reconnecting': return '#f59e0b';
			case 'error': return '#ef4444';
			default: return '#6b7280';
		}
	}

	function getStatusText(status: string): string {
		switch (status) {
			case 'connected': return 'Link Active';
			case 'connecting': return 'Connecting...';
			case 'reconnecting': case 'error': return 'Signal Lost';
			default: return 'No Signal';
		}
	}

	let showRestored = $state(false);
	let prevConnectionStatus = $state('disconnected');

	$effect(() => {
		const status = $connectionStatus;
		if (status === 'connected' && (prevConnectionStatus === 'error' || prevConnectionStatus === 'reconnecting')) {
			showRestored = true;
			setTimeout(() => { showRestored = false; }, 2000);
		}
		prevConnectionStatus = status;
	});

	$effect(() => {
		$currentInstanceId;
		editingName = false;
	});

	function toggleTerminal() {
		setTerminalMode(!$showTerminal);
	}
</script>

<header class="main-header">
	{#if !$isDesktop}
		<button class="menu-btn" onclick={openSidebar} aria-label="Open menu">
			<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
				<path d="M4 6h16M4 12h16M4 18h16" />
			</svg>
		</button>
	{/if}

	<div class="header-info">
		{#if editingName}
			<!-- svelte-ignore a11y_autofocus -->
			<input
				class="instance-name-input"
				type="text"
				bind:value={editNameValue}
				onblur={commitNameEdit}
				onkeydown={handleNameKeydown}
				autofocus
			/>
		{:else}
			<!-- svelte-ignore a11y_no_static_element_interactions -->
			<h2 class="instance-name" ondblclick={startEditingName}>{$currentInstance?.custom_name ?? $currentInstance?.name}</h2>
		{/if}
		<span
			class="connection-status"
			class:signal-lost={$connectionStatus === 'error' || $connectionStatus === 'reconnecting' || $connectionStatus === 'disconnected'}
			class:signal-restored={showRestored}
			style="color: {getStatusColor($connectionStatus)}"
		>
			{showRestored ? 'Link Restored' : getStatusText($connectionStatus)}
		</span>
		{#if presence.length > 1}
			<span class="presence-bar">
				{#each presence as user}
					<span class="presence-user" class:has-lock={$currentTerminalLock?.holder?.user_id === user.user_id}>
						{#if $currentTerminalLock?.holder?.user_id === user.user_id}
							<svg class="lock-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
								<rect x="3" y="11" width="18" height="11" rx="2" ry="2" />
								<path d="M7 11V7a5 5 0 0110 0v4" />
							</svg>
						{/if}
						{user.display_name}
					</span>
				{/each}
			</span>
		{/if}
	</div>

	{#if $isActive && $isClaudeInstance}
		<div class="baud-panel" class:thinking={$isThinking} class:tool={$isToolExecuting} class:compact={!$isDesktop}>
			<div class="panel-scanline"></div>
			<div class="panel-led" class:pulse={$activityLevel > 0}></div>
			{#if $isDesktop}
				<span class="panel-label">
					{#if $isToolExecuting && $currentTool}
						{$currentTool.toUpperCase().slice(0, 8)}
					{:else}
						{$currentVerb.toUpperCase()}
					{/if}
				</span>
			{/if}
			<div class="panel-meter">
				{#each Array($isMobile ? 5 : 10) as _, i}
					<div
						class="panel-bar"
						class:active={$activityLevel > i / ($isMobile ? 5 : 10)}
						class:hot={i >= ($isMobile ? 3 : 7)}
						class:warn={i >= ($isMobile ? 2 : 4) && i < ($isMobile ? 3 : 7)}
					></div>
				{/each}
			</div>
			{#if !$isMobile}
				<span class="panel-rate">{$baudRate.toString().padStart(4, '0')}</span>
			{/if}
		</div>
	{/if}

	<div class="header-actions">
		<button
			class="action-btn icon-only-mobile"
			class:active={$isExplorerOpen}
			onclick={toggleExplorer}
			title="Files (⌘E)"
			aria-label="Toggle file explorer"
			aria-pressed={$isExplorerOpen}
		>
			<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
				<path d="M3 7v10a2 2 0 002 2h14a2 2 0 002-2V9a2 2 0 00-2-2h-6l-2-2H5a2 2 0 00-2 2z" />
			</svg>
			<span class="btn-label">Files</span>
		</button>
		<button class="action-btn icon-only-mobile" onclick={sendRefresh} title="Refresh" aria-label="Refresh conversation">
			<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
				<path d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15" />
			</svg>
		</button>
		<button
			class="action-btn icon-only-mobile"
			class:active={$showTerminal}
			onclick={toggleTerminal}
			title="Toggle terminal"
			aria-label="Toggle terminal view"
			aria-pressed={$showTerminal}
		>
			<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
				<path d="M4 17l6-6-6-6M12 19h8" />
			</svg>
			<span class="btn-label">Terminal</span>
		</button>
		<button
			class="action-btn icon-only-mobile tasks-btn"
			class:active={$isTaskPanelOpen}
			onclick={toggleTaskPanel}
			title="Tasks (Q)"
			aria-label="Toggle task panel"
			aria-pressed={$isTaskPanelOpen}
		>
			<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
				<path d="M9 5H7a2 2 0 00-2 2v12a2 2 0 002 2h10a2 2 0 002-2V7a2 2 0 00-2-2h-2M9 5a2 2 0 002 2h2a2 2 0 002-2M9 5a2 2 0 012-2h2a2 2 0 012 2" />
			</svg>
			{#if $currentInstanceTaskCount > 0}
				<span class="tasks-badge">{$currentInstanceTaskCount}</span>
			{/if}
			<span class="btn-label">Tasks</span>
		</button>
		<button
			class="action-btn icon-only-mobile chat-btn"
			class:active={$isChatOpen}
			onclick={toggleChat}
			title="Chat (C)"
			aria-label="Toggle chat panel"
			aria-pressed={$isChatOpen}
		>
			<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
				<path d="M21 15a2 2 0 01-2 2H7l-4 4V5a2 2 0 012-2h14a2 2 0 012 2z" />
			</svg>
			{#if $totalUnread > 0}
				<span class="chat-badge">{$totalUnread > 99 ? '99+' : $totalUnread}</span>
			{/if}
			<span class="btn-label">Chat</span>
		</button>
	</div>
</header>

<style>
	.main-header {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 8px;
		padding: 12px 16px;
		background: linear-gradient(180deg, var(--surface-600) 0%, var(--surface-700) 100%);
		border-bottom: 1px solid var(--surface-border);
		flex-shrink: 0;
		box-shadow: var(--elevation-low);
	}

	.menu-btn {
		display: flex;
		align-items: center;
		justify-content: center;
		width: 40px;
		height: 40px;
		background: transparent;
		border: 1px solid var(--surface-border);
		border-radius: 4px;
		color: var(--text-secondary);
		cursor: pointer;
		transition: all 0.15s ease;
		flex-shrink: 0;
	}

	.menu-btn:hover, .menu-btn:active {
		border-color: var(--amber-600);
		color: var(--amber-400);
		background: var(--tint-active);
	}

	.menu-btn svg {
		width: 20px;
		height: 20px;
	}

	.header-info {
		display: flex;
		align-items: center;
		gap: 12px;
		flex: 1;
		min-width: 0;
	}

	.instance-name {
		margin: 0;
		font-size: 13px;
		font-weight: 600;
		letter-spacing: 0.05em;
		color: var(--amber-400);
		text-shadow: var(--emphasis-strong);
		transition: text-shadow 0.8s ease;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		cursor: default;
		font-family: var(--font-display);
	}

	.instance-name-input {
		margin: 0;
		font-size: 13px;
		font-weight: 600;
		letter-spacing: 0.05em;
		font-family: inherit;
		color: var(--amber-400);
		background: var(--surface-600);
		border: 1px solid var(--amber-600);
		border-radius: 2px;
		padding: 2px 6px;
		outline: none;
		min-width: 120px;
		max-width: 300px;
	}

	.connection-status {
		font-size: 10px;
		font-weight: 600;
		letter-spacing: 0.1em;
		text-transform: uppercase;
		flex-shrink: 0;
		transition: all 0.3s ease;
		font-family: var(--font-mono);
	}

	.connection-status.signal-lost {
		animation: signal-pulse 1.5s ease-in-out infinite;
	}

	@keyframes signal-pulse {
		0%, 100% { opacity: 1; }
		50% { opacity: 0.4; }
	}

	.connection-status.signal-restored {
		color: var(--status-green) !important;
		text-shadow: var(--emphasis);
		animation: signal-restore-flash 0.5s ease-out;
	}

	@keyframes signal-restore-flash {
		0% { filter: brightness(3); }
		100% { filter: brightness(1); }
	}

	.presence-bar {
		display: flex;
		gap: 6px;
		align-items: center;
		flex-shrink: 0;
	}

	.presence-user {
		font-size: 10px;
		font-weight: 600;
		letter-spacing: 0.05em;
		color: var(--text-secondary);
		background: var(--tint-active-strong);
		border: 1px solid var(--tint-focus);
		border-radius: 3px;
		padding: 2px 6px;
		display: flex;
		align-items: center;
		gap: 3px;
	}

	.presence-user.has-lock {
		border-color: var(--status-green-border);
		background: var(--status-green-tint);
	}

	.lock-icon {
		width: 10px;
		height: 10px;
		flex-shrink: 0;
		color: var(--status-green-text);
	}

	.header-actions {
		display: flex;
		gap: 8px;
		flex-shrink: 0;
	}

	.action-btn {
		display: flex;
		align-items: center;
		gap: 6px;
		padding: 8px 14px;
		background: linear-gradient(180deg, var(--surface-500) 0%, var(--surface-600) 100%);
		border: 1px solid var(--surface-border);
		border-radius: 4px;
		color: var(--text-secondary);
		font-size: 11px;
		font-weight: 600;
		font-family: inherit;
		letter-spacing: 0.05em;
		cursor: pointer;
		transition: all 0.15s ease;
		min-height: 40px;
	}

	.action-btn:hover {
		background: linear-gradient(180deg, var(--surface-400) 0%, var(--surface-500) 100%);
		border-color: var(--surface-border-light);
		color: var(--text-primary);
	}

	.action-btn.active {
		background: linear-gradient(180deg, var(--tint-focus) 0%, var(--tint-active) 100%);
		border-color: var(--amber-600);
		color: var(--amber-400);
		box-shadow: var(--elevation-low);
		text-shadow: var(--emphasis);
	}

	.action-btn svg {
		width: 14px;
		height: 14px;
		flex-shrink: 0;
	}

	.btn-label {
		display: inline;
	}

	.tasks-btn {
		position: relative;
	}

	.tasks-badge {
		position: absolute;
		top: 2px;
		right: 2px;
		min-width: 16px;
		height: 16px;
		padding: 0 4px;
		font-size: 9px;
		font-weight: 700;
		line-height: 16px;
		text-align: center;
		border-radius: 8px;
		background: var(--amber-500);
		color: var(--surface-900);
		box-shadow: var(--elevation-low);
	}

	.chat-btn {
		position: relative;
	}

	.chat-badge {
		position: absolute;
		top: 2px;
		right: 2px;
		min-width: 16px;
		height: 16px;
		padding: 0 4px;
		font-size: 9px;
		font-weight: 700;
		line-height: 16px;
		text-align: center;
		border-radius: 8px;
		background: var(--amber-500);
		color: var(--surface-900);
		box-shadow: var(--elevation-low);
		animation: badge-pulse 2s ease-in-out infinite;
	}

	@keyframes badge-pulse {
		0%, 100% { box-shadow: var(--elevation-low); }
		50% { box-shadow: var(--elevation-high); }
	}

	@media (max-width: 639px) {
		.main-header {
			padding: 10px 12px;
			gap: 6px;
		}

		.header-info {
			gap: 8px;
		}

		.instance-name {
			font-size: 12px;
		}

		.connection-status {
			display: none;
		}

		.action-btn.icon-only-mobile {
			padding: 8px;
			min-width: 40px;
			justify-content: center;
		}

		.action-btn.icon-only-mobile .btn-label {
			display: none;
		}

		.header-actions {
			gap: 6px;
		}
	}

	@media (min-width: 640px) and (max-width: 1023px) {
		.instance-name {
			max-width: 150px;
		}
	}

	.baud-panel {
		position: relative;
		display: flex;
		align-items: center;
		gap: 10px;
		padding: 6px 14px;
		background: linear-gradient(180deg, var(--surface-600) 0%, var(--surface-700) 100%);
		border: 1px solid var(--surface-border);
		border-radius: 4px;
		font-family: var(--font-mono);
		overflow: hidden;
		box-shadow: var(--depth-up);
	}

	.baud-panel::before {
		content: '';
		position: absolute;
		inset: 0;
		background: var(--texture-overlay);
		opacity: var(--texture-opacity);
		pointer-events: none;
		z-index: 10;
	}

	.panel-scanline {
		position: absolute;
		top: 0;
		left: 0;
		right: 0;
		height: 2px;
		background: linear-gradient(90deg, transparent, var(--amber-500), transparent);
		opacity: var(--texture-opacity);
		animation: panel-scan 1.5s linear infinite;
		z-index: 5;
	}

	@keyframes panel-scan {
		0% { top: 0; opacity: 1; }
		100% { top: 100%; opacity: 0.3; }
	}

	.baud-panel.thinking {
		border-color: var(--surface-border);
		box-shadow: var(--depth-up);
	}

	.baud-panel.thinking .panel-scanline {
		background: linear-gradient(90deg, transparent, var(--purple-500), transparent);
	}

	.baud-panel.thinking .panel-led {
		background: var(--purple-500);
		box-shadow: var(--elevation-low);
	}

	.baud-panel.thinking .panel-label {
		color: var(--purple-400);
	}

	.baud-panel.thinking .panel-rate {
		color: var(--text-primary);
		text-shadow: var(--emphasis);
	}

	.baud-panel.thinking .panel-bar.active {
		background: var(--purple-500);
		box-shadow: var(--elevation-low);
	}

	.panel-led {
		width: 6px;
		height: 6px;
		background: var(--amber-500);
		border-radius: 50%;
		box-shadow: var(--elevation-low);
		flex-shrink: 0;
	}

	.panel-led.pulse {
		animation: led-glow 0.4s ease-in-out infinite alternate;
	}

	@keyframes led-glow {
		0% { opacity: 0.5; }
		100% { opacity: 1; box-shadow: 0 0 8px currentColor, 0 0 16px currentColor; }
	}

	.panel-label {
		font-size: 10px;
		font-weight: 700;
		letter-spacing: 0.08em;
		color: var(--amber-500);
		text-shadow: var(--emphasis);
		min-width: 50px;
	}

	.panel-meter {
		display: flex;
		gap: 2px;
	}

	.panel-bar {
		width: 6px;
		height: 16px;
		background: var(--surface-500);
		border: 1px solid var(--surface-border);
		border-radius: 1px;
		transition: all 0.06s ease;
	}

	.panel-bar.active {
		background: var(--amber-500);
		box-shadow: var(--elevation-low);
		border-color: var(--amber-500);
	}

	.panel-bar.active.warn {
		background: var(--status-yellow);
		box-shadow: var(--elevation-low);
		border-color: var(--status-yellow);
	}

	.panel-bar.active.hot {
		background: var(--status-red);
		box-shadow: var(--elevation-low);
		border-color: var(--status-red);
		animation: bar-flash 0.1s ease infinite;
	}

	@keyframes bar-flash {
		0%, 100% { opacity: 1; }
		50% { opacity: 0.7; }
	}

	.panel-rate {
		font-size: 12px;
		font-weight: 700;
		color: var(--amber-400);
		text-shadow: var(--emphasis);
		font-variant-numeric: tabular-nums;
		min-width: 36px;
		text-align: right;
	}

	.baud-panel.compact {
		padding: 4px 10px;
		gap: 6px;
	}

	.baud-panel.compact .panel-bar {
		width: 5px;
		height: 12px;
	}

	@media (max-width: 639px) {
		.baud-panel {
			display: none;
		}
	}

	@media (min-width: 640px) and (max-width: 1023px) {
		.baud-panel {
			padding: 4px 10px;
			gap: 6px;
		}

		.panel-label {
			font-size: 9px;
			min-width: 40px;
		}

		.panel-bar {
			width: 5px;
			height: 12px;
		}

		.panel-rate {
			font-size: 10px;
			min-width: 28px;
		}
	}

	:global([data-theme="analog"]) .main-header {
		background-color: var(--surface-800);
		background-image: var(--grain-fine), var(--grain-coarse), var(--ink-wash);
		background-blend-mode: multiply, multiply, normal;
		border-bottom-width: 2px;
	}

	:global([data-theme="analog"]) .baud-panel::before {
		display: none;
	}

	:global([data-theme="analog"]) .panel-scanline {
		display: none;
	}

	:global([data-theme="analog"]) .baud-panel {
		background-color: var(--surface-700);
		background-image: var(--grain-fine);
		background-blend-mode: multiply;
		border-width: 2px;
	}

	:global([data-theme="analog"]) .panel-label {
		font-family: 'Source Serif 4', Georgia, serif;
		font-style: italic;
		font-weight: 600;
		letter-spacing: 0;
		text-transform: lowercase;
		color: var(--text-secondary);
	}

	:global([data-theme="analog"]) .panel-bar.active.hot {
		animation: none;
	}

	:global([data-theme="analog"]) .action-btn {
		background-color: var(--surface-600);
		background-image: var(--grain-fine);
		background-blend-mode: multiply;
		border-width: 1.5px;
		box-shadow: var(--elevation-low);
	}

	:global([data-theme="analog"]) .action-btn:hover {
		background-color: var(--surface-500);
		background-image: var(--grain-fine);
		background-blend-mode: multiply;
	}

	:global([data-theme="analog"]) .action-btn.active {
		background-color: var(--tint-active-strong);
		background-image: var(--grain-fine), var(--ink-wash);
		background-blend-mode: multiply, normal;
		border-width: 2px;
		box-shadow:
			var(--elevation-low),
			inset 0 1px 3px rgba(42, 31, 24, 0.06);
	}

	:global([data-theme="analog"]) .chat-badge {
		animation: none;
		box-shadow: 0 0 2px rgba(42, 31, 24, 0.2);
	}
</style>
