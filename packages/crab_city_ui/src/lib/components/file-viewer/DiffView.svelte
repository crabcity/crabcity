<script lang="ts">
	import type { InlineHighlight } from '$lib/stores/git';

	interface DiffLine {
		type: string;
		oldNum?: number | null;
		newNum?: number | null;
		content: string;
		highlights?: InlineHighlight[];
	}

	interface DiffHunk {
		header: string;
		lines: DiffLine[];
	}

	interface DiffData {
		additions: number;
		deletions: number;
		hunks: DiffHunk[];
	}

	interface Props {
		diffData: DiffData;
		diffEngine: string;
		actualEngine?: string;
		refreshStatus: 'idle' | 'loading' | 'done';
		showRefreshOverlay: boolean;
		onengineToggle: () => void;
	}

	let {
		diffData,
		diffEngine,
		actualEngine,
		refreshStatus,
		showRefreshOverlay,
		onengineToggle
	}: Props = $props();

	function splitByHighlights(content: string, highlights?: InlineHighlight[]): Array<{ text: string; highlighted: boolean }> {
		if (!highlights || highlights.length === 0) {
			return [{ text: content, highlighted: false }];
		}
		const segments: Array<{ text: string; highlighted: boolean }> = [];
		let pos = 0;
		for (const hl of highlights) {
			if (hl.start > pos) {
				segments.push({ text: content.slice(pos, hl.start), highlighted: false });
			}
			segments.push({ text: content.slice(hl.start, hl.end), highlighted: true });
			pos = hl.end;
		}
		if (pos < content.length) {
			segments.push({ text: content.slice(pos), highlighted: false });
		}
		return segments;
	}
</script>

