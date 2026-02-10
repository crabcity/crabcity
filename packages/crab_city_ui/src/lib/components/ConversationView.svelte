<script lang="ts">
	import { notebookCells, isWaiting, toolStats } from '$lib/stores/conversation';
	import { isActive, isThinking, isToolExecuting, currentTool } from '$lib/stores/claude';
	import { currentVerb } from '$lib/stores/activity';
	import { isDesktop } from '$lib/stores/ui';
	import { currentInstanceId } from '$lib/stores/instances';
	import NotebookCell from './NotebookCell.svelte';
	import MessageInput from './MessageInput.svelte';
	import TodoQueue from './TodoQueue.svelte';
	import TopoAvatar from './TopoAvatar.svelte';
	import ConversationMinimap from './ConversationMinimap.svelte';
	import VirtualList from './VirtualList.svelte';

	// Reference to VirtualList for scroll control
	let virtualList = $state<VirtualList<(typeof $notebookCells)[0]> | undefined>();
	let scrollInfo = $state({ scrollTop: 0, scrollHeight: 0, clientHeight: 0, visibleStart: 0, visibleEnd: 0 });

	// Use server state for activity, fall back to heuristic isWaiting for empty state detection
	const showEmptyState = $derived($isWaiting && $notebookCells.length === 0);

	// Debug logging removed - was firing on every state change and causing jank with DevTools open

	// Handle scroll updates from VirtualList (for minimap)
	function handleScroll(scrollTop: number, scrollHeight: number, clientHeight: number, visibleStart: number, visibleEnd: number) {
		scrollInfo = { scrollTop, scrollHeight, clientHeight, visibleStart, visibleEnd };
	}

	// Handle minimap click to scroll to a cell
	function handleScrollToIndex(index: number) {
		virtualList?.scrollToIndex(index);
	}

	// Format tool stats for display
	const topTools = $derived(
		Array.from($toolStats.entries())
			.sort((a, b) => b[1] - a[1])
			.slice(0, 6)
	);

	// Track latest message for screen reader announcements
	let prevCellCount = $state(0);
	let latestAnnouncement = $state('');

	$effect(() => {
		const cells = $notebookCells;
		if (cells.length > prevCellCount && cells.length > 0) {
			const latest = cells[cells.length - 1];
			if (latest) {
				const role = latest.type === 'user' ? 'User' : 'Claude';
				const preview = latest.content?.slice(0, 100) ?? '';
				latestAnnouncement = `New message from ${role}: ${preview}`;
			}
		}
		prevCellCount = cells.length;
	});

	// --- Keyboard navigation (j/k between messages, Enter to toggle thinking) ---
	let focusedIndex = $state<number | null>(null);

	function handleConversationKeydown(e: KeyboardEvent) {
		const tag = (document.activeElement?.tagName ?? '').toLowerCase();
		if (tag === 'input' || tag === 'textarea') return;

		const cellCount = $notebookCells.length;
		if (cellCount === 0) return;

		if (e.key === 'j') {
			e.preventDefault();
			focusedIndex = focusedIndex === null
				? cellCount - 1
				: Math.min(cellCount - 1, focusedIndex + 1);
			virtualList?.scrollToIndex(focusedIndex);
		} else if (e.key === 'k') {
			e.preventDefault();
			focusedIndex = focusedIndex === null
				? cellCount - 1
				: Math.max(0, focusedIndex - 1);
			virtualList?.scrollToIndex(focusedIndex);
		} else if (e.key === 'Enter' && focusedIndex !== null) {
			e.preventDefault();
			const cell = $notebookCells[focusedIndex];
			if (cell) {
				const el = document.querySelector(`[data-cell-id="${cell.id}"]`);
				const toggle = el?.querySelector('.thinking-toggle, .tools-collapsed-toggle') as HTMLElement;
				if (toggle) toggle.click();
			}
		} else if (e.key === 'Escape' && focusedIndex !== null) {
			focusedIndex = null;
		}
	}

	// --- "New messages below" indicator ---
	let lastBottomCount = $state(0);
	let newMessageCount = $state(0);

	$effect(() => {
		const count = $notebookCells.length;
		const atBottom = scrollInfo.scrollHeight - scrollInfo.scrollTop - scrollInfo.clientHeight < 200;

		if (atBottom) {
			lastBottomCount = count;
			newMessageCount = 0;
		} else if (count > lastBottomCount) {
			newMessageCount = count - lastBottomCount;
		}
	});

	// --- Timestamp dividers (show when messages span different hours) ---
	function getTimeDivider(index: number): string | null {
		if (index === 0) return null;
		const current = $notebookCells[index];
		const prev = $notebookCells[index - 1];
		if (!current?.timestamp || !prev?.timestamp) return null;

		const currentDate = new Date(current.timestamp);
		const prevDate = new Date(prev.timestamp);

		if (currentDate.toDateString() !== prevDate.toDateString() ||
			currentDate.getHours() !== prevDate.getHours()) {
			return currentDate.toLocaleTimeString('en-US', {
				hour: '2-digit', minute: '2-digit', hour12: false
			});
		}
		return null;
	}
