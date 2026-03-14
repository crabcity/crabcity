<script lang="ts">
	import { layoutState, focusPane, paneCount, getPaneInstanceId } from '$lib/stores/layout';
	import PaneTerminal from './PaneTerminal.svelte';
	import PaneConversation from './PaneConversation.svelte';
	import PaneFileExplorer from './PaneFileExplorer.svelte';
	import PaneChat from './PaneChat.svelte';
	import PaneTasks from './PaneTasks.svelte';
	import PaneFileViewer from './PaneFileViewer.svelte';
	import PaneGit from './PaneGit.svelte';
	import PaneChrome from './PaneChrome.svelte';
	import PaneLanding from './PaneLanding.svelte';
	import PaneInstancePicker from './PaneInstancePicker.svelte';

	interface Props {
		paneId: string;
	}

	let { paneId }: Props = $props();

	const pane = $derived($layoutState.panes.get(paneId) ?? null);
	const isFocused = $derived($layoutState.focusedPaneId === paneId);
	const showChrome = $derived($paneCount > 1);

	// Pane kinds that show the instance picker when instanceId is null.
	const PICKER_KINDS = new Set(['terminal', 'conversation', 'file-explorer', 'tasks', 'git']);

	const needsInstancePicker = $derived.by(() => {
		if (!pane) return false;
		if (!PICKER_KINDS.has(pane.content.kind)) return false;
		return !getPaneInstanceId(pane.content);
	});

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
			{@const content = pane.content}
			{#if content.kind === 'landing'}
				<PaneLanding />
			{:else if needsInstancePicker}
				<PaneInstancePicker paneId={pane.id} kind={content.kind} />
			{:else if content.kind === 'terminal' && content.instanceId}
				<PaneTerminal instanceId={content.instanceId} />
			{:else if content.kind === 'conversation' && content.instanceId}
				<PaneConversation instanceId={content.instanceId} />
			{:else if content.kind === 'file-explorer' && 'instanceId' in content}
				<PaneFileExplorer instanceId={content.instanceId} />
			{:else if content.kind === 'chat'}
				<PaneChat scope={content.scope} />
			{:else if content.kind === 'tasks' && 'instanceId' in content}
				<PaneTasks instanceId={content.instanceId} />
			{:else if content.kind === 'file-viewer'}
				<PaneFileViewer filePath={content.filePath} lineNumber={content.lineNumber} paneId={pane.id} />
			{:else if content.kind === 'git' && 'instanceId' in content}
				<PaneGit instanceId={content.instanceId} />
			{:else}
				<PaneLanding />
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

</style>
