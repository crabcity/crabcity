<script lang="ts">
	import Terminal from '../Terminal.svelte';
	import ErrorBoundary from '../ErrorBoundary.svelte';

	interface Props {
		instanceId: string;
	}

	let { instanceId }: Props = $props();
</script>

<div class="pane-terminal">
	<ErrorBoundary>
		{#snippet children()}
			{#key 'terminal-' + instanceId}
				<Terminal {instanceId} />
			{/key}
		{/snippet}
	</ErrorBoundary>
</div>

<style>
	.pane-terminal {
		flex: 1;
		min-height: 0;
	}
</style>
