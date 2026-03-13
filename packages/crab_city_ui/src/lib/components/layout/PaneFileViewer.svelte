<script lang="ts">
	import FileViewer from '../FileViewer.svelte';
</script>

<div class="pane-file-viewer">
	<FileViewer embedded={true} />
</div>

<style>
	.pane-file-viewer {
		width: 100%;
		height: 100%;
		overflow: hidden;
	}
</style>
