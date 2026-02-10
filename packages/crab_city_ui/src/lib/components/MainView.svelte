<script lang="ts">
	import { onDestroy } from 'svelte';
	import Terminal from './Terminal.svelte';
	import ConversationView from './ConversationView.svelte';
	import ErrorBoundary from './ErrorBoundary.svelte';
	import SnakeGame from './SnakeGame.svelte';
	import SnakeTeaser from './SnakeTeaser.svelte';
	import MainHeader from './main-view/MainHeader.svelte';
	import { currentInstance, currentInstanceId, isClaudeInstance, showTerminal, setTerminalMode, initTerminalModeFromUrl, initViewStateFromUrl } from '$lib/stores/instances';
	import { connect, disconnect } from '$lib/stores/websocket';
	import { isActive } from '$lib/stores/claude';
	import { currentVerb } from '$lib/stores/activity';
	import { openSidebar, isDesktop } from '$lib/stores/ui';
	import { openExplorer, fetchFileContent, openFileFromTool, openFileDiffLoading, setDiffData, setDiffError } from '$lib/stores/files';
	import { toggleChat, isChatOpen, totalUnread } from '$lib/stores/chat';
	import { openGitTab, fetchGitDiff, gitDiff } from '$lib/stores/git';
	import { diffEngine } from '$lib/stores/settings';
	import { get } from 'svelte/store';

	let lastInstanceId: string | null = null;
	let hasInitializedFromUrl = false;

	// Easter egg: click the monitor icon 3 times to launch snake
	let easterEggClicks = $state(0);
	let easterEggTimer: ReturnType<typeof setTimeout> | null = null;
	let showSnake = $state(false);

	function onEmptyIconClick() {
		easterEggClicks++;
		if (easterEggTimer) clearTimeout(easterEggTimer);
		easterEggTimer = setTimeout(() => { easterEggClicks = 0; }, 2000);
		if (easterEggClicks >= 3) {
			easterEggClicks = 0;
			showSnake = true;
		}
	}

	function exitSnake() {
		showSnake = false;
	}

	// React to instance changes - connect to selected instance
	$effect(() => {
		const instanceId = $currentInstanceId;

		if (instanceId !== lastInstanceId) {
			lastInstanceId = instanceId;

			if (instanceId) {
				connect(instanceId);
				if (!hasInitializedFromUrl) {
					hasInitializedFromUrl = true;
					const urlTerminalMode = initTerminalModeFromUrl();
					const url = new URL(window.location.href);
					if (url.searchParams.has('terminal')) {
						setTerminalMode(urlTerminalMode);
					} else {
						setTerminalMode(!$isClaudeInstance);
					}

					const viewState = initViewStateFromUrl();
					if (viewState.explorer) {
						openExplorer();
						if (viewState.explorer === 'git') {
							openGitTab();
						}
					}
					if (viewState.file) {
						const filePath = viewState.file;
						if (viewState.view === 'diff') {
							openFileDiffLoading(filePath, viewState.commit);
							fetchGitDiff(instanceId, viewState.commit, filePath, get(diffEngine))
								.then(() => {
									const diff = get(gitDiff);
									if (diff && diff.files.length > 0) {
										setDiffData(diff.files[0]);
									} else {
										setDiffError('No changes found');
									}
								})
								.catch(() => setDiffError('Failed to load diff'));
						} else {
							const lineNum = viewState.line;
							fetchFileContent(filePath)
								.then((content) => {
									openFileFromTool(filePath, content, lineNum);
								})
								.catch((err) => {
									console.error('Failed to restore file from URL:', err);
								});
						}
					}
				}
			} else {
				disconnect();
			}
		}
	});

	onDestroy(() => {
		disconnect();
	});

	// Update browser tab title to reflect current instance and activity
	$effect(() => {
		const instance = $currentInstance;
		if (instance) {
			const displayName = instance.custom_name ?? instance.name;
			if ($isActive) {
				document.title = `${$currentVerb}... | ${displayName}`;
			} else {
				document.title = displayName;
			}
		} else {
			document.title = 'Crab City';
		}
	});
</script>

