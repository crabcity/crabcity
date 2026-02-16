<script lang="ts">
	import { base } from '$app/paths';
	import { goto } from '$app/navigation';
	import {
		currentIdentity,
		localKeypair,
		isAuthenticated,
		exportIdentity,
		downloadIdentity,
		importIdentity,
		clearIdentity,
	} from '$lib/stores/auth';

	let showImport = $state(false);
	let importKeyValue = $state('');
	let importError = $state('');
	let importSuccess = $state('');
	let copied = $state(false);

	const exportedKey = $derived(
		$localKeypair ? exportIdentity($localKeypair) : null
	);

	function copyKey() {
		if (!exportedKey) return;
		navigator.clipboard.writeText(exportedKey);
		copied = true;
		setTimeout(() => { copied = false; }, 2000);
	}

	function handleDownload() {
		if (!$localKeypair) return;
		downloadIdentity($localKeypair);
	}

	async function handleImport(e: Event) {
		e.preventDefault();
		importError = '';
		importSuccess = '';

		if (!importKeyValue.trim()) {
			importError = 'Paste a key to import';
			return;
		}

		try {
			await importIdentity(importKeyValue.trim());
			importSuccess = 'Key imported successfully. Reconnect to use the new identity.';
			importKeyValue = '';
			showImport = false;
		} catch (err) {
			importError = `Invalid key: ${err instanceof Error ? err.message : 'unknown error'}`;
		}
	}

	async function handleClearIdentity() {
		await clearIdentity();
		goto(`${base}/join`);
	}
</script>

