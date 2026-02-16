<script lang="ts">
	import { goto } from '$app/navigation';
	import { base } from '$app/paths';
	import { page } from '$app/stores';
	import { onMount } from 'svelte';

	onMount(() => {
		// Preserve invite token if present
		const invite = $page.url.searchParams.get('invite');
		const hash = invite ? `#${invite}` : '';
		goto(`${base}/join${hash}`, { replaceState: true });
	});
</script>

<div class="auth-page">
	<p>Redirecting...</p>
</div>

<style>
	.auth-page {
		display: flex;
		align-items: center;
		justify-content: center;
		height: 100vh;
		background: var(--surface-900);
	}
	p { color: var(--text-muted); font-size: 0.85em; }
</style>