<main class="main-view">
	{#if $currentInstance}
		<MainHeader />

		<!-- Content -->
		<div class="content">
			{#if $showTerminal}
				<div class="terminal-container">
					<ErrorBoundary>
						{#snippet children()}
							{#key 'terminal-' + $currentInstanceId}
								<Terminal />
							{/key}
						{/snippet}
					</ErrorBoundary>
				</div>
			{:else if $isClaudeInstance}
				<ErrorBoundary>
					{#snippet children()}
						{#key 'conversation-' + $currentInstanceId}
							<ConversationView />
						{/key}
					{/snippet}
				</ErrorBoundary>
			{:else}
				<div class="terminal-container">
					<ErrorBoundary>
						{#snippet children()}
							{#key 'terminal-' + $currentInstanceId}
								<Terminal />
							{/key}
						{/snippet}
					</ErrorBoundary>
				</div>
			{/if}
		</div>
	{:else}
		<!-- Empty state -->
		<div class="empty-state">
			{#if !$isDesktop}
				<button class="floating-menu-btn" onclick={openSidebar} aria-label="Open menu">
					<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
						<path d="M4 6h16M4 12h16M4 18h16" />
					</svg>
				</button>
			{/if}
			<button
				class="floating-chat-btn"
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
					<span class="floating-chat-badge">{$totalUnread > 99 ? '99+' : $totalUnread}</span>
				{/if}
			</button>
			{#if showSnake}
				<SnakeGame onexit={exitSnake} />
			{:else}
				<div class="empty-content">
					<!-- svelte-ignore a11y_click_events_have_key_events -->
					<!-- svelte-ignore a11y_no_static_element_interactions -->
					<div
					class="empty-icon"
					onclick={onEmptyIconClick}
					style="opacity: {0.3 + easterEggClicks * 0.25}; filter: drop-shadow(0 0 {easterEggClicks * 8}px var(--amber-500));"
				>
						<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5">
							<path
								d="M9.75 17L9 20l-1 1h8l-1-1-.75-3M3 13h18M5 17h14a2 2 0 002-2V5a2 2 0 00-2-2H5a2 2 0 00-2 2v10a2 2 0 002 2z"
							/>
						</svg>
						<div class="monitor-screen">
							<SnakeTeaser />
						</div>
					</div>
					<h2>No Instance Selected</h2>
					<p>Create a new instance or select one from the sidebar</p>
				</div>
			{/if}
		</div>
	{/if}
</main>

<style>
	.main-view {
		display: flex;
		flex-direction: column;
		flex: 1;
		min-width: 0;
		background: var(--surface-800);
	}

	.content {
		flex: 1;
		min-height: 0;
		display: flex;
		flex-direction: column;
	}

	.terminal-container {
		flex: 1;
		min-height: 0;
	}

	.empty-state {
		display: flex;
		align-items: center;
		justify-content: center;
		flex: 1;
		background: radial-gradient(ellipse at center, var(--surface-700) 0%, var(--surface-800) 70%);
	}

	.floating-menu-btn {
		position: absolute;
		top: 16px;
		left: 16px;
		z-index: 10;
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
	}

	.floating-menu-btn:hover, .floating-menu-btn:active {
		border-color: var(--amber-600);
		color: var(--amber-400);
		background: var(--tint-active);
	}

	.floating-menu-btn svg {
		width: 20px;
		height: 20px;
	}

	.floating-chat-btn {
		position: absolute;
		top: 16px;
		right: 16px;
		z-index: 10;
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
		cursor: pointer;
		transition: all 0.15s ease;
		min-height: 40px;
	}

	.floating-chat-btn:hover {
		background: linear-gradient(180deg, var(--surface-400) 0%, var(--surface-500) 100%);
		border-color: var(--surface-border-light);
		color: var(--text-primary);
	}

	.floating-chat-btn.active {
		background: linear-gradient(180deg, var(--tint-focus) 0%, var(--tint-active) 100%);
		border-color: var(--amber-600);
		color: var(--amber-400);
		box-shadow: var(--elevation-low);
	}

	.floating-chat-btn svg {
		width: 14px;
		height: 14px;
		flex-shrink: 0;
	}

	.floating-chat-badge {
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

	.empty-content {
		text-align: center;
		color: var(--text-muted);
	}

	.empty-icon {
		position: relative;
		width: 80px;
		height: 80px;
		margin: 0 auto 20px;
		opacity: 0.3;
		color: var(--amber-500);
		cursor: pointer;
	}

	.empty-icon svg {
		width: 100%;
		height: 100%;
	}

	.monitor-screen {
		position: absolute;
		left: 10px;
		top: 10px;
		width: 60px;
		height: 33px;
		overflow: hidden;
		border-radius: 1px;
	}

	.empty-content h2 {
		margin: 0 0 12px;
		font-size: 14px;
		font-weight: 600;
		letter-spacing: 0.1em;
		text-transform: uppercase;
		color: var(--text-secondary);
	}

	.empty-content p {
		margin: 0;
		font-size: 12px;
		letter-spacing: 0.05em;
	}

	:global([data-theme="analog"]) .empty-state {
		background-color: var(--surface-900);
		background-image:
			var(--grain-fine),
			var(--grain-coarse),
			radial-gradient(ellipse at 40% 30%, rgba(42,31,24,0.03) 0%, transparent 60%),
			radial-gradient(circle at 70% 80%, rgba(42,31,24,0.02) 0%, transparent 40%);
		background-blend-mode: multiply, multiply, normal, normal;
	}

	:global([data-theme="analog"]) .floating-chat-badge {
		box-shadow: 0 0 2px rgba(42, 31, 24, 0.2);
	}
</style>
