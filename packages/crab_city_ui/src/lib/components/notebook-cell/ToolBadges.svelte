<script lang="ts">
	import type { ToolCell } from '$lib/types';
	import { openFilePath } from '$lib/stores/files';

	interface Props {
		toolCells: ToolCell[];
	}

	let { toolCells }: Props = $props();

	let toolsExpanded = $state(false);

	const toolCount = $derived(toolCells.length);
	const shouldCollapse = $derived(toolCount > 4);

	function getToolIcon(name: string): string {
		const icons: Record<string, string> = {
			Read: '📖',
			Write: '✏️',
			Edit: '🔧',
			Bash: '💻',
			Glob: '🔍',
			Grep: '🔎',
			WebFetch: '🌐',
			WebSearch: '🔍',
			Task: '📋'
		};
		return icons[name] ?? '⚡';
	}
</script>

<div class="cell-tools">
	{#if shouldCollapse && !toolsExpanded}
		<button class="tools-collapsed-toggle" onclick={() => toolsExpanded = true}>
			<span class="tools-count">{toolCount} tool operations</span>
			<span class="tools-expand-icon">&#9654;</span>
		</button>
	{:else}
		{#each toolCells as tool}
			{@const isFileOp = tool.name === 'Read' || tool.name === 'Edit' || tool.name === 'Write'}
			{@const filePath = isFileOp && tool.input?.file_path ? String(tool.input.file_path) : null}
			{#if filePath}
				<button
					class="tool-badge file-tool"
					title="Open {filePath}"
					onclick={() => openFilePath(filePath)}
				>
					<span class="tool-icon">{getToolIcon(tool.name)}</span>
					<span class="tool-name">{tool.name}</span>
					<span class="tool-file">{filePath.split('/').pop()}</span>
				</button>
			{:else}
				<span class="tool-badge" class:rerunnable={tool.canRerun} title={tool.name}>
					<span class="tool-icon">{getToolIcon(tool.name)}</span>
					<span class="tool-name">{tool.name}</span>
				</span>
			{/if}
		{/each}
		{#if shouldCollapse}
			<button class="tools-collapse-toggle" onclick={() => toolsExpanded = false}>
				&#9650; Collapse
			</button>
		{/if}
	{/if}
</div>

<style>
	.cell-tools {
		margin-top: 12px;
		margin-left: 16px;
		padding-left: 12px;
		border-left: 1px solid var(--surface-border);
		display: flex;
		flex-wrap: wrap;
		gap: 6px;
		position: relative;
	}

	/* Junction dot connecting tools to parent message */
	.cell-tools::before {
		content: '';
		position: absolute;
		left: -3px;
		top: 8px;
		width: 5px;
		height: 5px;
		border-radius: 50%;
		background: var(--surface-border-light);
	}

	/* Collapsed tools toggle */
	.tools-collapsed-toggle {
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 6px 12px;
		background: var(--surface-700);
		border: 1px solid var(--surface-border);
		border-radius: 4px;
		color: var(--text-secondary);
		font-family: inherit;
		font-size: 10px;
		font-weight: 600;
		letter-spacing: 0.05em;
		cursor: pointer;
		transition: all 0.15s ease;
	}

	.tools-collapsed-toggle:hover {
		background: var(--surface-600);
		border-color: var(--amber-600);
		color: var(--amber-400);
	}

	.tools-count {
		text-transform: uppercase;
	}

	.tools-expand-icon {
		font-size: 8px;
		opacity: 0.6;
	}

	.tools-collapse-toggle {
		background: none;
		border: none;
		color: var(--text-muted);
		font-family: inherit;
		font-size: 9px;
		cursor: pointer;
		padding: 4px 8px;
		opacity: 0.5;
		transition: opacity 0.15s ease;
		width: 100%;
		text-align: left;
	}

	.tools-collapse-toggle:hover {
		opacity: 1;
		color: var(--amber-400);
	}

	/* File path label on file operation tools */
	.tool-file {
		font-weight: 400;
		font-size: 9px;
		color: var(--text-muted);
		letter-spacing: 0;
		text-transform: none;
		max-width: 120px;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.tool-badge {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		padding: 5px 10px;
		background: var(--surface-700);
		border: 1px solid var(--surface-border);
		border-radius: 4px;
		font-size: 10px;
		font-weight: 600;
		letter-spacing: 0.05em;
		color: var(--amber-400);
		transition: all 0.15s ease;
	}

	.tool-badge.rerunnable {
		cursor: pointer;
	}

	.tool-badge.rerunnable:hover {
		background: var(--surface-600);
		border-color: var(--amber-600);
		box-shadow: var(--elevation-low);
	}

	/* File operation tool badges - clickable */
	button.tool-badge.file-tool {
		cursor: pointer;
		border-left: 2px solid var(--amber-500);
	}

	button.tool-badge.file-tool:hover {
		background: var(--surface-600);
		border-color: var(--amber-600);
		border-left-color: var(--amber-400);
		box-shadow: var(--elevation-low);
		text-shadow: var(--emphasis);
	}

	button.tool-badge.file-tool:focus {
		outline: none;
		box-shadow: 0 0 0 2px var(--tint-selection);
	}

	.tool-icon {
		font-size: 11px;
	}

	.tool-name {
		font-family: inherit;
		font-weight: 700;
		text-transform: uppercase;
		letter-spacing: 0.15em;
	}

	/* Mobile */
	@media (max-width: 639px) {
		.cell-tools {
			margin-top: 10px;
			gap: 4px;
		}

		.tool-badge {
			padding: 4px 8px;
			font-size: 9px;
			gap: 4px;
		}

		.tool-icon {
			font-size: 10px;
		}
	}

	/* Analog theme */
	:global([data-theme="analog"]) .tool-badge {
		background-color: var(--surface-700);
		background-image: var(--grain-fine);
		background-blend-mode: multiply;
		border-width: 1.5px;
		box-shadow: 0 1px 2px rgba(42, 31, 24, 0.06);
	}
</style>
