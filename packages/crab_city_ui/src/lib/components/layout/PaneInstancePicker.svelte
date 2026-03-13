<script lang="ts">
	import type { PaneContentKind, PaneContent } from '$lib/stores/layout';
	import { setPaneContent } from '$lib/stores/layout';
	import { instanceList, createInstance, selectInstance } from '$lib/stores/instances';
	import { defaultCommand } from '$lib/stores/settings';
	import { currentProject } from '$lib/stores/projects';

	interface Props {
		paneId: string;
		kind: PaneContentKind;
	}

	let { paneId, kind }: Props = $props();

	let isCreating = $state(false);

	const isTerminal = $derived(kind === 'terminal');

	const label = $derived.by(() => {
		switch (kind) {
			case 'terminal': return 'Terminal';
			case 'conversation': return 'Conversation';
			case 'file-explorer': return 'File Explorer';
			case 'tasks': return 'Tasks';
			case 'git': return 'Git';
			default: return 'Pane';
		}
	});

	// Terminal panes show only non-Claude (shell) instances;
	// all other pane kinds show only Claude instances.
	const filteredInstances = $derived(
		$instanceList.filter((inst) =>
			isTerminal ? !inst.command.includes('claude') : inst.command.includes('claude')
		)
	);

	function handleSelect(instanceId: string) {
		setPaneContent(paneId, { kind, instanceId } as PaneContent);
	}

	async function handleNew() {
		if (isCreating) return;
		isCreating = true;
		const command = isTerminal ? '/bin/bash' : $defaultCommand;
		const result = await createInstance({
			command,
			working_dir: $currentProject?.workingDir
		});
		if (result) {
			setPaneContent(paneId, { kind, instanceId: result.id } as PaneContent);
			selectInstance(result.id);
		}
		isCreating = false;
	}
</script>

<div class="picker">
	<div class="picker-inner">
		<h2 class="picker-title">{label}</h2>
		<p class="picker-subtitle">Select an instance or start new</p>

		{#if filteredInstances.length > 0}
			<div class="instance-list">
				{#each filteredInstances as inst}
					<button class="instance-btn" onclick={() => handleSelect(inst.id)}>
						<span class="instance-name">{inst.custom_name ?? inst.name}</span>
						<span class="instance-cmd">{inst.command}</span>
					</button>
				{/each}
			</div>
		{/if}

		<button class="new-btn" onclick={handleNew} disabled={isCreating}>
			{#if isCreating}
				Creating…
			{:else}
				+ New {isTerminal ? 'Shell' : 'Instance'}
			{/if}
		</button>
	</div>
</div>

<style>
	.picker {
		display: flex;
		align-items: center;
		justify-content: center;
		flex: 1;
		min-height: 0;
	}

	.picker-inner {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 12px;
		max-width: 280px;
		width: 100%;
		padding: 24px;
	}

	.picker-title {
		margin: 0;
		font-size: 13px;
		font-weight: 600;
		letter-spacing: 0.1em;
		text-transform: uppercase;
		color: var(--text-secondary);
	}

	.picker-subtitle {
		margin: 0;
		font-size: 11px;
		letter-spacing: 0.05em;
		color: var(--text-muted);
	}

	.instance-list {
		display: flex;
		flex-direction: column;
		gap: 2px;
		width: 100%;
		max-height: 200px;
		overflow-y: auto;
	}

	.instance-btn {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 8px;
		width: 100%;
		padding: 6px 10px;
		background: var(--surface-700);
		border: 1px solid var(--surface-border);
		border-radius: 3px;
		color: var(--text-secondary);
		font-family: inherit;
		font-size: 11px;
		cursor: pointer;
		transition: all 0.1s ease;
		text-align: left;
	}

	.instance-btn:hover {
		background: var(--surface-600);
		border-color: var(--amber-600);
		color: var(--amber-400);
	}

	.instance-name {
		font-weight: 600;
		letter-spacing: 0.03em;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.instance-cmd {
		font-size: 10px;
		color: var(--text-muted);
		flex-shrink: 0;
		max-width: 100px;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.new-btn {
		width: 100%;
		padding: 8px 12px;
		background: transparent;
		border: 1px dashed var(--amber-600);
		border-radius: 3px;
		color: var(--amber-500);
		font-family: inherit;
		font-size: 11px;
		font-weight: 600;
		letter-spacing: 0.08em;
		text-transform: uppercase;
		cursor: pointer;
		transition: all 0.1s ease;
	}

	.new-btn:hover:not(:disabled) {
		background: var(--surface-700);
		border-color: var(--amber-400);
		color: var(--amber-400);
	}

	.new-btn:disabled {
		opacity: 0.5;
		cursor: default;
	}
</style>
