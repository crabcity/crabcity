<script lang="ts">
	import FileExplorer from '../FileExplorer.svelte';

	interface Props {
		instanceId: string | null;
	}

	let { instanceId }: Props = $props();
</script>

<div class="pane-file-explorer">
	<FileExplorer embedded={true} />
</div>

<style>
	.pane-file-explorer {
		width: 100%;
		height: 100%;
		overflow: hidden;
	}
</style>
