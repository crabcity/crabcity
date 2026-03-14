<script lang="ts">
	import { onMount } from 'svelte';
	import { userSettings, toggleTheme } from '$lib/stores/settings';
	import { layoutState, splitPane, setPaneContent, focusPane } from '$lib/stores/layout';

	interface Props {
		onclose: () => void;
	}

	let { onclose }: Props = $props();

	let popoverEl: HTMLDivElement | undefined = $state();

	function handleOpenSettings() {
		// Find the focused pane and set it to settings, or split
		const state = $layoutState;
		const focusedId = state.focusedPaneId;
		const pane = state.panes.get(focusedId);

		if (pane && pane.content.kind === 'landing') {
			// Replace landing with settings
			setPaneContent(focusedId, { kind: 'settings' });
		} else if (pane) {
			// Split focused pane and open settings in the new pane
			splitPane(focusedId, 'vertical', { kind: 'settings' });
		}
		onclose();
	}

	function handleKeydown(e: KeyboardEvent) {
		if (e.key === 'Escape') {
			onclose();
		}
	}

	function handleClickOutside(e: MouseEvent) {
		if (popoverEl && !popoverEl.contains(e.target as Node)) {
			onclose();
		}
	}

	onMount(() => {
		// Delay adding click listener to avoid immediate close
		const timer = setTimeout(() => {
			document.addEventListener('click', handleClickOutside, true);
		}, 0);

		document.addEventListener('keydown', handleKeydown);

		return () => {
			clearTimeout(timer);
			document.removeEventListener('click', handleClickOutside, true);
			document.removeEventListener('keydown', handleKeydown);
		};
	});
</script>

<div class="quick-settings" bind:this={popoverEl}>
	<div class="qs-header">QUICK SETTINGS</div>

	<div class="qs-row">
		<span class="qs-label">Theme</span>
		<button
			class="qs-toggle"
			class:active={$userSettings.theme === 'analog'}
			onclick={() => toggleTheme()}
		>
			<span class="qs-opt" class:selected={$userSettings.theme === 'phosphor'}>Phosphor</span>
			<span class="qs-div">/</span>
			<span class="qs-opt" class:selected={$userSettings.theme === 'analog'}>Analog</span>
		</button>
	</div>

	<div class="qs-separator"></div>

	<button class="qs-open-btn" onclick={handleOpenSettings}>
		Open Settings
	</button>
</div>

<style>
	.quick-settings {
		position: fixed;
		left: 56px;
		bottom: 80px;
		width: 200px;
		background: var(--surface-700);
		border: 1px solid var(--surface-border);
		border-radius: 6px;
		padding: 8px;
		z-index: 1000;
		box-shadow: 0 4px 16px rgba(0, 0, 0, 0.4);
	}

	.qs-header {
		font-size: 9px;
		font-weight: 700;
		letter-spacing: 0.12em;
		color: var(--text-muted);
		padding: 4px 4px 8px;
	}

	.qs-row {
		display: flex;
		align-items: center;
		justify-content: space-between;
		padding: 4px;
	}

	.qs-label {
		font-size: 11px;
		color: var(--text-secondary);
		font-weight: 600;
		letter-spacing: 0.03em;
	}

	.qs-toggle {
		display: flex;
		align-items: center;
		gap: 3px;
		background: var(--surface-600);
		border: 1px solid var(--surface-border);
		border-radius: 3px;
		padding: 3px 6px;
		cursor: pointer;
		font-family: inherit;
		font-size: 10px;
		letter-spacing: 0.05em;
		transition: border-color 0.15s ease;
	}

	.qs-toggle:hover {
		border-color: var(--amber-600);
	}

	.qs-opt {
		color: var(--text-muted);
		transition: color 0.15s ease;
	}

	.qs-opt.selected {
		color: var(--amber-400);
		font-weight: 700;
	}

	.qs-div {
		color: var(--text-muted);
		opacity: 0.3;
	}

	.qs-separator {
		height: 1px;
		background: var(--surface-border);
		margin: 6px 4px;
	}

	.qs-open-btn {
		display: block;
		width: 100%;
		font-size: 11px;
		font-weight: 600;
		font-family: inherit;
		letter-spacing: 0.05em;
		color: var(--text-secondary);
		background: transparent;
		border: 1px solid var(--surface-border);
		border-radius: 3px;
		padding: 5px 8px;
		cursor: pointer;
		text-align: center;
		transition: all 0.15s ease;
	}

	.qs-open-btn:hover {
		background: var(--tint-hover);
		border-color: var(--amber-600);
		color: var(--amber-400);
	}
</style>
