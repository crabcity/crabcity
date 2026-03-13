<script lang="ts">
	import ConversationView from '../ConversationView.svelte';
	import ErrorBoundary from '../ErrorBoundary.svelte';

	interface Props {
		instanceId: string;
	}

	let { instanceId }: Props = $props();
</script>

<ErrorBoundary>
	{#snippet children()}
		{#key 'conversation-' + instanceId}
			<ConversationView {instanceId} />
		{/key}
	{/snippet}
</ErrorBoundary>
