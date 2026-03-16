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
	const isClaudeInstance = $derived(inst ? inst.kind.type === 'Structured' : true);

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
		<div class="view-toggle-bar">
			<button
				class="view-toggle"
				role="switch"
				aria-checked={viewMode === 'terminal'}
				aria-label="Toggle raw view"
				onclick={() => viewMode = viewMode === 'structured' ? 'terminal' : 'structured'}
			>
				<span class="toggle-label" class:active={viewMode === 'structured'}>Structured</span>
				<span class="toggle-track">
					<span class="toggle-thumb" class:on={viewMode === 'terminal'}></span>
				</span>
				<span class="toggle-label" class:active={viewMode === 'terminal'}>Raw</span>
			</button>
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

	.view-toggle-bar {
		display: flex;
		align-items: center;
		justify-content: flex-start;
		height: 24px;
		padding-left: 8px;
		background: var(--surface-800);
		border-bottom: 1px solid var(--surface-border);
		flex-shrink: 0;
	}

	.view-toggle {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		background: transparent;
		border: none;
		cursor: pointer;
		padding: 0;
		font-family: inherit;
	}

	.toggle-label {
		font-size: 9px;
		font-weight: 600;
		letter-spacing: 0.08em;
		text-transform: uppercase;
		color: var(--text-muted);
		transition: color 0.15s ease;
	}

	.toggle-label.active {
		color: var(--amber-400);
	}

	.toggle-track {
		position: relative;
		width: 24px;
		height: 12px;
		background: var(--surface-600);
		border: 1px solid var(--surface-border);
		border-radius: 6px;
	}

	.toggle-thumb {
		position: absolute;
		top: 1px;
		left: 1px;
		width: 8px;
		height: 8px;
		background: var(--text-muted);
		border-radius: 50%;
		transition: transform 0.15s ease, background 0.15s ease;
	}

	.toggle-thumb.on {
		transform: translateX(12px);
		background: var(--amber-400);
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