<div class="auth-page">
	<div class="auth-card">
		<a class="back-link" href="{base}/">&larr; Dashboard</a>
		<h1>IDENTITY</h1>

		{#if $currentIdentity}
			<div class="info-row">
				<span class="label">FINGERPRINT</span>
				<code class="value fingerprint">{$currentIdentity.fingerprint}</code>
			</div>
			<div class="info-row">
				<span class="label">DISPLAY NAME</span>
				<span class="value">{$currentIdentity.displayName}</span>
			</div>
			<div class="info-row">
				<span class="label">CAPABILITY</span>
				<span class="value cap-badge">{$currentIdentity.capability}</span>
			</div>
		{:else if $localKeypair}
			<div class="info-row">
				<span class="label">FINGERPRINT</span>
				<code class="value fingerprint">{$localKeypair.fingerprint}</code>
			</div>
			<div class="info-row">
				<span class="label">STATUS</span>
				<span class="value dim">Not connected</span>
			</div>
		{:else}
			<p class="empty-state">No keypair loaded. Import or generate one via the join flow.</p>
		{/if}

		{#if $localKeypair}
			<hr />

			<h2>KEY MANAGEMENT</h2>

			<div class="key-actions">
				<button class="action-btn" onclick={copyKey}>
					{copied ? 'COPIED' : 'COPY KEY'}
				</button>
				<button class="action-btn" onclick={handleDownload}>
					DOWNLOAD .KEY FILE
				</button>
			</div>

			{#if exportedKey}
				<div class="key-preview">
					<span class="label">PRIVATE KEY (BASE64)</span>
					<pre class="key-value">{exportedKey}</pre>
				</div>
			{/if}

			<hr />

			<button class="toggle-link" onclick={() => { showImport = !showImport; }}>
				{showImport ? 'Cancel import' : 'Import a different key'}
			</button>

			{#if showImport}
				{#if importError}
					<div class="error">{importError}</div>
				{/if}
				{#if importSuccess}
					<div class="success">{importSuccess}</div>
				{/if}

				<form onsubmit={handleImport}>
					<label>
						<span>BASE64 PRIVATE KEY</span>
						<textarea
							bind:value={importKeyValue}
							placeholder="Paste base64 key..."
							rows="3"
						></textarea>
					</label>
					<button type="submit" class="submit-btn">IMPORT KEY</button>
				</form>
			{/if}

			<hr />

			<button class="danger-btn" onclick={handleClearIdentity}>
				CLEAR IDENTITY & SIGN OUT
			</button>
		{/if}

		<p class="footer-link">
			<a href="{base}/">Back to dashboard</a>
		</p>
	</div>
</div>

<style>
	.auth-page {
		position: fixed;
		inset: 0;
		overflow-y: auto;
		display: flex;
		align-items: flex-start;
		justify-content: center;
		background: var(--surface-900);
		padding: 40px 20px;
	}

	.auth-card {
		width: 100%;
		max-width: 420px;
		padding: 32px;
		background: linear-gradient(180deg, var(--surface-700) 0%, var(--surface-800) 100%);
		border: 1px solid var(--surface-border);
		border-radius: 4px;
	}

	.back-link {
		display: inline-block;
		margin-bottom: 12px;
		font-size: 0.8em;
		color: var(--text-muted);
		text-decoration: none;
	}

	.back-link:hover {
		color: var(--amber-400);
	}

	h1 {
		margin: 0 0 20px;
		font-size: 0.9em;
		font-weight: 700;
		letter-spacing: 0.15em;
		color: var(--amber-400);
		text-align: center;
		text-shadow: var(--emphasis);
	}

	h2 {
		margin: 0 0 12px;
		font-size: 0.7em;
		font-weight: 700;
		letter-spacing: 0.12em;
		color: var(--text-muted);
	}

	.info-row {
		display: flex;
		justify-content: space-between;
		align-items: center;
		padding: 6px 0;
		font-size: 0.85em;
	}

	.label {
		font-size: 0.65em;
		font-weight: 700;
		letter-spacing: 0.1em;
		color: var(--text-muted);
	}

	.value {
		color: var(--text-primary);
	}

	.fingerprint {
		color: var(--amber-400);
		letter-spacing: 0.03em;
	}

	.cap-badge {
		padding: 1px 6px;
		font-size: 0.8em;
		font-weight: 700;
		background: rgba(251, 146, 60, 0.12);
		border: 1px solid rgba(251, 146, 60, 0.2);
		border-radius: 2px;
		letter-spacing: 0.05em;
	}

	.dim {
		color: var(--text-muted);
	}

	.empty-state {
		text-align: center;
		font-size: 0.8em;
		color: var(--text-muted);
		padding: 16px 0;
	}

	hr {
		border: none;
		border-top: 1px solid var(--surface-border);
		margin: 16px 0;
	}

	.key-actions {
		display: flex;
		gap: 8px;
		margin-bottom: 12px;
	}

	.action-btn {
		flex: 1;
		padding: 8px 4px;
		background: linear-gradient(180deg, var(--surface-500) 0%, var(--surface-600) 100%);
		border: 1px solid var(--surface-border);
		border-radius: 3px;
		color: var(--text-secondary);
		font-family: inherit;
		font-size: 0.68em;
		font-weight: 700;
		letter-spacing: 0.08em;
		cursor: pointer;
		transition: all 0.15s ease;
	}

	.action-btn:hover {
		background: linear-gradient(180deg, var(--surface-400) 0%, var(--surface-500) 100%);
		border-color: var(--amber-600);
		color: var(--amber-400);
	}

	.key-preview {
		margin-bottom: 4px;
	}

	.key-value {
		margin: 4px 0 0;
		padding: 8px 10px;
		background: var(--surface-900);
		border: 1px solid var(--surface-border);
		border-radius: 3px;
		color: var(--text-primary);
		font-size: 0.72em;
		line-height: 1.5;
		word-break: break-all;
		white-space: pre-wrap;
		max-height: 72px;
		overflow-y: auto;
	}

	.toggle-link {
		display: block;
		width: 100%;
		padding: 6px;
		background: none;
		border: none;
		color: var(--text-muted);
		font-family: inherit;
		font-size: 0.72em;
		cursor: pointer;
		transition: color 0.15s ease;
		margin-bottom: 8px;
	}

	.toggle-link:hover {
		color: var(--amber-400);
	}

	.error {
		padding: 8px 12px;
		margin-bottom: 12px;
		background: rgba(239, 68, 68, 0.12);
		border: 1px solid rgba(239, 68, 68, 0.25);
		border-radius: 3px;
		color: var(--status-red);
		font-size: 0.8em;
	}

	.success {
		padding: 8px 12px;
		margin-bottom: 12px;
		background: rgba(16, 185, 129, 0.12);
		border: 1px solid rgba(16, 185, 129, 0.25);
		border-radius: 3px;
		color: var(--status-green);
		font-size: 0.8em;
	}

	form {
		display: flex;
		flex-direction: column;
		gap: 12px;
	}

	label {
		display: flex;
		flex-direction: column;
		gap: 4px;
	}

	label span {
		font-size: 0.65em;
		font-weight: 700;
		letter-spacing: 0.1em;
		color: var(--text-muted);
	}

	textarea {
		padding: 8px 12px;
		background: var(--surface-900);
		border: 1px solid var(--surface-border);
		border-radius: 3px;
		color: var(--text-primary);
		font-family: inherit;
		font-size: 0.85em;
		resize: none;
	}

	textarea:focus {
		outline: none;
		border-color: var(--amber-600);
	}

	.submit-btn {
		padding: 8px;
		background: var(--amber-600);
		border: none;
		border-radius: 3px;
		color: var(--surface-900);
		font-family: inherit;
		font-weight: 700;
		font-size: 0.72em;
		letter-spacing: 0.1em;
		cursor: pointer;
	}

	.submit-btn:hover {
		background: var(--amber-500);
	}

	.danger-btn {
		width: 100%;
		padding: 10px;
		background: transparent;
		border: 1px solid var(--surface-border);
		border-radius: 3px;
		color: var(--text-muted);
		font-family: inherit;
		font-size: 0.72em;
		font-weight: 700;
		letter-spacing: 0.08em;
		cursor: pointer;
		transition: all 0.15s ease;
	}

	.danger-btn:hover {
		border-color: var(--status-red);
		color: var(--status-red);
	}

	.footer-link {
		margin-top: 16px;
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
