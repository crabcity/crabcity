<script lang="ts">
	import type { Instance } from '$lib/types';
	import type { StateInfo } from '$lib/utils/instance-state';

	interface Props {
		instance: Instance;
		isFocused: boolean;
		stateInfo: StateInfo;
		onclick: () => void;
	}

	let { instance, isFocused, stateInfo, onclick }: Props = $props();

	const displayName = $derived(
		(instance.custom_name ?? instance.name).slice(0, 14)
	);
</script>

<!-- svelte-ignore a11y_click_events_have_key_events -->
<button
	class="instance-chip"
	class:focused={isFocused}
	class:active={stateInfo.animate}
	{onclick}
	title={instance.custom_name ?? instance.name}
	aria-label="Focus {instance.custom_name ?? instance.name}"
	aria-pressed={isFocused}
>
	<span
		class="chip-led"
		style="background: {stateInfo.color}"
		class:pulse={stateInfo.animate}
	></span>
	<span class="chip-name">{displayName}</span>
	{#if stateInfo.label}
		<span class="chip-state" class:stale={stateInfo.stale}>{stateInfo.label}</span>
	{/if}
</button>

<style>
	.instance-chip {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		padding: 4px 10px;
		background: var(--surface-600);
		border: 1px solid var(--surface-border);
		border-radius: 3px;
		cursor: pointer;
		transition: all 0.15s ease;
		white-space: nowrap;
		flex-shrink: 0;
		font-family: inherit;
		color: var(--text-secondary);
	}

	.instance-chip:hover {
		background: var(--surface-500);
		border-color: var(--surface-border-light);
	}

	.instance-chip.focused {
		border-color: var(--amber-600);
		background: var(--tint-active);
		color: var(--amber-400);
	}

	.chip-led {
		width: 6px;
		height: 6px;
		border-radius: 50%;
		flex-shrink: 0;
	}

	.chip-led.pulse {
		animation: led-pulse 0.8s ease-in-out infinite;
	}

	@keyframes led-pulse {
		0%, 100% { opacity: 1; }
		50% { opacity: 0.4; }
	}

	@media (prefers-reduced-motion: reduce) {
		.chip-led.pulse { animation: none; }
	}

	.chip-name {
		font-size: 10px;
		font-weight: 600;
		letter-spacing: 0.05em;
		text-transform: uppercase;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.chip-state {
		font-size: 9px;
		font-weight: 600;
		letter-spacing: 0.03em;
		color: var(--text-muted);
		opacity: 0.8;
	}

	.chip-state.stale {
		font-style: italic;
		opacity: 0.5;
	}
</style>
