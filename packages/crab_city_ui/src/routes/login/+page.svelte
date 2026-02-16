<script lang="ts">
	import { goto } from '$app/navigation';
	import { base } from '$app/paths';
	import { onDestroy } from 'svelte';
	import { currentIdentity, authError } from '$lib/stores/auth';
	import {
		initMultiplexedConnection,
		setPendingPasswordAuth,
	} from '$lib/stores/websocket';

	let username = $state('');
	let password = $state('');
	let error = $state('');
	let loggingIn = $state(false);
	let timeoutHandle: ReturnType<typeof setTimeout> | null = null;
	let identityUnsubscribe: (() => void) | null = null;

	// Surface auth errors from the store
	$effect(() => {
		const err = $authError;
		if (err && loggingIn) {
			error = err;
			loggingIn = false;
		}
	});

	onDestroy(() => {
		cleanup();
	});

	function cleanup() {
		identityUnsubscribe?.();
		identityUnsubscribe = null;
		if (timeoutHandle) {
			clearTimeout(timeoutHandle);
			timeoutHandle = null;
		}
	}

	function handleLogin(e: Event) {
		e.preventDefault();
		error = '';

		if (!username.trim()) {
			error = 'Username is required';
			return;
		}
		if (!password.trim()) {
			error = 'Password is required';
			return;
		}

		loggingIn = true;

		setPendingPasswordAuth({
			username: username.trim(),
			password,
		});

		identityUnsubscribe = currentIdentity.subscribe(($id) => {
			if ($id && loggingIn) {
				console.log('[Login] Authenticated as', $id.fingerprint);
				loggingIn = false;
				cleanup();
				goto(`${base}/`, { replaceState: true });
			}
		});

		initMultiplexedConnection();

		timeoutHandle = setTimeout(() => {
			if (loggingIn) {
				loggingIn = false;
				error = 'Connection timed out. Check your credentials and try again.';
				cleanup();
			}
		}, 15000);
	}
</script>

<div class="login-page">
	<div class="login-card">
		<div class="header">
			<h1>CRAB CITY</h1>
			<div class="divider"></div>
			<p class="subtitle">LOG IN</p>
		</div>

		{#if error}
			<div class="error">{error}</div>
		{/if}

		<form onsubmit={handleLogin}>
			<label>
				<span class="field-label">USERNAME</span>
				<input
					type="text"
					bind:value={username}
					placeholder="Your username"
					required
					autocomplete="username"
					disabled={loggingIn}
				/>
			</label>

			<label>
				<span class="field-label">PASSWORD</span>
				<input
					type="password"
					bind:value={password}
					placeholder="Your password"
					required
					autocomplete="current-password"
					disabled={loggingIn}
				/>
			</label>

			<button type="submit" class="login-btn" disabled={loggingIn}>
				{#if loggingIn}
					LOGGING IN...
				{:else}
					LOG IN
				{/if}
			</button>
		</form>

		<p class="footer-link">
			New here? <a href="{base}/join">Join with an invite</a>
		</p>
	</div>
</div>

<style>
	.login-page {
		display: flex;
		align-items: center;
		justify-content: center;
		min-height: 100vh;
		background: var(--surface-900);
		padding: 16px;
	}

	.login-card {
		width: 100%;
		max-width: 400px;
		padding: 32px 28px;
		background: linear-gradient(180deg, var(--surface-700) 0%, var(--surface-800) 100%);
		border: 1px solid var(--surface-border);
		border-radius: 4px;
		box-shadow: var(--elevation-high);
	}

	.header {
		text-align: center;
		margin-bottom: 24px;
	}

	h1 {
		margin: 0;
		font-size: 1.1em;
		font-weight: 700;
		letter-spacing: 0.2em;
		color: var(--amber-400);
		text-shadow: var(--emphasis);
	}

	.divider {
		width: 40px;
		height: 1px;
		margin: 10px auto;
		background: var(--amber-600);
		opacity: 0.5;
	}

	.subtitle {
		margin: 0;
		font-size: 0.7em;
		font-weight: 700;
		letter-spacing: 0.15em;
		color: var(--text-muted);
	}

	.error {
		padding: 8px 12px;
		margin-bottom: 16px;
		background: rgba(239, 68, 68, 0.12);
		border: 1px solid rgba(239, 68, 68, 0.25);
		border-radius: 3px;
		color: var(--status-red);
		font-size: 0.8em;
	}

	form {
		display: flex;
		flex-direction: column;
		gap: 14px;
	}

	label {
		display: flex;
		flex-direction: column;
		gap: 4px;
	}

	.field-label {
		font-size: 0.65em;
		font-weight: 700;
		letter-spacing: 0.12em;
		color: var(--text-muted);
	}

	input {
		padding: 8px 12px;
		background: var(--surface-900);
		border: 1px solid var(--surface-border);
		border-radius: 3px;
		color: var(--text-primary);
		font-family: inherit;
		font-size: 0.85em;
		transition: border-color 0.15s ease;
	}

	input:focus {
		outline: none;
		border-color: var(--amber-600);
		box-shadow: var(--elevation-low);
	}

	input::placeholder {
		color: var(--text-muted);
	}

	.login-btn {
		padding: 10px;
		margin-top: 4px;
		background: var(--amber-600);
		border: none;
		border-radius: 3px;
		color: var(--surface-900);
		font-family: inherit;
		font-weight: 700;
		font-size: 0.78em;
		letter-spacing: 0.1em;
		cursor: pointer;
		transition: all 0.15s ease;
	}

	.login-btn:hover:not(:disabled) {
		background: var(--amber-500);
		box-shadow: 0 0 16px rgba(217, 119, 6, 0.25);
	}

	.login-btn:disabled {
		opacity: 0.4;
		cursor: not-allowed;
	}

	.footer-link {
		margin: 20px 0 0;
		text-align: center;
		font-size: 0.75em;
		color: var(--text-muted);
	}

	.footer-link a {
		color: var(--amber-400);
		text-decoration: none;
	}

	.footer-link a:hover {
		text-decoration: underline;
	}
</style>
