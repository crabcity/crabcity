<script lang="ts">
	import { currentInstanceId, instanceList, createInstance, selectInstance } from '$lib/stores/instances';
	import { connectionStatus } from '$lib/stores/websocket';
	import { currentProject, projects } from '$lib/stores/projects';
	import { getStateInfo } from '$lib/utils/instance-state';
	import { isMobile, isDesktop } from '$lib/stores/ui';
	import { toggleExplorer, isExplorerOpen } from '$lib/stores/files';
	import { toggleChat, isChatOpen, totalUnread } from '$lib/stores/chat';
	import { isTaskPanelOpen, toggleTaskPanel, currentInstanceTaskCount } from '$lib/stores/tasks';
	import { defaultCommand } from '$lib/stores/settings';
	import { paneCount, splitPane, layoutState, applyPreset, resetLayout, focusPane, setPaneContent, getPaneInstanceId, defaultContentForKind } from '$lib/stores/layout';
	import type { PaneContentKind, LayoutPreset } from '$lib/stores/layout';
	import { sendRefresh } from '$lib/stores/websocket';
	import { activityLevel } from '$lib/stores/activity';
	import InstanceChip from './InstanceChip.svelte';

	const isMultiPane = $derived($paneCount > 1);

	function getStatusColor(status: string): string {
		switch (status) {
			case 'connected': return 'var(--status-green)';
			case 'connecting': case 'reconnecting': return 'var(--amber-500)';
			case 'error': case 'server_gone': return 'var(--status-red)';
			default: return 'var(--text-muted)';
		}
	}

	function getStatusText(status: string): string {
		switch (status) {
			case 'connected': return 'Online';
			case 'connecting': return 'Connecting';
			case 'reconnecting': return 'Reconnecting';
			case 'server_gone': return 'Offline';
			case 'error': return 'Error';
			default: return 'No Signal';
		}
	}

	let showRestored = $state(false);
	let prevConnectionStatus = $state('disconnected');

	$effect(() => {
		const status = $connectionStatus;
		if (status === 'connected' && (prevConnectionStatus === 'error' || prevConnectionStatus === 'reconnecting' || prevConnectionStatus === 'server_gone')) {
			showRestored = true;
			setTimeout(() => { showRestored = false; }, 2000);
		}
		prevConnectionStatus = status;
	});

	/** Focus the first pane showing this instance, or bind the focused pane to it */
	function handleChipClick(instanceId: string) {
		const state = $layoutState;
		// Try to find a pane already showing this instance
		for (const [paneId, pane] of state.panes) {
			const paneInstId = getPaneInstanceId(pane.content);
			if (paneInstId === instanceId) {
				focusPane(paneId);
				selectInstance(instanceId, false);
				return;
			}
		}
		// No pane shows this instance — bind focused pane to it
		const focusedId = state.focusedPaneId;
		const focusedPane = state.panes.get(focusedId);
		if (focusedPane && 'instanceId' in focusedPane.content) {
			setPaneContent(focusedId, { ...focusedPane.content, instanceId });
		}
		selectInstance(instanceId, false);
	}

	let isCreating = $state(false);

	async function handleCreateInstance() {
		if (isCreating) return;
		isCreating = true;
		const result = await createInstance({
			command: $defaultCommand,
			working_dir: $currentProject?.workingDir
		});
		if (result) {
			selectInstance(result.id);
		}
		isCreating = false;
	}

	/** In multi-pane mode, open a panel type as a new split pane */
	function openAsSplit(kind: PaneContentKind) {
		const focusedId = $layoutState.focusedPaneId;
		const instanceId = getPaneInstanceId($layoutState.panes.get(focusedId)?.content ?? { kind: 'terminal', instanceId: null }) ?? $currentInstanceId;
		splitPane(focusedId, 'vertical', defaultContentForKind(kind, instanceId));
	}

	function handleFilesClick() {
		if (isMultiPane) openAsSplit('file-explorer');
		else toggleExplorer();
	}

	function handleTasksClick() {
		if (isMultiPane) openAsSplit('tasks');
		else toggleTaskPanel();
	}

	function handleChatClick() {
		if (isMultiPane) openAsSplit('chat');
		else toggleChat();
	}

	let showPresetMenu = $state(false);

	function handlePresetSelect(preset: LayoutPreset) {
		applyPreset(preset);
		showPresetMenu = false;
	}

	function togglePresetMenu() {
		showPresetMenu = !showPresetMenu;
	}

	function handlePresetMenuBlur() {
		setTimeout(() => { showPresetMenu = false; }, 150);
	}

	// Instance fleet for the current project
	const fleetInstances = $derived($currentProject?.instances ?? $instanceList);