</script>

<!-- Keyboard navigation for conversation -->
<svelte:window on:keydown={handleConversationKeydown} />

<!-- Screen reader live region for new message announcements -->
<div aria-live="polite" aria-atomic="true" class="sr-only">
	{latestAnnouncement}
</div>

<div class="conversation-view" class:with-minimap={$isDesktop && $notebookCells.length > 0}>
	<!-- Stats bar -->
	{#if topTools.length > 0}
		<div class="stats-bar">
			{#each topTools as [name, count]}
				<span class="stat-item">
					{name}
					<span class="stat-count">{count}</span>
				</span>
			{/each}
		</div>
	{/if}

	<!-- Messages container (relative for minimap positioning) -->
	<div class="messages-wrapper">
		{#if showEmptyState}
			<div class="messages">
				<div class="empty-state">
					<div class="empty-icon">
						<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5">
							<path
								d="M8 12h.01M12 12h.01M16 12h.01M21 12c0 4.418-4.03 8-9 8a9.863 9.863 0 01-4.255-.949L3 20l1.395-3.72C3.512 15.042 3 13.574 3 12c0-4.418 4.03-8 9-8s9 3.582 9 8z"
							/>
						</svg>
					</div>
					<h3>Start a conversation</h3>
					<p>Send a message to begin working with Claude</p>
				</div>
			</div>
		{:else}
			<!-- Always use VirtualList for consistent scroll tracking and minimap support -->
			<VirtualList
				bind:this={virtualList}
				items={$notebookCells}
				estimatedHeight={120}
				buffer={3}
				autoScroll={true}
				onScroll={handleScroll}
			>
				{#snippet children({ item: cell, index })}
					{@const timeDivider = getTimeDivider(index)}
					<div class="notebook-cell-wrapper">
						{#if timeDivider}
							<div class="time-divider">
								<span class="time-divider-label">{timeDivider}</span>
							</div>
						{/if}
						<NotebookCell {cell} instanceId={$currentInstanceId} age={$notebookCells.length - 1 - index} focused={focusedIndex === index} />
					</div>
				{/snippet}
				{#snippet footer()}
					{#if $isActive}
						<div class="activity-indicator" class:thinking={$isThinking}>
							<TopoAvatar
								identity={$currentInstanceId ?? 'claude'}
								type="agent"
								variant={$isThinking ? 'thinking' : 'assistant'}
								size={24}
								animated={true}
							/>
							{#key $isToolExecuting ? $currentTool : $currentVerb}
							<span class="activity-text">
								{#if $isToolExecuting && $currentTool}
									Running {$currentTool}...
								{:else}
									{$currentVerb}...
								{/if}
							</span>
						{/key}
						</div>
					{/if}
				{/snippet}
			</VirtualList>

			<!-- "New messages below" indicator -->
			{#if newMessageCount > 0}
				<button class="new-messages-bar" onclick={() => virtualList?.scrollToBottom()}>
					<span class="new-messages-count">{newMessageCount}</span> new &#9660;
				</button>
			{/if}
		{/if}

		<!-- Minimap (desktop only, when there are messages) -->
		{#if $isDesktop && $notebookCells.length > 0}
			<ConversationMinimap
				cells={$notebookCells}
				scrollTop={scrollInfo.scrollTop}
				scrollHeight={scrollInfo.scrollHeight}
				clientHeight={scrollInfo.clientHeight}
				visibleStart={scrollInfo.visibleStart}
				visibleEnd={scrollInfo.visibleEnd}
				onScrollToIndex={handleScrollToIndex}
			/>
		{/if}
	</div>

	<!-- Todo Queue -->
	<TodoQueue />

	<!-- Input -->
	<MessageInput />
</div>

<style>
	/* Screen reader only - visually hidden but accessible */
	.sr-only {
		position: absolute;
		width: 1px;
		height: 1px;
		padding: 0;
		margin: -1px;
		overflow: hidden;
		clip: rect(0, 0, 0, 0);
		white-space: nowrap;
		border: 0;
	}

	.conversation-view {
		display: flex;
		flex-direction: column;
		height: 100%;
		background: var(--surface-800);
	}

	.stats-bar {
		display: flex;
		gap: 8px;
		padding: 8px 16px;
		background: linear-gradient(180deg, var(--surface-600) 0%, var(--surface-700) 100%);
		border-bottom: 1px solid var(--surface-border);
		overflow-x: auto;
		flex-shrink: 0;
	}

	.stat-item {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		padding: 4px 10px;
		background: var(--surface-800);
		border: 1px solid var(--surface-border);
		border-radius: 4px;
		font-size: 10px;
		font-weight: 500;
		letter-spacing: 0.05em;
		color: var(--text-secondary);
		white-space: nowrap;
		text-transform: uppercase;
	}

	.stat-count {
		background: var(--surface-500);
		padding: 2px 6px;
		border-radius: 3px;
		font-weight: 700;
		color: var(--amber-400);
		text-shadow: var(--emphasis);
		transition: text-shadow 0.8s ease;
	}

	.messages-wrapper {
		flex: 1;
		position: relative;
		overflow: hidden;
	}

	.messages {
		height: 100%;
		overflow-y: auto;
		overflow-x: hidden;
	}

	/* Add right padding when minimap is visible */
	.conversation-view.with-minimap .messages,
	.conversation-view.with-minimap :global(.virtual-container) {
		padding-right: 56px;
	}

	.notebook-cell-wrapper {
		padding: 0 16px;
	}

	.empty-state {
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		height: 100%;
		color: var(--text-muted);
		text-align: center;
		padding: 24px;
		background: radial-gradient(ellipse at center, var(--surface-700) 0%, var(--surface-800) 70%);
	}

	.empty-icon {
		width: 80px;
		height: 80px;
		margin-bottom: 20px;
		opacity: 0.3;
		color: var(--amber-500);
	}

	.empty-icon svg {
		width: 100%;
		height: 100%;
	}

	.empty-state h3 {
		margin: 0 0 12px;
		font-size: 14px;
		font-weight: 600;
		letter-spacing: 0.1em;
		text-transform: uppercase;
		color: var(--text-secondary);
	}

	.empty-state p {
		margin: 0;
		font-size: 12px;
		letter-spacing: 0.05em;
	}

	/* Activity indicator - amber CRT style with power-on drama */
	.activity-indicator {
		display: flex;
		align-items: center;
		gap: 12px;
		padding: 12px 20px;
		margin: 8px 16px;
		background: linear-gradient(180deg, var(--tint-active) 0%, var(--tint-hover) 100%);
		border: 1px solid var(--tint-focus);
		border-radius: 4px;
		box-shadow: var(--elevation-low);
		/* Power-on flicker when appearing */
		animation: indicator-power-on 0.3s ease-out;
	}

	@keyframes indicator-power-on {
		0% { opacity: 0; }
		20% { opacity: 1; }
		35% { opacity: 0.5; }
		50% { opacity: 0.7; }
		70% { opacity: 1; }
		100% { opacity: 1; }
	}

	.activity-indicator.thinking {
		background: linear-gradient(180deg, var(--tint-thinking-strong) 0%, var(--tint-thinking) 100%);
		border-color: var(--tint-thinking-strong);
		box-shadow: var(--elevation-low);
	}

	.activity-text {
		font-size: 11px;
		font-weight: 600;
		letter-spacing: 0.1em;
		text-transform: uppercase;
		color: var(--amber-400);
		text-shadow: var(--emphasis);
		/* Typewriter reveal + blinking block cursor */
		display: inline-block;
		overflow: hidden;
		white-space: nowrap;
		border-right: 2px solid var(--amber-400);
		animation:
			activity-typewriter 0.5s steps(20) forwards,
			activity-cursor-blink 0.8s step-end infinite 0.5s;
	}

	@keyframes activity-typewriter {
		from { max-width: 0; }
		to { max-width: 20ch; }
	}

	@keyframes activity-cursor-blink {
		0%, 100% { border-right-color: currentColor; }
		50% { border-right-color: transparent; }
	}

	.activity-indicator.thinking .activity-text {
		color: var(--purple-400);
		text-shadow: var(--emphasis);
	}

	/* Timestamp dividers — thin centered line with time label */
	.time-divider {
		display: flex;
		align-items: center;
		gap: 12px;
		padding: 8px 0;
		margin: 4px 0;
	}

	.time-divider::before,
	.time-divider::after {
		content: '';
		flex: 1;
		height: 1px;
		background: var(--surface-border);
		opacity: 0.4;
	}

	.time-divider-label {
		font-size: 10px;
		font-weight: 500;
		letter-spacing: 0.1em;
		text-transform: uppercase;
		color: var(--text-muted);
		font-variant-numeric: tabular-nums;
		flex-shrink: 0;
	}

	/* "New messages below" indicator — amber pulsing bar */
	.new-messages-bar {
		position: absolute;
		bottom: 8px;
		left: 50%;
		transform: translateX(-50%);
		z-index: 20;
		display: flex;
		align-items: center;
		gap: 6px;
		padding: 6px 16px;
		background: linear-gradient(180deg, var(--tint-focus) 0%, var(--tint-active-strong) 100%);
		border: 1px solid var(--tint-selection);
		border-radius: 20px;
		color: var(--amber-400);
		font-family: inherit;
		font-size: 10px;
		font-weight: 600;
		letter-spacing: 0.05em;
		text-transform: uppercase;
		cursor: pointer;
		backdrop-filter: blur(8px);
		box-shadow: var(--shadow-dropdown);
		transition: all 0.15s ease;
		animation: new-msg-pulse 2s ease-in-out infinite;
	}

	.new-messages-bar:hover {
		background: linear-gradient(180deg, var(--tint-selection) 0%, var(--tint-focus) 100%);
		border-color: var(--tint-selection);
		box-shadow: var(--elevation-high);
	}

	.new-messages-count {
		text-shadow: var(--emphasis);
	}

	@keyframes new-msg-pulse {
		0%, 100% { opacity: 0.9; }
		50% { opacity: 1; }
	}

	/* Scrollbar */
	.messages::-webkit-scrollbar {
		width: 8px;
	}

	.messages::-webkit-scrollbar-track {
		background: transparent;
	}

	.messages::-webkit-scrollbar-thumb {
		background: var(--surface-border);
		border-radius: 4px;
	}

	.messages::-webkit-scrollbar-thumb:hover {
		background: var(--amber-600);
	}

	/* Mobile responsive */
	@media (max-width: 639px) {
		.stats-bar {
			padding: 8px 12px;
			gap: 6px;
		}

		.stat-item {
			padding: 3px 8px;
			font-size: 9px;
		}

		.stat-count {
			padding: 1px 4px;
		}

		.notebook-cell-wrapper {
			padding: 0 8px;
		}

		.activity-indicator {
			padding: 12px 14px;
			margin: 6px 12px;
			gap: 10px;
		}

		.activity-text {
			font-size: 10px;
		}

		.empty-state {
			padding: 20px;
		}

		.empty-icon {
			width: 60px;
			height: 60px;
			margin-bottom: 16px;
		}

		.empty-state h3 {
			font-size: 13px;
		}

		.empty-state p {
			font-size: 11px;
		}
	}

	/* ============================================
	   ANALOG THEME — ConversationView overrides
	   Fountain pen activity indicator
	   ============================================ */

	/* Stats bar: paper grain with ruled-line aesthetic */
	:global([data-theme="analog"]) .stats-bar {
		background-color: var(--surface-800);
		background-image: var(--grain-fine), var(--grain-coarse);
		background-blend-mode: multiply, multiply;
		border-bottom-width: 2px;
	}

	/* Stat items: ink stamps with fine grain visible */
	:global([data-theme="analog"]) .stat-item {
		background-color: var(--surface-700);
		background-image: var(--grain-fine);
		background-blend-mode: multiply;
		border-width: 1.5px;
	}

	/* Activity indicator: heavy pen stroke down margin, paper grain */
	:global([data-theme="analog"]) .activity-indicator {
		background-color: var(--tint-active);
		background-image: var(--grain-fine), var(--ink-wash);
		background-blend-mode: multiply, normal;
		border-left: 3px solid var(--amber-600);
		box-shadow: inset 2px 0 4px rgba(42, 31, 24, 0.08);
		animation: ink-bleed-in 0.5s cubic-bezier(0.1, 0.9, 0.2, 1);
	}

	:global([data-theme="analog"]) .activity-indicator.thinking {
		border-left-color: var(--purple-500);
		background-color: var(--tint-thinking);
		background-image: var(--grain-fine);
		background-blend-mode: multiply;
		box-shadow: inset 2px 0 4px rgba(26, 74, 122, 0.08);
	}

	/* Activity text: marginal annotation, as if penned in by hand */
	:global([data-theme="analog"]) .activity-text {
		font-family: 'Source Serif 4', Georgia, serif;
		font-style: italic;
		font-weight: 600;
		text-transform: none;
		letter-spacing: 0;
		text-shadow: 0 0 2px rgba(42, 31, 24, 0.1);
	}

	/* Empty state inside conversation: paper grain */
	:global([data-theme="analog"]) .empty-state {
		background-color: var(--surface-900);
		background-image: var(--grain-fine), var(--grain-coarse);
		background-blend-mode: multiply, multiply;
	}

	@keyframes ink-bleed-in {
		0% { opacity: 0; transform: translateY(-1px); border-left-width: 1px; }
		40% { opacity: 0.6; border-left-width: 4px; }
		70% { border-left-width: 3px; }
		100% { opacity: 1; transform: translateY(0); border-left-width: 3px; }
	}
</style>