<div class="diff-view" class:refreshing={showRefreshOverlay}>
	{#if showRefreshOverlay}
		<div class="diff-refresh-bar"></div>
	{/if}
	<div class="diff-stats-bar">
		<span class="diff-stat additions">+{diffData.additions}</span>
		<span class="diff-stat deletions">-{diffData.deletions}</span>
		<button
			class="engine-toggle"
			class:active={diffEngine !== 'standard'}
			onclick={onengineToggle}
			disabled={refreshStatus === 'loading'}
			title={diffEngine === 'structural' ? 'Using structural diff (syntax-aware)' : diffEngine === 'patience' ? 'Using patience diff (word-level)' : 'Using standard diff'}
		>
			{diffEngine}
		</button>
		{#if diffEngine === 'structural' && actualEngine === 'patience'}
			<span class="engine-fallback" title="Structural diff unavailable for this file — using patience">fell back to patience</span>
		{/if}
		<span class="refresh-indicator" class:loading={refreshStatus === 'loading'} class:done={refreshStatus === 'done'}></span>
	</div>
	{#each diffData.hunks as hunk}
		<div class="diff-hunk-header">{hunk.header}</div>
		{#each hunk.lines as line}
			<div class="diff-line {line.type}">
				<span class="line-num old">{line.oldNum ?? ''}</span>
				<span class="line-num new">{line.newNum ?? ''}</span>
				<span class="line-marker">{line.type === 'add' ? '+' : line.type === 'del' ? '-' : ' '}</span>
				<span class="line-content">{#each splitByHighlights(line.content, line.highlights) as seg}{#if seg.highlighted}<mark class="inline-hl">{seg.text}</mark>{:else}{seg.text}{/if}{/each}</span>
			</div>
		{/each}
	{/each}
</div>

<style>
	.diff-view {
		font-family: inherit;
		font-size: 12px;
		line-height: 1.5;
		position: relative;
		transition: opacity 0.2s ease;
	}

	.diff-view.refreshing {
		opacity: 0.5;
		pointer-events: none;
	}

	.diff-refresh-bar {
		position: sticky;
		top: 0;
		left: 0;
		right: 0;
		height: 2px;
		z-index: 2;
		background: linear-gradient(
			90deg,
			transparent 0%,
			var(--amber-400) 40%,
			var(--amber-400) 60%,
			transparent 100%
		);
		background-size: 200% 100%;
		animation: shimmer 1.2s ease-in-out infinite;
	}

	@keyframes shimmer {
		0% { background-position: 100% 0; }
		100% { background-position: -100% 0; }
	}

	.diff-stats-bar {
		display: flex;
		gap: 12px;
		padding: 8px 16px;
		background: var(--surface-700);
		border-bottom: 1px solid var(--surface-border);
		font-size: 11px;
		font-weight: 600;
		font-family: inherit;
	}

	.diff-stat.additions {
		color: var(--amber-400);
		text-shadow: var(--emphasis);
	}

	.diff-stat.deletions {
		color: var(--text-muted);
	}

	.diff-hunk-header {
		padding: 6px 16px;
		background: var(--tint-hover);
		border-top: 1px solid var(--surface-border);
		border-bottom: 1px solid var(--surface-border);
		color: var(--amber-600);
		font-style: italic;
		font-size: 11px;
		user-select: none;
	}

	.diff-line {
		display: flex;
		align-items: stretch;
		min-height: 20px;
		border-left: 2px solid transparent;
	}

	.diff-line.add {
		background: var(--tint-active);
		border-left-color: var(--amber-500);
	}

	.diff-line.add .line-content {
		color: var(--amber-400);
	}

	.diff-line.del {
		background: var(--status-red-tint);
		border-left-color: var(--status-red-border);
	}

	.diff-line.del .line-content {
		color: var(--text-muted);
		text-decoration: line-through;
		text-decoration-color: var(--status-red-border);
	}

	.diff-line.ctx .line-content {
		color: var(--text-secondary);
	}

	.diff-line .line-num {
		display: inline-block;
		width: 4ch;
		padding: 0 4px;
		text-align: right;
		font-size: 10px;
		color: var(--text-muted);
		opacity: 0.5;
		user-select: none;
		flex-shrink: 0;
		font-variant-numeric: tabular-nums;
	}

	.diff-line .line-marker {
		display: inline-block;
		width: 2ch;
		text-align: center;
		flex-shrink: 0;
		font-weight: 700;
		user-select: none;
	}

	.diff-line.add .line-marker {
		color: var(--amber-400);
	}

	.diff-line.del .line-marker {
		color: var(--status-red-text);
	}

	.diff-line.ctx .line-marker {
		color: var(--text-muted);
		opacity: 0.3;
	}

	.diff-line .line-content {
		flex: 1;
		white-space: pre-wrap;
		word-break: break-all;
		padding-right: 16px;
	}

	/* Inline word-level highlights */
	.diff-line .line-content :global(.inline-hl) {
		background: none;
		border-radius: 2px;
		padding: 0 1px;
	}

	.diff-line.add .line-content :global(.inline-hl) {
		background: var(--tint-selection);
		color: var(--amber-300);
	}

	.diff-line.del .line-content :global(.inline-hl) {
		background: var(--status-red-strong);
		color: var(--status-red-text);
		text-decoration: line-through;
		text-decoration-color: var(--status-red-border);
	}

	/* Engine toggle */
	.engine-toggle {
		margin-left: auto;
		padding: 2px 8px;
		background: var(--surface-600);
		border: 1px solid var(--surface-border);
		border-radius: 3px;
		font-family: inherit;
		font-size: 9px;
		font-weight: 600;
		letter-spacing: 0.06em;
		text-transform: uppercase;
		color: var(--text-muted);
		cursor: pointer;
		transition: all 0.15s ease;
	}

	.engine-toggle:hover {
		background: var(--surface-500);
		color: var(--text-secondary);
		border-color: var(--amber-600);
	}

	.engine-toggle.active {
		color: var(--amber-400);
		border-color: var(--tint-selection);
	}

	.engine-fallback {
		font-size: 9px;
		color: var(--amber-400);
		opacity: 0.7;
		letter-spacing: 0.03em;
	}

	.refresh-indicator {
		width: 6px;
		height: 6px;
		border-radius: 50%;
		flex-shrink: 0;
		opacity: 0;
		transform: scale(0);
		transition: opacity 0.15s ease, transform 0.15s ease, background-color 0.2s ease;
	}

	.refresh-indicator.loading {
		opacity: 1;
		transform: scale(1);
		background: var(--amber-400);
		animation: indicator-pulse 0.6s ease-in-out infinite alternate;
	}

	.refresh-indicator.done {
		opacity: 1;
		transform: scale(1);
		background: var(--status-green-text);
		animation: indicator-fade 0.6s ease-out forwards;
	}

	@keyframes indicator-pulse {
		from { opacity: 0.4; }
		to { opacity: 1; }
	}

	@keyframes indicator-fade {
		0% { opacity: 1; transform: scale(1); }
		70% { opacity: 1; transform: scale(1); }
		100% { opacity: 0; transform: scale(0); }
	}
</style>
