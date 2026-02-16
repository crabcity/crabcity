<script lang="ts">
	import { page } from '$app/stores';
	import { goto } from '$app/navigation';
	import { base } from '$app/paths';

	$effect(() => {
		const token = $page.params.token;
		// Redirect old invite URLs to the new join flow
		goto(`${base}/join#${token ?? ''}`, { replaceState: true });
	});
</script>

<div class="auth-page">
	<div class="auth-card">
		<p>Redirecting to join page...</p>
	</div>
</div>

<style>
	.auth-page {
		display: flex;
		align-items: center;
		justify-content: center;
		height: 100vh;
		background: var(--surface-900);
	}

	.auth-card {
		width: 100%;
		max-width: 360px;
		padding: 32px;
		background: var(--surface-700);
		border: 1px solid var(--surface-border);
		border-radius: 8px;
		text-align: center;
	}

	p {
		color: var(--text-muted);
		font-size: 0.85em;
	}
</style>
