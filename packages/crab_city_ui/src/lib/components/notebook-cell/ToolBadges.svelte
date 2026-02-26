<script lang="ts">
	import type { ToolCell } from '$lib/types';
	import { openFilePath, openExplorerWithSearch } from '$lib/stores/files';

	interface Props {
		toolCells: ToolCell[];
		agentLog?: Array<{ content: string; agentId?: string; role?: string }>;
	}

	let { toolCells, agentLog }: Props = $props();

	let toolsExpanded = $state(false);
	let expandedToolId: string | null = $state(null);

	const toolCount = $derived(toolCells.length);
	const shouldCollapse = $derived(toolCount > 4);

	const expandedTool = $derived(
		expandedToolId ? toolCells.find((t) => t.id === expandedToolId) ?? null : null
	);

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

	/** Extract a human-readable detail string from tool input for badge label */
	function getToolDetail(name: string, input: Record<string, unknown>): string | null {
		let raw: string | null = null;
		switch (name) {
			case 'Bash': raw = input.command as string | null; break;
			case 'Grep': raw = input.pattern as string | null; break;
			case 'Glob': raw = input.pattern as string | null; break;
			case 'WebFetch': raw = input.url as string | null; break;
			case 'WebSearch': raw = input.query as string | null; break;
			case 'Task': raw = input.description as string | null; break;
			default: return null;
		}
		if (!raw) return null;
		return raw.length > 40 ? raw.slice(0, 40) + '…' : raw;
	}

	function toggleTool(toolId: string) {
		expandedToolId = expandedToolId === toolId ? null : toolId;
	}

	/** Get labeled input fields for the expanded view */
	function getExpandedInputFields(tool: ToolCell): Array<{ label: string; value: string; clickable?: 'file' | 'glob' }> {
		const fields: Array<{ label: string; value: string; clickable?: 'file' | 'glob' }> = [];
		const input = tool.input;

		switch (tool.name) {
			case 'Read':
			case 'Write':
			case 'Edit':
				if (input.file_path) fields.push({ label: 'FILE', value: String(input.file_path), clickable: 'file' });
				if (input.old_string != null) fields.push({ label: 'OLD', value: String(input.old_string) });
				if (input.new_string != null) fields.push({ label: 'NEW', value: String(input.new_string) });
				if (input.content != null) fields.push({ label: 'CONTENT', value: String(input.content).length > 500 ? String(input.content).slice(0, 500) + '…' : String(input.content) });
				break;
			case 'Bash':
				if (input.command) fields.push({ label: 'COMMAND', value: String(input.command) });
				if (input.description) fields.push({ label: 'DESCRIPTION', value: String(input.description) });
				break;
			case 'Grep':
				if (input.pattern) fields.push({ label: 'PATTERN', value: String(input.pattern) });
				if (input.path) fields.push({ label: 'PATH', value: String(input.path), clickable: 'file' });
				if (input.glob) fields.push({ label: 'GLOB', value: String(input.glob) });
				break;
			case 'Glob':
				if (input.pattern) fields.push({ label: 'PATTERN', value: String(input.pattern), clickable: 'glob' });
				if (input.path) fields.push({ label: 'PATH', value: String(input.path), clickable: 'file' });
				break;
			case 'WebFetch':
				if (input.url) fields.push({ label: 'URL', value: String(input.url) });
				if (input.prompt) fields.push({ label: 'PROMPT', value: String(input.prompt) });
				break;
			case 'WebSearch':
				if (input.query) fields.push({ label: 'QUERY', value: String(input.query) });
				break;
			case 'Task':
				if (input.description) fields.push({ label: 'TASK', value: String(input.description) });
				if (input.prompt) fields.push({ label: 'PROMPT', value: String(input.prompt).length > 500 ? String(input.prompt).slice(0, 500) + '…' : String(input.prompt) });
				break;
			default: {
				// Unknown tool: show all input keys
				for (const [key, val] of Object.entries(input)) {
					const str = typeof val === 'string' ? val : JSON.stringify(val, null, 2);
					fields.push({ label: key.toUpperCase(), value: str.length > 500 ? str.slice(0, 500) + '…' : str });
				}
			}
		}
		return fields;
	}

	function handleFieldClick(field: { clickable?: 'file' | 'glob'; value: string }) {
		if (field.clickable === 'file') openFilePath(field.value);
		else if (field.clickable === 'glob') openExplorerWithSearch(field.value);
	}
</script>

