<script lang="ts">
	import type { PaneState, PaneContentKind, PaneContent } from '$lib/stores/layout';
	import { paneCount, splitPane, closePane, setPaneContent, getPaneInstanceId, defaultContentForKind } from '$lib/stores/layout';
	import { instances, instanceList, currentInstanceId, createInstance, selectInstance } from '$lib/stores/instances';
	import { defaultCommand } from '$lib/stores/settings';
	import { currentProject } from '$lib/stores/projects';

	interface Props {
		pane: PaneState;
	}

	let { pane }: Props = $props();

	const canClose = $derived($paneCount > 1);

	// Whether this pane kind carries an instanceId
	const hasInstanceId = $derived(
		pane.content.kind === 'terminal' ||
		pane.content.kind === 'conversation' ||
		pane.content.kind === 'file-explorer' ||
		pane.content.kind === 'tasks' ||
		pane.content.kind === 'git'
	);

	const paneInstanceId = $derived(getPaneInstanceId(pane.content));

	// Terminal panes show only shell instances; other kinds show only Claude instances
	const filteredInstances = $derived(
		$instanceList.filter((inst) =>
			pane.content.kind === 'terminal'
				? !inst.command.includes('claude')
				: inst.command.includes('claude')
		)
	);

	// Instance status indicator for terminal/conversation panes
	const instanceStatus = $derived.by((): 'thinking' | 'responding' | 'tool' | 'idle' | null => {
		if (!paneInstanceId) return null;
		const kind = pane.content.kind;
		if (kind !== 'terminal' && kind !== 'conversation') return null;
		const inst = $instances.get(paneInstanceId);
		if (!inst) return null;
		const cs = inst.claude_state;
		if (!cs) return 'idle';
		if (cs.type === 'Thinking') return 'thinking';
		if (cs.type === 'Responding') return 'responding';
		if (cs.type === 'ToolExecuting') return 'tool';
		return 'idle';
	});

	const statusLabel = $derived.by(() => {
		if (instanceStatus === 'thinking') return 'Claude is thinking';
		if (instanceStatus === 'responding') return 'Claude is responding';
		if (instanceStatus === 'tool') return 'Claude is executing a tool';
		return null;
	});

	// File name for file-viewer chrome
	const fileViewerLabel = $derived.by(() => {
		if (pane.content.kind !== 'file-viewer') return null;
		const fp = pane.content.filePath;
		if (!fp) return 'No file';
		const name = fp.split('/').pop() ?? fp;
		return name.length > 20 ? name.slice(0, 20) + '\u2026' : name;
	});

	// Scope label for chat chrome
	const chatScopeLabel = $derived.by(() => {
		if (pane.content.kind !== 'chat') return null;
		return pane.content.scope === 'global' ? 'Global' : 'Instance';
	});

	function handleSplitVertical() {
		splitPane(pane.id, 'vertical');
	}

	function handleSplitHorizontal() {
		splitPane(pane.id, 'horizontal');
	}

	function handleClose() {
		closePane(pane.id);
	}

	function handleContentChange(e: Event) {
		const newKind = (e.target as HTMLSelectElement).value as PaneContentKind;
		if (newKind === 'terminal') {
			setPaneContent(pane.id, { kind: 'terminal', instanceId: null });
			return;
		}
		const instanceId = getPaneInstanceId(pane.content) ?? $currentInstanceId;
		setPaneContent(pane.id, defaultContentForKind(newKind, instanceId));
	}

	let isCreating = $state(false);

	async function handleInstanceChange(e: Event) {
		const select = e.target as HTMLSelectElement;
		const value = select.value;

		if (value === '__new__') {
			// Reset select to current value while creating
			select.value = paneInstanceId ?? '';
			if (isCreating) return;
			isCreating = true;
			const result = await createInstance({
				command: $defaultCommand,
				working_dir: $currentProject?.workingDir
			});
			if (result && 'instanceId' in pane.content) {
				setPaneContent(pane.id, { ...pane.content, instanceId: result.id });
				selectInstance(result.id);
			}
			isCreating = false;
			return;
		}

		const newId = value || null;
		if ('instanceId' in pane.content) {
			setPaneContent(pane.id, { ...pane.content, instanceId: newId });
		}
	}
</script>

