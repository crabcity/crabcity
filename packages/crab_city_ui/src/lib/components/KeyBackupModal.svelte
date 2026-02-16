<script lang="ts">
	import { exportKey, type KeyIdentity } from '$lib/crypto/keys';

	interface Props {
		identity: KeyIdentity;
		onconfirm: () => void;
	}

	let { identity, onconfirm }: Props = $props();

	let confirmed = $state(false);
	let copied = $state(false);
	let downloaded = $state(false);

	const b64Key = $derived(exportKey(identity));
	const canProceed = $derived(confirmed && (copied || downloaded));

	function copyKey() {
		navigator.clipboard.writeText(b64Key);
		copied = true;
		setTimeout(() => { copied = false; }, 2000);
	}

	function downloadKey() {
		const blob = new Blob([b64Key], { type: 'text/plain' });
		const url = URL.createObjectURL(blob);
		const a = document.createElement('a');
		a.href = url;
		a.download = `${identity.fingerprint}.key`;
		a.click();
		URL.revokeObjectURL(url);
		downloaded = true;
	}

	function handleKeydown(e: KeyboardEvent) {
		// Block Escape — modal cannot be dismissed
		if (e.key === 'Escape') {
			e.preventDefault();
			e.stopPropagation();
		}
	}
</script>

<svelte:window onkeydown={handleKeydown} />

<!-- svelte-ignore a11y_click_events_have_key_events -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="backdrop">
	<div class="modal" role="dialog" aria-modal="true" aria-labelledby="backup-title">
		<div class="glyph">&#x1F510;</div>

		<h2 id="backup-title">SAVE YOUR KEY</h2>
		<p class="warning">
			This is your only identity credential. If you lose it, you lose access.
			There is no recovery. Back it up now.
		</p>

		<div class="fingerprint-row">
			<span class="label">FINGERPRINT</span>
			<code class="fingerprint">{identity.fingerprint}</code>
		</div>

		<div class="key-block">
			<span class="label">PRIVATE KEY</span>
			<pre class="key-value">{b64Key}</pre>
		</div>

		<div class="actions">
			<button class="action-btn" onclick={copyKey}>
				{copied ? 'COPIED' : 'COPY TO CLIPBOARD'}
			</button>
			<button class="action-btn" onclick={downloadKey}>
				{downloaded ? 'DOWNLOADED' : 'DOWNLOAD .KEY FILE'}
			</button>
		</div>

		<label class="confirm-row">
			<input type="checkbox" bind:checked={confirmed} />
			<span>I have saved my key and understand it cannot be recovered</span>
		</label>

		<button class="proceed-btn" disabled={!canProceed} onclick={onconfirm}>
			CONTINUE
		</button>
	</div>
</div>

<style>
	.backdrop {
		position: fixed;
		inset: 0;
		z-index: 100;
		display: flex;
		align-items: center;
		justify-content: center;
		background: rgba(0, 0, 0, 0.85);
		backdrop-filter: blur(4px);
		animation: fade-in 0.2s ease;
	}

	@keyframes fade-in {
		from { opacity: 0; }
		to { opacity: 1; }
	}

	.modal {
		width: 100%;
		max-width: 440px;
		margin: 16px;
		padding: 28px 24px;
		background: linear-gradient(180deg, var(--surface-700) 0%, var(--surface-800) 100%);
		border: 1px solid var(--amber-600);
		border-radius: 4px;
		box-shadow: 0 0 40px rgba(217, 119, 6, 0.15), 0 0 80px rgba(217, 119, 6, 0.05);
		animation: modal-in 0.25s cubic-bezier(0.4, 0, 0.2, 1);
	}

	@keyframes modal-in {
		from { opacity: 0; transform: scale(0.95) translateY(8px); }
		to { opacity: 1; transform: scale(1) translateY(0); }
	}

	.glyph {
		text-align: center;
		font-size: 2em;
		margin-bottom: 8px;
		filter: grayscale(1) brightness(1.5);
	}

	h2 {
		margin: 0 0 8px;
		text-align: center;
		font-size: 0.9em;
		font-weight: 700;
		letter-spacing: 0.15em;
		color: var(--amber-400);
		text-shadow: var(--emphasis);
	}

	.warning {
		margin: 0 0 20px;
		text-align: center;
		font-size: 0.78em;
		line-height: 1.5;
		color: var(--text-secondary);
	}

	.label {
		display: block;
		font-size: 0.65em;
		font-weight: 700;
		letter-spacing: 0.12em;
		color: var(--text-muted);
		margin-bottom: 4px;
	}

	.fingerprint-row {
		margin-bottom: 16px;
	}

	.fingerprint {
		display: block;
		padding: 6px 10px;
		background: var(--surface-900);
		border: 1px solid var(--surface-border);
		border-radius: 3px;
		color: var(--amber-400);
		font-size: 0.9em;
		letter-spacing: 0.05em;
	}

	.key-block {
		margin-bottom: 16px;
	}

	.key-value {
		margin: 0;
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

	.actions {
		display: flex;
		gap: 8px;
		margin-bottom: 16px;
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
		box-shadow: var(--elevation-low);
	}

	.confirm-row {
		display: flex;
		align-items: flex-start;
		gap: 8px;
		margin-bottom: 16px;
		cursor: pointer;
	}

	.confirm-row input[type="checkbox"] {
		margin-top: 2px;
		accent-color: var(--amber-600);
		flex-shrink: 0;
	}

	.confirm-row span {
		font-size: 0.78em;
		color: var(--text-secondary);
		line-height: 1.4;
	}

	.proceed-btn {
		width: 100%;
		padding: 10px;
		background: var(--amber-600);
		border: none;
		border-radius: 3px;
		color: var(--surface-900);
		font-family: inherit;
		font-weight: 700;
		font-size: 0.8em;
		letter-spacing: 0.1em;
		cursor: pointer;
		transition: all 0.15s ease;
	}

	.proceed-btn:hover:not(:disabled) {
		background: var(--amber-500);
		box-shadow: 0 0 20px rgba(217, 119, 6, 0.3);
	}

	.proceed-btn:disabled {
		opacity: 0.35;
		cursor: not-allowed;
	}
</style>
