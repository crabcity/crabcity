<script lang="ts">
	import { layoutState, focusPane, paneCount } from '$lib/stores/layout';
	import type { PaneState } from '$lib/stores/layout';
	import PaneTerminal from './PaneTerminal.svelte';
	import PaneConversation from './PaneConversation.svelte';
	import PaneFileExplorer from './PaneFileExplorer.svelte';
	import PaneChat from './PaneChat.svelte';
	import PaneTasks from './PaneTasks.svelte';
	import PaneFileViewer from './PaneFileViewer.svelte';
	import PaneGit from './PaneGit.svelte';
	import PaneChrome from './PaneChrome.svelte';
	import { currentInstanceId, isClaudeInstance } from '$lib/stores/instances';

	interface Props {
		paneId: string;
	}

	let { paneId }: Props = $props();

	const pane = $derived($layoutState.panes.get(paneId) ?? null);
	const isFocused = $derived($layoutState.focusedPaneId === paneId);
	const showChrome = $derived($paneCount > 1);

	// Resolve instanceId: pane-specific or fall back to global
	const instanceId = $derived(pane?.content.instanceId ?? $currentInstanceId);

	// Resolve effective content kind.
	// When kind is 'conversation' but instance is not Claude, fall back to terminal.
	const effectiveKind = $derived(
		pane?.content.kind === 'conversation' && !$isClaudeInstance
			? 'terminal'
			: pane?.content.kind ?? 'terminal'
	);

	function handleFocus() {
		focusPane(paneId);
	}
</script>

<!-- svelte-ignore a11y_click_events_have_key_events -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
	class="pane-host"
	class:focused={isFocused && showChrome}
	onclick={handleFocus}
>
	{#if showChrome && pane}
		<PaneChrome {pane} />
	{/if}

	<div class="pane-content">
		{#if pane}
			{#if effectiveKind === 'terminal' && instanceId}
				<PaneTerminal {instanceId} />
			{:else if effectiveKind === 'conversation' && instanceId}
				<PaneConversation {instanceId} />
			{:else if effectiveKind === 'file-explorer'}
				<PaneFileExplorer />
			{:else if effectiveKind === 'chat'}
				<PaneChat />
			{:else if effectiveKind === 'tasks'}
				<PaneTasks />
			{:else if effectiveKind === 'file-viewer'}
				<PaneFileViewer />
			{:else if effectiveKind === 'git'}
				<PaneGit />
			{:else}
				<div class="pane-empty">
					<span class="pane-empty-label">No instance selected</span>
					<span class="pane-empty-hint">Select an instance from the sidebar</span>
				</div>
			{/if}
		{/if}
	</div>
</div>

<style>
	.pane-host {
		display: flex;
		flex-direction: column;
		width: 100%;
		height: 100%;
		min-width: 0;
		min-height: 0;
		overflow: hidden;
	}

	.pane-host.focused {
		outline: 1px solid var(--amber-600);
		outline-offset: -1px;
	}

	.pane-content {
		flex: 1;
		min-height: 0;
		display: flex;
		flex-direction: column;
	}

	.pane-empty {
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		flex: 1;
		gap: 6px;
	}

	.pane-empty-label {
		font-size: 12px;
		font-weight: 600;
		letter-spacing: 0.08em;
		text-transform: uppercase;
		color: var(--text-muted);
	}

	.pane-empty-hint {
		font-size: 10px;
		color: var(--text-muted);
		opacity: 0.6;
	}
</style>
