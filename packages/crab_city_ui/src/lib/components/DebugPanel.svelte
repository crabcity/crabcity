<script lang="ts">
	/**
	 * DebugPanel - Performance metrics overlay
	 *
	 * Toggle with Ctrl+Shift+D (or Cmd+Shift+D on Mac).
	 * Shows real-time metrics for debugging performance issues.
	 */

	import { metrics, debugPanelVisible, avatarHitRate } from '$lib/stores/metrics';
</script>

{#if $debugPanelVisible}
	<div class="debug-panel">
		<h4>Performance Metrics</h4>

		<section>
			<h5>VirtualList</h5>
			<div class="metric">
				<span class="label">Items</span>
				<span class="value">{$metrics.virtualList.renderedItems}/{$metrics.virtualList.totalItems}</span>
			</div>
			<div class="metric">
				<span class="label">Height cache</span>
				<span class="value">{$metrics.virtualList.heightCacheSize}</span>
			</div>
			<div class="metric">
				<span class="label">Last render</span>
				<span class="value">{$metrics.virtualList.lastRenderMs.toFixed(1)}ms</span>
			</div>
		</section>

		<section>
			<h5>Avatar Cache</h5>
			<div class="metric">
				<span class="label">Size</span>
				<span class="value">{$metrics.avatar.cacheSize}/100</span>
			</div>
			<div class="metric">
				<span class="label">Hit rate</span>
				<span class="value">{($avatarHitRate * 100).toFixed(1)}%</span>
			</div>
		</section>

		<section>
			<h5>Terminal Buffers</h5>
			<div class="metric">
				<span class="label">Instances</span>
				<span class="value">{$metrics.terminal.bufferCount}</span>
			</div>
			<div class="metric">
				<span class="label">Total chunks</span>
				<span class="value">{$metrics.terminal.totalChunks}</span>
			</div>
			<div class="metric">
				<span class="label">Near capacity</span>
				<span class="value" class:warning={$metrics.terminal.nearCapacityCount > 0}>
					{$metrics.terminal.nearCapacityCount}
				</span>
			</div>
		</section>

		<section>
			<h5>WebSocket</h5>
			<div class="metric">
				<span class="label">Messages/sec</span>
				<span class="value">{$metrics.websocket.messagesPerSecond.toFixed(1)}</span>
			</div>
			<div class="metric">
				<span class="label">Total received</span>
				<span class="value">{$metrics.websocket.messagesReceived}</span>
			</div>
			<div class="metric">
				<span class="label">Reconnects</span>
				<span class="value" class:warning={$metrics.websocket.reconnectCount > 0}>
					{$metrics.websocket.reconnectCount}
				</span>
			</div>
		</section>

		<div class="hint">Press Ctrl+Shift+D to close</div>
	</div>
{/if}

<style>
	.debug-panel {
		position: fixed;
		bottom: 1rem;
		right: 1rem;
		background: var(--surface-900, #1a1a1a);
		border: 1px solid var(--amber-600, #d97706);
		border-radius: 4px;
		padding: 12px;
		font-size: 11px;
		font-family: var(--font-mono, monospace);
		z-index: 9999;
		min-width: 200px;
		max-width: 280px;
		box-shadow: 0 4px 20px rgba(0, 0, 0, 0.5);
	}

	h4 {
		margin: 0 0 8px;
		font-size: 11px;
		font-weight: 700;
		letter-spacing: 0.1em;
		text-transform: uppercase;
		color: var(--amber-400, #fbbf24);
	}

	section {
		margin-bottom: 10px;
		padding-bottom: 8px;
		border-bottom: 1px solid var(--surface-border, #333);
	}

	section:last-of-type {
		border-bottom: none;
		margin-bottom: 8px;
	}

	h5 {
		margin: 0 0 4px;
		font-size: 10px;
		font-weight: 600;
		letter-spacing: 0.05em;
		text-transform: uppercase;
		color: var(--amber-500, #f59e0b);
	}

	.metric {
		display: flex;
		justify-content: space-between;
		padding: 2px 0;
	}

	.label {
		color: var(--text-muted, #666);
	}

	.value {
		color: var(--text-secondary, #999);
		font-weight: 500;
	}

	.value.warning {
		color: var(--red-400, #f87171);
	}

	.hint {
		font-size: 9px;
		color: var(--text-muted, #666);
		text-align: center;
		margin-top: 4px;
	}
</style>
