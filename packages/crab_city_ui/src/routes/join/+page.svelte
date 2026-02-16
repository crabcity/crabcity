<script lang="ts">
	import { goto } from '$app/navigation';
	import { base } from '$app/paths';
	import { onMount, onDestroy } from 'svelte';
	import {
		localKeypair,
		currentIdentity,
		importIdentity,
		authError,
	} from '$lib/stores/auth';
	import { generateKeypair, saveKeypair, type KeyIdentity } from '$lib/crypto/keys';
	import {
		sendRedeemInvite,
		initMultiplexedConnection,
		setAuthRequiredCallback,
		setPendingPasswordAuth,
	} from '$lib/stores/websocket';
	import KeyBackupModal from '$lib/components/KeyBackupModal.svelte';

	type JoinMode = 'password' | 'generate' | 'import';

	let token = $state('');
	let displayName = $state('');
	let username = $state('');
	let password = $state('');
	let importKeyValue = $state('');
	let mode = $state<JoinMode>('password');
	let showBackupModal = $state(false);
	let pendingIdentity = $state<KeyIdentity | null>(null);
	let error = $state('');
	let joining = $state(false);
	let step = $state<'form' | 'connecting'>('form');
	let timeoutHandle: ReturnType<typeof setTimeout> | null = null;
	let identityUnsubscribe: (() => void) | null = null;

	// Surface auth errors from the store
	$effect(() => {
		const err = $authError;
		if (err && joining) {
			error = err;
			joining = false;
			step = 'form';
		}
	});

	// Activity indicator from preview endpoint
	let previewData = $state<{ terminal_count: number; user_count: number; instance_name: string } | null>(null);
	let previewWs: WebSocket | null = null;

	onMount(() => {
		// Extract invite token from hash fragment
		const hash = window.location.hash.slice(1);
		if (hash) {
			token = hash;
		}

		// Connect to preview WS for activity indicator
		const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
		const wsUrl = `${protocol}//${window.location.host}/api/preview`;
		try {
			previewWs = new WebSocket(wsUrl);
			previewWs.onmessage = (event) => {
				try {
					previewData = JSON.parse(event.data);
				} catch { /* ignore */ }
			};
		} catch { /* preview is optional */ }

		return () => {
			previewWs?.close();
		};
	});

	onDestroy(() => {
		cleanup();
	});

	function cleanup() {
		setAuthRequiredCallback(null);
		identityUnsubscribe?.();
		identityUnsubscribe = null;
		if (timeoutHandle) {
			clearTimeout(timeoutHandle);
			timeoutHandle = null;
		}
	}

	function validateCommonFields(): boolean {
		if (!token.trim()) {
			error = 'Invite token is required';
			return false;
		}
		if (!displayName.trim()) {
			error = 'Display name is required';
			return false;
		}
		return true;
	}

	async function handleSubmit(e: Event) {
		e.preventDefault();
		error = '';

		if (!validateCommonFields()) return;

		if (mode === 'password') {
			handlePasswordJoin();
		} else if (mode === 'import') {
			await handleImportJoin();
		} else {
			handleGenerateJoin();
		}
	}

	function handlePasswordJoin() {
		if (!username.trim()) {
			error = 'Username is required';
			return;
		}
		if (!password.trim()) {
			error = 'Password is required';
			return;
		}

		step = 'connecting';
		joining = true;

		// Set up password auth — when WS connects and Challenge arrives,
		// websocket.ts will send PasswordAuth instead of ChallengeResponse.
		setPendingPasswordAuth({
			username: username.trim(),
			password,
			inviteToken: token.trim(),
			displayName: displayName.trim(),
		});

		// Watch for successful authentication
		identityUnsubscribe = currentIdentity.subscribe(($id) => {
			if ($id && joining) {
				console.log('[Join] Authenticated as', $id.fingerprint);
				joining = false;
				cleanup();
				goto(`${base}/`, { replaceState: true });
			}
		});

		initMultiplexedConnection();
		startTimeout();
	}

	function handleGenerateJoin() {
		const existingKp = $localKeypair;
		if (existingKp) {
			proceedToRedeem(existingKp);
			return;
		}

		const kp = generateKeypair();
		pendingIdentity = kp;
		showBackupModal = true;
	}

	async function handleImportJoin() {
		if (!importKeyValue.trim()) {
			error = 'Paste your key to import';
			return;
		}

		try {
			const kp = await importIdentity(importKeyValue.trim());
			proceedToRedeem(kp);
		} catch (err) {
			error = `Invalid key: ${err instanceof Error ? err.message : 'unknown error'}`;
		}
	}

	async function onBackupConfirmed() {
		if (!pendingIdentity) return;
		showBackupModal = false;

		await saveKeypair(pendingIdentity);
		localKeypair.set(pendingIdentity);

		proceedToRedeem(pendingIdentity);
	}

	function proceedToRedeem(_kp: KeyIdentity) {
		step = 'connecting';
		joining = true;
		error = '';

		setAuthRequiredCallback((err?: string) => {
			if (err) {
				error = err;
				joining = false;
				step = 'form';
				return;
			}
			console.log('[Join] AuthRequired received — sending RedeemInvite');
			sendRedeemInvite(token, displayName);
		});

		identityUnsubscribe = currentIdentity.subscribe(($id) => {
			if ($id && joining) {
				console.log('[Join] Authenticated as', $id.fingerprint);
				joining = false;
				cleanup();
				goto(`${base}/`, { replaceState: true });
			}
		});

		initMultiplexedConnection();
		startTimeout();
	}

	function startTimeout() {
		timeoutHandle = setTimeout(() => {
			if (joining) {
				joining = false;
				step = 'form';
				error = 'Connection timed out. Check your invite token and try again.';
				cleanup();
			}
		}, 15000);
	}