<div class="cell-tools">
	{#if shouldCollapse && !toolsExpanded}
		<button class="tools-collapsed-toggle" onclick={() => toolsExpanded = true}>
			<span class="tools-count">{toolCount} tool operations</span>
			<span class="tools-expand-icon">&#9654;</span>
		</button>
	{:else}
		<div class="badges-row">
			{#each toolCells as tool}
				{@const isFileOp = tool.name === 'Read' || tool.name === 'Edit' || tool.name === 'Write'}
				{@const filePath = isFileOp && tool.input?.file_path ? String(tool.input.file_path) : null}
				{@const globPattern = tool.name === 'Glob' && tool.input?.pattern ? String(tool.input.pattern) : null}
				{@const isExpanded = expandedToolId === tool.id}
				<button
					class="tool-badge"
					class:file-tool={!!filePath || !!globPattern}
					class:expanded={isExpanded}
					onclick={() => toggleTool(tool.id)}
					title={filePath ?? globPattern ?? getToolDetail(tool.name, tool.input) ?? tool.name}
				>
					<span class="tool-icon">{getToolIcon(tool.name)}</span>
					<span class="tool-name">{tool.name}</span>
					{#if filePath}
						<span class="tool-file">{filePath.split('/').pop()}</span>
					{:else if globPattern}
						<span class="tool-detail">{globPattern.length > 40 ? globPattern.slice(0, 40) + '…' : globPattern}</span>
					{:else}
						{@const detail = getToolDetail(tool.name, tool.input)}
						{#if detail}
							<span class="tool-detail">{detail}</span>
						{/if}
					{/if}
					{#if isExpanded}
						<span class="expand-indicator">▼</span>
					{/if}
				</button>
			{/each}
			{#if shouldCollapse}
				<button class="tools-collapse-toggle" onclick={() => { toolsExpanded = false; expandedToolId = null; }}>
					&#9650; Collapse
				</button>
			{/if}
		</div>

		<!-- Expanded tool detail panel -->
		{#if expandedTool}
			{@const fields = getExpandedInputFields(expandedTool)}
			<div class="tool-detail-panel" class:error-panel={expandedTool.is_error}>
				{#if fields.length > 0}
					<div class="detail-section">
						{#each fields as field}
							<div class="detail-field">
								<span class="detail-label">{field.label}</span>
								{#if field.clickable}
									<button class="detail-value clickable" onclick={() => handleFieldClick(field)}>
										{field.value}
										<span class="open-arrow">&rarr;</span>
									</button>
								{:else}
									<pre class="detail-value">{field.value}</pre>
								{/if}
							</div>
						{/each}
					</div>
				{/if}
				{#if expandedTool.output}
					<div class="detail-section output-section" class:error-output={expandedTool.is_error}>
						<span class="detail-label">{expandedTool.is_error ? 'ERROR' : 'OUTPUT'}</span>
						<pre class="detail-output">{expandedTool.output}</pre>
					</div>
				{/if}
				{#if expandedTool.name === 'Task' && agentLog && agentLog.length > 0}
					<div class="detail-section agent-log-section">
						<span class="detail-label">AGENT LOG</span>
						<div class="agent-log">
							{#each agentLog as entry}
								<div class="agent-log-entry" class:agent-response={entry.role === 'agent_assistant'}>
									<span class="agent-log-arrow">{entry.role === 'agent_assistant' ? '←' : '→'}</span>
									<span class="agent-log-content">{entry.content}</span>
								</div>
							{/each}
						</div>
					</div>
				{/if}
			</div>
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
		flex-direction: column;
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

	.badges-row {
		display: flex;
		flex-wrap: wrap;
		gap: 6px;
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
		text-align: left;
	}

	.tools-collapse-toggle:hover {
		opacity: 1;
		color: var(--amber-400);
	}

	/* File path label on file operation tools */
	.tool-file,
	.tool-detail {
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
		cursor: pointer;
		transition: all 0.15s ease;
		font-family: inherit;
	}

	.tool-badge:hover {
		background: var(--surface-600);
		border-color: var(--amber-600);
		box-shadow: var(--elevation-low);
	}

	.tool-badge:focus {
		outline: none;
		box-shadow: 0 0 0 2px var(--tint-selection);
	}

	.tool-badge.expanded {
		border-color: var(--amber-500);
		background: var(--surface-600);
		text-shadow: var(--emphasis);
	}

	/* File operation tool badges */
	.tool-badge.file-tool {
		border-left: 2px solid var(--amber-500);
	}

	.tool-badge.file-tool:hover {
		border-left-color: var(--amber-400);
		text-shadow: var(--emphasis);
	}

	.expand-indicator {
		font-size: 8px;
		opacity: 0.7;
		margin-left: 2px;
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

	/* ── Expanded detail panel ─────────────────────────── */

	.tool-detail-panel {
		border: 1px solid var(--amber-600);
		border-radius: 3px;
		background: var(--surface-800);
		overflow: hidden;
		animation: panel-on 0.3s ease-out;
	}

	.tool-detail-panel.error-panel {
		border-color: #dc2626;
	}

	.detail-section {
		padding: 10px 12px;
	}

	.detail-section + .detail-section {
		border-top: 1px solid var(--surface-border);
	}

	.detail-field {
		margin-bottom: 6px;
	}

	.detail-field:last-child {
		margin-bottom: 0;
	}

	.detail-label {
		display: block;
		font-size: 9px;
		font-weight: 700;
		text-transform: uppercase;
		letter-spacing: 0.15em;
		color: var(--amber-400);
		margin-bottom: 3px;
	}

	pre.detail-value {
		margin: 0;
		white-space: pre-wrap;
		word-break: break-all;
		font-family: inherit;
		font-size: 11px;
		line-height: 1.5;
		color: var(--text-primary);
	}

	button.detail-value.clickable {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		background: none;
		border: none;
		padding: 0;
		font-family: inherit;
		font-size: 11px;
		color: var(--amber-400);
		cursor: pointer;
		text-decoration: underline;
		text-decoration-style: dotted;
		text-underline-offset: 3px;
	}

	button.detail-value.clickable:hover {
		color: var(--amber-300);
		text-shadow: var(--emphasis);
	}

	.open-arrow {
		font-size: 10px;
		opacity: 0.6;
	}

	.output-section {
		background: var(--surface-700);
		position: relative;
	}

	.output-section.error-output {
		border-top-color: #dc2626;
	}

	.output-section.error-output .detail-label {
		color: #ef4444;
	}

	.detail-output {
		margin: 0;
		white-space: pre-wrap;
		word-break: break-all;
		font-family: inherit;
		font-size: 11px;
		line-height: 1.5;
		color: var(--text-primary);
		max-height: 300px;
		overflow-y: auto;
	}

	.output-section.error-output .detail-output {
		color: #fca5a5;
	}

	/* Scanline overlay on output — denser than main display */
	.output-section::after {
		content: '';
		position: absolute;
		inset: 0;
		background: repeating-linear-gradient(0deg, transparent, transparent 2px, rgba(0,0,0,0.03) 2px, rgba(0,0,0,0.03) 4px);
		pointer-events: none;
		border-radius: 2px;
	}

	/* ── Agent log (sub-agent activity inside Task) ─── */

	.agent-log-section {
		background: var(--surface-700);
	}

	.agent-log {
		max-height: 300px;
		overflow-y: auto;
	}

	.agent-log-entry {
		display: flex;
		gap: 6px;
		padding: 3px 0;
		font-size: 11px;
		line-height: 1.5;
		color: var(--text-primary);
	}

	.agent-log-entry + .agent-log-entry {
		border-top: 1px solid var(--surface-border);
	}

	.agent-log-arrow {
		flex-shrink: 0;
		color: var(--amber-400);
		font-size: 10px;
		width: 14px;
		text-align: center;
	}

	.agent-log-entry.agent-response .agent-log-arrow {
		color: var(--text-muted);
	}

	.agent-log-content {
		white-space: pre-wrap;
		word-break: break-word;
	}

	@keyframes panel-on {
		0% { opacity: 0; filter: brightness(3); }
		30% { opacity: 0.5; filter: brightness(2); }
		60% { opacity: 0.8; filter: brightness(1.2); }
		100% { opacity: 1; filter: brightness(1); }
	}

	/* Mobile */
	@media (max-width: 639px) {
		.cell-tools {
			margin-top: 10px;
			gap: 4px;
		}

		.badges-row {
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

		.detail-output {
			max-height: 200px;
			font-size: 10px;
		}

		.tool-detail-panel {
			margin-top: 2px;
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

	:global([data-theme="analog"]) .tool-detail-panel {
		background-color: var(--surface-800);
		background-image: var(--grain-fine);
		background-blend-mode: multiply;
		border-color: var(--surface-border);
	}

	:global([data-theme="analog"]) .output-section::after {
		display: none;
	}

	:global([data-theme="analog"]) .tool-detail-panel {
		animation: ink-bleed 0.5s cubic-bezier(0.1, 0.9, 0.2, 1);
	}

	@keyframes ink-bleed {
		0% { opacity: 0; transform: scaleY(0.95); }
		100% { opacity: 1; transform: scaleY(1); }
	}
</style>
