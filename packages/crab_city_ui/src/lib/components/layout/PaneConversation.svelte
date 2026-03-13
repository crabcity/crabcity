<script lang="ts">
	import ConversationView from '../ConversationView.svelte';
	import Terminal from '../Terminal.svelte';
	import ErrorBoundary from '../ErrorBoundary.svelte';
	import { instances } from '$lib/stores/instances';

	interface Props {
		instanceId: string;
	}

	let { instanceId }: Props = $props();

	const inst = $derived($instances.get(instanceId));
	const isClaudeInstance = $derived(inst ? inst.command.includes('claude') : true);

	let viewMode = $state<'structured' | 'terminal'>('structured');

	// Default to terminal view for non-Claude instances
	$effect(() => {
		if (!isClaudeInstance) {
			viewMode = 'terminal';
		}
	});
</script>

<div class="pane-conversation">
	{#if isClaudeInstance}
		<div class="view-toggle" role="tablist" aria-label="View mode">
			<button
				class="toggle-tab"
				class:active={viewMode === 'structured'}
				role="tab"
				aria-selected={viewMode === 'structured'}
				onclick={() => viewMode = 'structured'}
			>Structured</button>
			<button
				class="toggle-tab"
				class:active={viewMode === 'terminal'}
				role="tab"
				aria-selected={viewMode === 'terminal'}
				onclick={() => viewMode = 'terminal'}
			>Raw</button>
		</div>
	{/if}

	<div class="pane-content-inner">
		<ErrorBoundary>
			{#snippet children()}
				{#if viewMode === 'structured'}
					{#key 'conversation-' + instanceId}
						<ConversationView {instanceId} />
					{/key}
				{:else}
					<div class="pane-terminal">
						{#key 'terminal-' + instanceId}
							<Terminal {instanceId} />
						{/key}
					</div>
				{/if}
			{/snippet}
		</ErrorBoundary>
	</div>
</div>

<style>
	.pane-conversation {
		display: flex;
		flex-direction: column;
		flex: 1;
		min-height: 0;
	}

	.view-toggle {
		display: flex;
		height: 24px;
		padding: 0 8px;
		background: var(--surface-800);
		border-bottom: 1px solid var(--surface-border);
		flex-shrink: 0;
		gap: 0;
	}

	.toggle-tab {
		font-size: 10px;
		font-weight: 600;
		letter-spacing: 0.08em;
		text-transform: uppercase;
		color: var(--text-muted);
		background: transparent;
		border: none;
		border-bottom: 2px solid transparent;
		cursor: pointer;
		font-family: inherit;
		padding: 0 10px;
		line-height: 22px;
		transition: color 0.1s ease, border-color 0.1s ease;
	}

	.toggle-tab:hover {
		color: var(--text-secondary);
	}

	.toggle-tab.active {
		color: var(--amber-400);
		border-bottom-color: var(--amber-400);
	}

	.pane-content-inner {
		flex: 1;
		min-height: 0;
		display: flex;
		flex-direction: column;
	}

	.pane-terminal {
		flex: 1;
		min-height: 0;
	}
</style>