</script>

<header class="main-header">
	<!-- Left: Project identity + connection status -->
	<div class="header-project">
		{#if $currentProject}
			<span class="project-name">{$currentProject.name}</span>
			{#if $projects.length > 1}
				<span class="project-count">{$projects.length} projects</span>
			{/if}
		{:else}
			<span class="project-name">Crab City</span>
		{/if}
		<span
			class="connection-dot"
			class:signal-lost={$connectionStatus === 'error' || $connectionStatus === 'reconnecting' || $connectionStatus === 'server_gone'}
			class:signal-restored={showRestored}
			style="background: {getStatusColor($connectionStatus)}"
			title={showRestored ? 'Link Restored' : getStatusText($connectionStatus)}
		></span>
	</div>

	<!-- Center: Instance fleet chips -->
	<div class="header-fleet">
		{#if fleetInstances.length === 0}
			<span class="fleet-empty">No instances</span>
		{:else}
			{#each fleetInstances as instance (instance.id)}
				{@const stateInfo = getStateInfo(instance.id, instance.claude_state, instance.claude_state_stale)}
				<InstanceChip
					{instance}
					isFocused={$currentInstanceId === instance.id}
					{stateInfo}
					onclick={() => handleChipClick(instance.id)}
				/>
			{/each}
		{/if}
		<button
			class="fleet-add"
			onclick={handleCreateInstance}
			disabled={isCreating}
			title="New instance"
			aria-label="Create new instance"
		>
			{#if isCreating}
				<span class="mini-spinner"></span>
			{:else}
				+
			{/if}
		</button>
	</div>

	<!-- Right: Actions -->
	<div class="header-actions">
		<div class="preset-wrapper">
			<button
				class="action-btn icon-only-mobile"
				class:active={showPresetMenu}
				onclick={togglePresetMenu}
				onblur={handlePresetMenuBlur}
				title="Layout presets"
				aria-label="Layout presets"
				aria-expanded={showPresetMenu}
			>
				<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
					<rect x="3" y="3" width="7" height="7" />
					<rect x="14" y="3" width="7" height="7" />
					<rect x="3" y="14" width="7" height="7" />
					<rect x="14" y="14" width="7" height="7" />
				</svg>
				<span class="btn-label">Layout</span>
			</button>
			{#if showPresetMenu}
				<div class="preset-menu">
					<button class="preset-item" onclick={() => handlePresetSelect('single')}>
						<span class="preset-icon">
							<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5"><rect x="1" y="1" width="14" height="14" rx="1" /></svg>
						</span>
						Single
					</button>
					<button class="preset-item" onclick={() => handlePresetSelect('dev-split')}>
						<span class="preset-icon">
							<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5"><rect x="1" y="1" width="14" height="14" rx="1" /><line x1="10" y1="1" x2="10" y2="15" /></svg>
						</span>
						Dev Split
					</button>
					<button class="preset-item" onclick={() => handlePresetSelect('side-by-side')}>
						<span class="preset-icon">
							<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5"><rect x="1" y="1" width="14" height="14" rx="1" /><line x1="8" y1="1" x2="8" y2="15" /></svg>
						</span>
						Side by Side
					</button>
					{#if isMultiPane}
						<div class="preset-divider"></div>
						<button class="preset-item reset" onclick={() => { resetLayout(); showPresetMenu = false; }}>
							Reset
						</button>
					{/if}
				</div>
			{/if}
		</div>
		<button
			class="action-btn icon-only-mobile"
			class:active={!isMultiPane && $isExplorerOpen}
			onclick={handleFilesClick}
			title="Files"
			aria-label="Toggle file explorer"
		>
			<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
				<path d="M3 7v10a2 2 0 002 2h14a2 2 0 002-2V9a2 2 0 00-2-2h-6l-2-2H5a2 2 0 00-2 2z" />
			</svg>
			<span class="btn-label">Files</span>
		</button>
		<button class="action-btn icon-only-mobile" onclick={sendRefresh} title="Refresh" aria-label="Refresh">
			<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
				<path d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15" />
			</svg>
		</button>
		<button
			class="action-btn icon-only-mobile tasks-btn"
			class:active={!isMultiPane && $isTaskPanelOpen}
			onclick={handleTasksClick}
			title="Tasks"
			aria-label="Toggle task panel"
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
			class:active={!isMultiPane && $isChatOpen}
			onclick={handleChatClick}
			title="Chat"
			aria-label="Toggle chat panel"
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
		gap: 12px;
		padding: 6px 12px;
		background: linear-gradient(180deg, var(--surface-600) 0%, var(--surface-700) 100%);
		border-bottom: 1px solid var(--surface-border);
		flex-shrink: 0;
		box-shadow: var(--elevation-low);
		min-height: 40px;
	}

	/* Left: Project identity */
	.header-project {
		display: flex;
		align-items: center;
		gap: 6px;
		flex-shrink: 0;
	}

	.project-name {
		font-size: 12px;
		font-weight: 700;
		letter-spacing: 0.08em;
		color: var(--amber-400);
		text-shadow: var(--emphasis-strong);
		text-transform: uppercase;
		font-family: var(--font-display);
	}

	.project-count {
		font-size: 9px;
		color: var(--text-muted);
		letter-spacing: 0.05em;
	}

	.connection-dot {
		width: 6px;
		height: 6px;
		border-radius: 50%;
		flex-shrink: 0;
	}

	.connection-dot.signal-lost:not(.signal-restored) {
		animation: dot-blink 1.5s ease-in-out infinite;
	}

	.connection-dot.signal-restored {
		animation: dot-flash 0.5s ease-out;
	}

	@keyframes dot-blink {
		0%, 100% { opacity: 1; }
		50% { opacity: 0.3; }
	}

	@keyframes dot-flash {
		0% { box-shadow: 0 0 8px currentColor; }
		100% { box-shadow: none; }
	}

	/* Center: Fleet */
	.header-fleet {
		display: flex;
		align-items: center;
		gap: 4px;
		flex: 1;
		min-width: 0;
		overflow-x: auto;
		padding: 2px 0;
	}

	.header-fleet::-webkit-scrollbar { height: 0; }

	.fleet-empty {
		font-size: 10px;
		color: var(--text-muted);
		letter-spacing: 0.05em;
		text-transform: uppercase;
	}

	.fleet-add {
		display: flex;
		align-items: center;
		justify-content: center;
		width: 24px;
		height: 24px;
		background: var(--surface-600);
		border: 1px dashed var(--surface-border);
		border-radius: 3px;
		color: var(--text-muted);
		font-size: 14px;
		font-family: inherit;
		cursor: pointer;
		flex-shrink: 0;
		transition: all 0.15s ease;
	}

	.fleet-add:hover:not(:disabled) {
		border-color: var(--amber-600);
		color: var(--amber-400);
		background: var(--tint-active);
	}

	.fleet-add:disabled { opacity: 0.5; cursor: not-allowed; }

	.mini-spinner {
		width: 10px;
		height: 10px;
		border: 1.5px solid var(--surface-border);
		border-top-color: var(--amber-500);
		border-radius: 50%;
		animation: spin 0.8s linear infinite;
	}

	@keyframes spin { to { transform: rotate(360deg); } }

	/* Right: Actions */
	.header-actions {
		display: flex;
		gap: 6px;
		flex-shrink: 0;
	}

	.action-btn {
		display: flex;
		align-items: center;
		gap: 4px;
		padding: 5px 10px;
		background: linear-gradient(180deg, var(--surface-500) 0%, var(--surface-600) 100%);
		border: 1px solid var(--surface-border);
		border-radius: 3px;
		color: var(--text-secondary);
		font-size: 10px;
		font-weight: 600;
		font-family: inherit;
		letter-spacing: 0.05em;
		cursor: pointer;
		transition: all 0.15s ease;
		min-height: 28px;
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
		width: 12px;
		height: 12px;
		flex-shrink: 0;
	}

	.btn-label { display: inline; }

	.tasks-btn, .chat-btn { position: relative; }

	.tasks-badge, .chat-badge {
		position: absolute;
		top: 0;
		right: 0;
		min-width: 14px;
		height: 14px;
		padding: 0 3px;
		font-size: 8px;
		font-weight: 700;
		line-height: 14px;
		text-align: center;
		border-radius: 7px;
		background: var(--amber-500);
		color: var(--surface-900);
		box-shadow: var(--elevation-low);
	}

	.chat-badge {
		animation: badge-pulse 2s ease-in-out infinite;
	}

	@keyframes badge-pulse {
		0%, 100% { box-shadow: var(--elevation-low); }
		50% { box-shadow: var(--elevation-high); }
	}

	/* Mobile */
	@media (max-width: 639px) {
		.main-header {
			padding: 4px 8px;
			gap: 6px;
		}

		.header-project {
			display: none;
		}

		.action-btn.icon-only-mobile {
			padding: 5px;
			min-width: 28px;
			justify-content: center;
		}

		.action-btn.icon-only-mobile .btn-label {
			display: none;
		}

		.header-actions { gap: 4px; }
	}

	@media (min-width: 640px) and (max-width: 1023px) {
		.action-btn.icon-only-mobile .btn-label {
			display: none;
		}

		.action-btn.icon-only-mobile {
			padding: 5px 8px;
		}
	}

	/* Preset menu */
	.preset-wrapper { position: relative; }

	.preset-menu {
		position: absolute;
		top: 100%;
		right: 0;
		margin-top: 4px;
		min-width: 150px;
		background: var(--surface-600);
		border: 1px solid var(--surface-border);
		border-radius: 4px;
		box-shadow: var(--shadow-dropdown);
		padding: 4px 0;
		z-index: 60;
		animation: preset-pop 0.12s ease-out;
	}

	@keyframes preset-pop {
		from { opacity: 0; transform: scale(0.95) translateY(-4px); }
		to { opacity: 1; transform: scale(1) translateY(0); }
	}

	.preset-item {
		display: flex;
		align-items: center;
		gap: 8px;
		width: 100%;
		padding: 6px 12px;
		background: transparent;
		border: none;
		color: var(--text-secondary);
		font-size: 11px;
		font-weight: 600;
		font-family: inherit;
		letter-spacing: 0.03em;
		cursor: pointer;
		transition: all 0.1s ease;
		text-align: left;
	}

	.preset-item:hover { background: var(--tint-active-strong); color: var(--amber-400); }
	.preset-item.reset { color: var(--text-muted); }
	.preset-item.reset:hover { color: var(--text-primary); }

	.preset-icon {
		display: flex;
		align-items: center;
		justify-content: center;
		width: 16px;
		height: 16px;
		flex-shrink: 0;
	}

	.preset-icon svg { width: 14px; height: 14px; }

	.preset-divider {
		height: 1px;
		margin: 4px 0;
		background: var(--surface-border);
	}

	/* Analog theme */
	:global([data-theme="analog"]) .main-header {
		background-color: var(--surface-800);
		background-image: var(--grain-fine), var(--grain-coarse), var(--ink-wash);
		background-blend-mode: multiply, multiply, normal;
		border-bottom-width: 2px;
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
	}

	:global([data-theme="analog"]) .chat-badge {
		animation: none;
		box-shadow: 0 0 2px rgba(42, 31, 24, 0.2);
	}
</style>