<div class="pane-chrome">
	{#if instanceStatus && instanceStatus !== 'idle'}
		<span
			class="status-dot"
			class:thinking={instanceStatus === 'thinking'}
			class:responding={instanceStatus === 'responding'}
			class:tool={instanceStatus === 'tool'}
			title={statusLabel}
			role="status"
			aria-label={statusLabel}
		></span>
	{/if}
	<select
		class="pane-type-select"
		value={pane.content.kind}
		onchange={handleContentChange}
		aria-label="Pane content type"
	>
		<option value="terminal">Terminal</option>
		<option value="conversation">Conversation</option>
		<option value="file-explorer">Files</option>
		<option value="chat">Chat</option>
		<option value="tasks">Tasks</option>
		<option value="file-viewer">File Viewer</option>
		<option value="git">Git</option>
	</select>
	{#if hasInstanceId}
		<span class="chrome-sep">/</span>
		<select
			class="instance-select"
			value={paneInstanceId ?? ''}
			onchange={handleInstanceChange}
			aria-label="Instance"
			disabled={isCreating}
		>
			<option value="">None</option>
			{#each filteredInstances as inst}
				<option value={inst.id}>{inst.custom_name ?? inst.name}</option>
			{/each}
			<option value="__new__">+ New</option>
		</select>
	{:else if pane.content.kind === 'file-viewer'}
		<span class="chrome-sep">/</span>
		<span class="chrome-label">{fileViewerLabel}</span>
	{:else if pane.content.kind === 'chat'}
		<span class="chrome-sep">/</span>
		<span class="chrome-label">{chatScopeLabel}</span>
	{/if}
	<div class="pane-spacer"></div>
	<div class="pane-actions">
		<button
			class="chrome-btn"
			onclick={handleSplitVertical}
			title="Split vertical (Cmd+\)"
			aria-label="Split pane vertically"
		>
			<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5">
				<rect x="1" y="1" width="14" height="14" rx="1" />
				<line x1="8" y1="1" x2="8" y2="15" />
			</svg>
		</button>
		<button
			class="chrome-btn"
			onclick={handleSplitHorizontal}
			title="Split horizontal (Cmd+-)"
			aria-label="Split pane horizontally"
		>
			<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5">
				<rect x="1" y="1" width="14" height="14" rx="1" />
				<line x1="1" y1="8" x2="15" y2="8" />
			</svg>
		</button>
		{#if canClose}
			<button
				class="chrome-btn close"
				onclick={handleClose}
				title="Close pane (Cmd+W)"
				aria-label="Close pane"
			>
				<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5">
					<line x1="4" y1="4" x2="12" y2="12" />
					<line x1="12" y1="4" x2="4" y2="12" />
				</svg>
			</button>
		{/if}
	</div>
</div>

<style>
	.pane-chrome {
		display: flex;
		align-items: center;
		height: 24px;
		padding: 0 8px;
		background: var(--surface-700);
		border-bottom: 1px solid var(--surface-border);
		flex-shrink: 0;
		gap: 4px;
	}

	.status-dot {
		width: 6px;
		height: 6px;
		border-radius: 50%;
		flex-shrink: 0;
		animation: dot-pulse 0.8s ease-in-out infinite;
	}

	.status-dot.thinking {
		background: var(--purple-500);
		box-shadow: 0 0 4px var(--purple-glow);
	}

	.status-dot.responding,
	.status-dot.tool {
		background: var(--amber-500);
		box-shadow: 0 0 4px var(--amber-glow);
	}

	@keyframes dot-pulse {
		0%, 100% { opacity: 1; }
		50% { opacity: 0.5; }
	}

	@media (prefers-reduced-motion: reduce) {
		.status-dot {
			animation: none;
		}
	}

	.pane-type-select {
		font-size: 10px;
		font-weight: 600;
		letter-spacing: 0.08em;
		text-transform: uppercase;
		color: var(--text-muted);
		background: transparent;
		border: none;
		cursor: pointer;
		font-family: inherit;
		padding: 0;
		outline: none;
		appearance: none;
		-webkit-appearance: none;
	}

	.pane-type-select:hover {
		color: var(--text-secondary);
	}

	.pane-type-select option {
		background: var(--surface-600);
		color: var(--text-primary);
		text-transform: none;
		letter-spacing: normal;
	}

	.chrome-sep {
		color: var(--text-muted);
		opacity: 0.3;
		font-size: 10px;
		flex-shrink: 0;
	}

	.instance-select {
		font-size: 10px;
		font-weight: 600;
		letter-spacing: 0.05em;
		color: var(--text-secondary);
		background: transparent;
		border: none;
		cursor: pointer;
		font-family: inherit;
		padding: 0;
		outline: none;
		appearance: none;
		-webkit-appearance: none;
		max-width: 120px;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.instance-select:hover {
		color: var(--amber-400);
	}

	.instance-select option {
		background: var(--surface-600);
		color: var(--text-primary);
		letter-spacing: normal;
	}

	.chrome-label {
		font-size: 10px;
		font-weight: 600;
		color: var(--text-muted);
		letter-spacing: 0.05em;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		max-width: 120px;
	}

	.pane-spacer {
		flex: 1;
	}

	.pane-actions {
		display: flex;
		gap: 2px;
		flex-shrink: 0;
	}

	.chrome-btn {
		display: flex;
		align-items: center;
		justify-content: center;
		width: 18px;
		height: 18px;
		padding: 0;
		background: transparent;
		border: none;
		border-radius: 2px;
		color: var(--text-muted);
		cursor: pointer;
		transition: all 0.1s ease;
	}

	.chrome-btn:hover {
		background: var(--tint-hover);
		color: var(--text-secondary);
	}

	.chrome-btn.close:hover {
		background: var(--status-red-tint);
		color: var(--status-red);
	}

	.chrome-btn svg {
		width: 12px;
		height: 12px;
	}
</style>