</script>

<div class="join-page">
	<div class="join-card">
		<div class="header">
			<h1>CRAB CITY</h1>
			<div class="divider"></div>
			<p class="subtitle">JOIN INSTANCE</p>
		</div>

		{#if previewData}
			<div class="activity-bar">
				<span class="activity-dot"></span>
				<span class="activity-text">
					{previewData.instance_name}
					&mdash; {previewData.user_count} user{previewData.user_count !== 1 ? 's' : ''} online,
					{previewData.terminal_count} terminal{previewData.terminal_count !== 1 ? 's' : ''}
				</span>
			</div>
		{/if}

		{#if error}
			<div class="error">{error}</div>
		{/if}

		{#if step === 'form'}
			{#if showBackupModal && pendingIdentity}
				<KeyBackupModal identity={pendingIdentity} onconfirm={onBackupConfirmed} />
			{/if}

			<form onsubmit={handleSubmit}>
				<label>
					<span class="field-label">INVITE TOKEN</span>
					<input
						type="text"
						bind:value={token}
						placeholder="Paste invite token..."
						required
						disabled={joining}
					/>
				</label>

				<label>
					<span class="field-label">DISPLAY NAME</span>
					<input
						type="text"
						bind:value={displayName}
						placeholder="How others will see you"
						required
						maxlength="64"
						disabled={joining}
					/>
				</label>

				{#if mode === 'password'}
					<label>
						<span class="field-label">USERNAME</span>
						<input
							type="text"
							bind:value={username}
							placeholder="Choose a username"
							required
							maxlength="64"
							autocomplete="username"
							disabled={joining}
						/>
					</label>

					<label>
						<span class="field-label">PASSWORD</span>
						<input
							type="password"
							bind:value={password}
							placeholder="Choose a password"
							required
							autocomplete="new-password"
							disabled={joining}
						/>
					</label>
				{:else if mode === 'import'}
					<label>
						<span class="field-label">IMPORT KEY</span>
						<textarea
							bind:value={importKeyValue}
							placeholder="Paste base64 private key..."
							rows="3"
							disabled={joining}
						></textarea>
					</label>
				{/if}

				<button type="submit" class="join-btn" disabled={joining}>
					{#if mode === 'password'}
						JOIN
					{:else if mode === 'import'}
						IMPORT KEY & JOIN
					{:else}
						GENERATE KEY & JOIN
					{/if}
				</button>
			</form>

			<div class="mode-toggles">
				{#if mode === 'password'}
					<button class="toggle-link" onclick={() => { mode = 'generate'; }}>
						Use a keypair instead
					</button>
				{:else}
					<button class="toggle-link" onclick={() => { mode = 'password'; }}>
						Join with password instead
					</button>
					{#if mode === 'generate'}
						<button class="toggle-link" onclick={() => { mode = 'import'; }}>
							I already have a key
						</button>
					{:else}
						<button class="toggle-link" onclick={() => { mode = 'generate'; }}>
							Generate a new key instead
						</button>
					{/if}
				{/if}
			</div>

		{:else if step === 'connecting'}
			<div class="connecting">
				<div class="spinner"></div>
				<p>Connecting and redeeming invite...</p>
			</div>
		{/if}

		<p class="footer-link">
			Already a member? <a href="{base}/login">Log in</a>
		</p>
	</div>
</div>

<style>
	.join-page {
		display: flex;
		align-items: center;
		justify-content: center;
		min-height: 100vh;
		background: var(--surface-900);
		padding: 16px;
	}

	.join-card {
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

	.activity-bar {
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 8px 12px;
		margin-bottom: 16px;
		background: var(--surface-900);
		border: 1px solid var(--surface-border);
		border-radius: 3px;
	}

	.activity-dot {
		width: 6px;
		height: 6px;
		border-radius: 50%;
		background: var(--status-green);
		box-shadow: 0 0 6px var(--status-green);
		flex-shrink: 0;
		animation: pulse 2s ease-in-out infinite;
	}

	@keyframes pulse {
		0%, 100% { opacity: 1; }
		50% { opacity: 0.4; }
	}

	.activity-text {
		font-size: 0.72em;
		color: var(--text-muted);
		letter-spacing: 0.02em;
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

	input, textarea {
		padding: 8px 12px;
		background: var(--surface-900);
		border: 1px solid var(--surface-border);
		border-radius: 3px;
		color: var(--text-primary);
		font-family: inherit;
		font-size: 0.85em;
		transition: border-color 0.15s ease;
		resize: none;
	}

	input:focus, textarea:focus {
		outline: none;
		border-color: var(--amber-600);
		box-shadow: var(--elevation-low);
	}

	input::placeholder, textarea::placeholder {
		color: var(--text-muted);
	}

	.join-btn {
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

	.join-btn:hover:not(:disabled) {
		background: var(--amber-500);
		box-shadow: 0 0 16px rgba(217, 119, 6, 0.25);
	}

	.join-btn:disabled {
		opacity: 0.4;
		cursor: not-allowed;
	}

	.mode-toggles {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 2px;
		margin-top: 8px;
	}

	.toggle-link {
		display: block;
		padding: 6px;
		background: none;
		border: none;
		color: var(--text-muted);
		font-family: inherit;
		font-size: 0.72em;
		cursor: pointer;
		transition: color 0.15s ease;
	}

	.toggle-link:hover {
		color: var(--amber-400);
	}

	.connecting {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 16px;
		padding: 32px 0;
	}

	.connecting p {
		margin: 0;
		font-size: 0.8em;
		color: var(--text-secondary);
		letter-spacing: 0.02em;
	}

	.spinner {
		width: 24px;
		height: 24px;
		border: 2px solid var(--surface-border);
		border-top-color: var(--amber-600);
		border-radius: 50%;
		animation: spin 0.8s linear infinite;
	}

	@keyframes spin {
		to { transform: rotate(360deg); }
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
