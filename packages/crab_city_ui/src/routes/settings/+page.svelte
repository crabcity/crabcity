<script lang="ts">
	import { base } from '$app/paths';
	import { theme, toggleTheme, diffEngine, defaultCommand } from '$lib/stores/settings';
	import { currentIdentity, authEnabled } from '$lib/stores/auth';
	import { apiGet, api } from '$lib/utils/api';

	// =========================================================================
	// Types
	// =========================================================================

	interface OverrideState {
		host: boolean;
		port: boolean;
		auth_enabled: boolean;
		https: boolean;
	}

	interface ServerConfig {
		profile: string | null;
		host: string;
		port: number;
		auth_enabled: boolean;
		https: boolean;
		overrides: OverrideState;
	}

	// =========================================================================
	// Client state
	// =========================================================================

	const themes: Array<'phosphor' | 'analog'> = ['phosphor', 'analog'];
	const diffEngines: Array<'standard' | 'patience' | 'structural'> = ['standard', 'patience', 'structural'];

	// =========================================================================
	// Server section — Owner only
	// =========================================================================

	const isOwner = $derived(
		!$authEnabled || $currentIdentity?.capability === 'Owner'
	);

	let serverConfig = $state<ServerConfig | null>(null);
	let serverError = $state('');
	let serverLoading = $state(false);

	// Working copy
	let host = $state('127.0.0.1');
	let port = $state(3000);
	let srvAuth = $state(false);
	let srvHttps = $state(false);

	// Action state
	let applying = $state(false);
	let saving = $state(false);
	let statusMessage = $state('');

	const profiles = ['local', 'tunnel', 'server'] as const;
	type Profile = (typeof profiles)[number];

	function detectProfile(h: string, auth: boolean, https: boolean): Profile | null {
		if (h === '127.0.0.1' && !auth && !https) return 'local';
		if (h === '127.0.0.1' && auth && https) return 'tunnel';
		if (h === '0.0.0.0' && auth && https) return 'server';
		return null;
	}

	function applyProfileDefaults(p: Profile) {
		switch (p) {
			case 'local':
				host = '127.0.0.1';
				srvAuth = false;
				srvHttps = false;
				break;
			case 'tunnel':
				host = '127.0.0.1';
				srvAuth = true;
				srvHttps = true;
				break;
			case 'server':
				host = '0.0.0.0';
				srvAuth = true;
				srvHttps = true;
				break;
		}
	}

	const activeProfile = $derived(detectProfile(host, srvAuth, srvHttps));

	const isDirty = $derived(
		serverConfig !== null && (
			host !== serverConfig.host ||
			port !== serverConfig.port ||
			srvAuth !== serverConfig.auth_enabled ||
			srvHttps !== serverConfig.https
		)
	);

	function provenanceTag(field: keyof OverrideState): string {
		if (!serverConfig) return '';
		if (serverConfig.overrides[field]) return 'override';
		return '';
	}

	function syncFromConfig(cfg: ServerConfig) {
		host = cfg.host;
		port = cfg.port;
		srvAuth = cfg.auth_enabled;
		srvHttps = cfg.https;
	}

	async function fetchConfig() {
		serverLoading = true;
		serverError = '';
		try {
			const cfg = await apiGet<ServerConfig>('/api/admin/config');
			serverConfig = cfg;
			syncFromConfig(cfg);
		} catch (e) {
			serverError = e instanceof Error ? e.message : 'Failed to load config';
		} finally {
			serverLoading = false;
		}
	}

	async function applyConfig(save: boolean) {
		const busy = save ? 'saving' : 'applying';
		if (save) saving = true; else applying = true;
		statusMessage = '';

		try {
			const body: Record<string, unknown> = { save };
			if (host !== serverConfig?.host) body.host = host;
			if (port !== serverConfig?.port) body.port = port;
			if (srvAuth !== serverConfig?.auth_enabled) body.auth_enabled = srvAuth;
			if (srvHttps !== serverConfig?.https) body.https = srvHttps;

			const resp = await api('/api/admin/config', {
				method: 'PATCH',
				body: JSON.stringify(body),
			});

			if (!resp.ok) {
				throw new Error(`Server returned ${resp.status}`);
			}

			statusMessage = 'Restarting...';

			// Wait for server to restart, then refetch
			await new Promise((r) => setTimeout(r, 1500));
			try {
				await fetchConfig();
				statusMessage = save ? 'Saved to config' : 'Applied';
				setTimeout(() => { statusMessage = ''; }, 3000);
			} catch {
				statusMessage = 'Server may still be restarting';
				setTimeout(() => { statusMessage = ''; }, 5000);
			}
		} catch (e) {
			statusMessage = e instanceof Error ? e.message : 'Failed';
			setTimeout(() => { statusMessage = ''; }, 5000);
		} finally {
			applying = false;
			saving = false;
		}
	}

	$effect(() => {
		if (isOwner && !serverConfig && !serverLoading) {
			fetchConfig();
		}
	});
</script>

<div class="settings-page">
	<header class="settings-header">
		<a href="{base}/" class="back-link">
			<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
				<path d="M19 12H5M12 19l-7-7 7-7" />
			</svg>
			Back
		</a>
		<h1>Settings</h1>
		<div class="header-spacer"></div>
	</header>

	<div class="settings-content">
		<!-- ================================================================= -->
		<!-- APPEARANCE                                                        -->
		<!-- ================================================================= -->
		<section class="section">
			<h2 class="section-title">Appearance</h2>

			<div class="field-row">
				<span class="field-label">Theme</span>
				<div class="segmented">
					{#each themes as t}
						<button
							class="seg-btn"
							class:active={$theme === t}
							onclick={() => { if ($theme !== t) toggleTheme(); }}
						>{t}</button>
					{/each}
				</div>
			</div>

			<div class="field-row">
				<span class="field-label">Diff Engine</span>
				<div class="segmented">
					{#each diffEngines as d}
						<button
							class="seg-btn"
							class:active={$diffEngine === d}
							onclick={() => diffEngine.set(d)}
						>{d}</button>
					{/each}
				</div>
			</div>
		</section>

		<!-- ================================================================= -->
		<!-- DEFAULTS                                                          -->
		<!-- ================================================================= -->
		<section class="section">
			<h2 class="section-title">Defaults</h2>

			<div class="field-row">
				<label class="field-label" for="default-cmd">Instance Cmd</label>
				<input
					id="default-cmd"
					type="text"
					class="text-input"
					bind:value={$defaultCommand}
					placeholder="claude"
				/>
			</div>
		</section>

		<!-- ================================================================= -->
		<!-- IDENTITY                                                          -->
		<!-- ================================================================= -->
		<section class="section">
			<h2 class="section-title">Identity</h2>

			{#if $currentIdentity}
				<div class="field-row">
					<span class="field-label">Fingerprint</span>
					<code class="field-value fingerprint">{$currentIdentity.fingerprint}</code>
				</div>
				<div class="field-row">
					<span class="field-label">Display Name</span>
					<span class="field-value">{$currentIdentity.displayName}</span>
				</div>
				<div class="field-row">
					<span class="field-label">Capability</span>
					<span class="field-value cap-badge">{$currentIdentity.capability}</span>
				</div>
			{:else if !$authEnabled}
				<div class="field-row">
					<span class="field-label">Connection</span>
					<span class="field-value cap-badge">Loopback</span>
				</div>
				<div class="field-row">
					<span class="field-label">Access</span>
					<span class="field-value">Owner (automatic)</span>
				</div>
			{:else}
				<p class="hint-text">No identity loaded</p>
			{/if}

			<a href="{base}/account" class="manage-link">
				Manage Keys
				<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
					<path d="M9 18l6-6-6-6" />
				</svg>
			</a>
		</section>

		<!-- ================================================================= -->
		<!-- MEMBERS                                                           -->
		<!-- ================================================================= -->
		{#if isOwner}
			<section class="section">
				<h2 class="section-title">Members & Invites</h2>
				<p class="hint-text">Manage members, create invite links, and control access.</p>
				<a href="{base}/members" class="manage-link">
					Manage Members
					<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
						<path d="M9 18l6-6-6-6" />
					</svg>
				</a>
			</section>
		{/if}

		<!-- ================================================================= -->
		<!-- SERVER (Owner only)                                               -->
		<!-- ================================================================= -->
		{#if isOwner}
			<section class="section">
				<h2 class="section-title">Server</h2>

				{#if serverLoading}
					<div class="loading-row">
						<div class="spinner"></div>
						<span>Loading config...</span>
					</div>
				{:else if serverError}
					<div class="error-row">{serverError}</div>
				{:else if serverConfig}
					<!-- Profile -->
					<div class="field-row">
						<span class="field-label">Profile</span>
						<div class="segmented">
							{#each profiles as p}
								<button
									class="seg-btn"
									class:active={activeProfile === p}
									onclick={() => applyProfileDefaults(p)}
								>{p}</button>
							{/each}
						</div>
					</div>

					<!-- Host -->
					<div class="field-row">
						<label class="field-label" for="srv-host">Host</label>
						<input
							id="srv-host"
							type="text"
							class="text-input"
							bind:value={host}
						/>
						{#if !isDirty && provenanceTag('host')}
							<span class="provenance">{provenanceTag('host')}</span>
						{/if}
					</div>

					<!-- Port -->
					<div class="field-row">
						<label class="field-label" for="srv-port">Port</label>
						<input
							id="srv-port"
							type="number"
							class="text-input narrow"
							bind:value={port}
							min="1"
							max="65535"
						/>
						{#if !isDirty && provenanceTag('port')}
							<span class="provenance">{provenanceTag('port')}</span>
						{/if}
					</div>

					<!-- Auth -->
					<div class="field-row">
						<span class="field-label">Auth</span>
						<div class="segmented">
							<button class="seg-btn" class:active={!srvAuth} onclick={() => { srvAuth = false; }}>Off</button>
							<button class="seg-btn" class:active={srvAuth} onclick={() => { srvAuth = true; }}>On</button>
						</div>
						{#if !isDirty && provenanceTag('auth_enabled')}
							<span class="provenance">{provenanceTag('auth_enabled')}</span>
						{/if}
					</div>

					<!-- HTTPS -->
					<div class="field-row">
						<span class="field-label">HTTPS</span>
						<div class="segmented">
							<button class="seg-btn" class:active={!srvHttps} onclick={() => { srvHttps = false; }}>Off</button>
							<button class="seg-btn" class:active={srvHttps} onclick={() => { srvHttps = true; }}>On</button>
						</div>
						{#if !isDirty && provenanceTag('https')}
							<span class="provenance">{provenanceTag('https')}</span>
						{/if}
					</div>

					<!-- Status + Actions -->
					<div class="action-row">
						{#if isDirty}
							<span class="dirty-indicator">Unsaved changes</span>
						{/if}
						{#if statusMessage}
							<span class="status-msg">{statusMessage}</span>
						{/if}
						<div class="action-buttons">
							<button
								class="action-btn"
								disabled={!isDirty || applying || saving}
								onclick={() => applyConfig(false)}
							>
								{#if applying}Applying...{:else}Apply{/if}
							</button>
							<button
								class="action-btn primary"
								disabled={!isDirty || applying || saving}
								onclick={() => applyConfig(true)}
							>
								{#if saving}Saving...{:else}Save to Config{/if}
							</button>
						</div>
					</div>
				{/if}
			</section>
		{/if}
	</div>
</div>

<style>
	/* ===================================================================== */
	/* Page shell                                                            */
	/* ===================================================================== */

	.settings-page {
		display: flex;
		flex-direction: column;
		height: 100vh;
		height: 100dvh;
		background: var(--surface-800);
	}

	/* ===================================================================== */
	/* Header                                                                */
	/* ===================================================================== */

	.settings-header {
		display: flex;
		align-items: center;
		gap: 16px;
		padding: 16px 20px;
		background: linear-gradient(180deg, var(--surface-600) 0%, var(--surface-700) 100%);
		border-bottom: 1px solid var(--surface-border);
		flex-shrink: 0;
	}

	.back-link {
		display: flex;
		align-items: center;
		gap: 6px;
		padding: 8px 12px;
		background: transparent;
		border: 1px solid var(--surface-border);
		border-radius: 4px;
		color: var(--text-secondary);
		font-size: 12px;
		font-weight: 600;
		letter-spacing: 0.05em;
		text-decoration: none;
		text-transform: uppercase;
		transition: all 0.15s ease;
	}

	.back-link:hover {
		border-color: var(--amber-600);
		color: var(--amber-400);
		background: var(--tint-hover);
	}

	.back-link svg { width: 14px; height: 14px; }

	.settings-header h1 {
		flex: 1;
		margin: 0;
		font-size: 14px;
		font-weight: 700;
		letter-spacing: 0.1em;
		text-transform: uppercase;
		color: var(--amber-400);
		text-shadow: var(--emphasis-strong);
		font-family: var(--font-display);
	}

	.header-spacer { width: 72px; }

	/* ===================================================================== */
	/* Content area                                                          */
	/* ===================================================================== */

	.settings-content {
		flex: 1;
		overflow-y: auto;
		padding: 20px;
		display: flex;
		flex-direction: column;
		gap: 4px;
		width: 100%;
		max-width: 600px;
		margin: 0 auto;
	}

	.settings-content::-webkit-scrollbar { width: 8px; }
	.settings-content::-webkit-scrollbar-track { background: transparent; }
	.settings-content::-webkit-scrollbar-thumb { background: var(--surface-border); border-radius: 4px; }
	.settings-content::-webkit-scrollbar-thumb:hover { background: var(--amber-600); }

	/* ===================================================================== */
	/* Sections                                                              */
	/* ===================================================================== */

	.section {
		padding: 16px 0;
		border-bottom: 1px solid var(--surface-border);
	}

	.section:last-child { border-bottom: none; }

	.section-title {
		margin: 0 0 12px;
		font-size: 11px;
		font-weight: 700;
		letter-spacing: 0.1em;
		text-transform: uppercase;
		color: var(--text-muted);
	}

	/* ===================================================================== */
	/* Field rows                                                            */
	/* ===================================================================== */

	.field-row {
		display: flex;
		align-items: center;
		gap: 12px;
		padding: 6px 0;
	}

	.field-label {
		width: 100px;
		flex-shrink: 0;
		font-size: 12px;
		font-weight: 600;
		letter-spacing: 0.03em;
		color: var(--text-secondary);
		text-transform: uppercase;
	}

	.field-value {
		font-size: 12px;
		color: var(--text-primary);
	}

	.fingerprint {
		color: var(--amber-400);
		letter-spacing: 0.03em;
		font-size: 11px;
	}

	.cap-badge {
		padding: 1px 6px;
		font-size: 11px;
		font-weight: 700;
		background: var(--tint-active);
		border: 1px solid var(--amber-700);
		border-radius: 2px;
		letter-spacing: 0.05em;
		color: var(--amber-400);
	}

	.hint-text {
		margin: 0;
		padding: 4px 0;
		font-size: 12px;
		color: var(--text-muted);
	}

	/* ===================================================================== */
	/* Segmented controls                                                    */
	/* ===================================================================== */

	.segmented {
		display: flex;
		gap: 1px;
		background: var(--surface-900);
		border: 1px solid var(--surface-border);
		border-radius: 3px;
		overflow: hidden;
	}

	.seg-btn {
		padding: 5px 12px;
		background: transparent;
		border: none;
		color: var(--text-muted);
		font-size: 11px;
		font-weight: 600;
		font-family: inherit;
		letter-spacing: 0.05em;
		text-transform: uppercase;
		cursor: pointer;
		transition: all 0.15s ease;
	}

	.seg-btn:hover:not(.active) {
		color: var(--text-secondary);
		background: var(--tint-hover);
	}

	.seg-btn.active {
		background: var(--tint-active-strong);
		color: var(--amber-400);
		text-shadow: var(--emphasis);
	}

	/* ===================================================================== */
	/* Text inputs                                                           */
	/* ===================================================================== */

	.text-input {
		flex: 1;
		padding: 5px 10px;
		background: var(--surface-900);
		border: 1px solid var(--surface-border);
		border-radius: 3px;
		color: var(--text-primary);
		font-size: 12px;
		font-family: inherit;
		outline: none;
		transition: border-color 0.15s ease;
	}

	.text-input:focus { border-color: var(--amber-600); }

	.text-input.narrow { max-width: 100px; }

	/* Hide number input spinners */
	.text-input[type="number"]::-webkit-outer-spin-button,
	.text-input[type="number"]::-webkit-inner-spin-button {
		-webkit-appearance: none;
		margin: 0;
	}
	.text-input[type="number"] { -moz-appearance: textfield; }

	/* ===================================================================== */
	/* Manage keys link                                                      */
	/* ===================================================================== */

	.manage-link {
		display: inline-flex;
		align-items: center;
		gap: 4px;
		margin-top: 8px;
		font-size: 12px;
		color: var(--text-muted);
		text-decoration: none;
		transition: color 0.15s ease;
	}

	.manage-link:hover { color: var(--amber-400); }

	.manage-link svg { width: 14px; height: 14px; }

	/* ===================================================================== */
	/* Provenance tags                                                       */
	/* ===================================================================== */

	.provenance {
		font-size: 10px;
		font-weight: 600;
		letter-spacing: 0.05em;
		text-transform: uppercase;
		color: var(--text-muted);
		opacity: 0.6;
		flex-shrink: 0;
	}

	/* ===================================================================== */
	/* Server section: loading, errors, actions                              */
	/* ===================================================================== */

	.loading-row {
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 12px 0;
		font-size: 12px;
		color: var(--text-muted);
	}

	.spinner {
		width: 14px;
		height: 14px;
		border: 2px solid var(--surface-border);
		border-top-color: var(--amber-500);
		border-radius: 50%;
		animation: spin 0.8s linear infinite;
	}

	@keyframes spin { to { transform: rotate(360deg); } }

	.error-row {
		padding: 8px 12px;
		background: rgba(239, 68, 68, 0.12);
		border: 1px solid rgba(239, 68, 68, 0.25);
		border-radius: 3px;
		font-size: 12px;
		color: var(--status-red);
	}

	.action-row {
		display: flex;
		align-items: center;
		gap: 12px;
		padding-top: 12px;
		flex-wrap: wrap;
	}

	.dirty-indicator {
		font-size: 11px;
		font-weight: 600;
		color: var(--amber-500);
		letter-spacing: 0.03em;
	}

	.status-msg {
		font-size: 11px;
		color: var(--text-muted);
	}

	.action-buttons {
		display: flex;
		gap: 8px;
		margin-left: auto;
	}

	.action-btn {
		padding: 7px 14px;
		background: linear-gradient(180deg, var(--surface-500) 0%, var(--surface-600) 100%);
		border: 1px solid var(--surface-border);
		border-radius: 3px;
		color: var(--text-secondary);
		font-size: 11px;
		font-weight: 700;
		font-family: inherit;
		letter-spacing: 0.05em;
		text-transform: uppercase;
		cursor: pointer;
		transition: all 0.15s ease;
		box-shadow: var(--depth-up);
	}

	.action-btn:hover:not(:disabled) {
		border-color: var(--amber-600);
		color: var(--amber-400);
		box-shadow: var(--elevation-high);
	}

	.action-btn:disabled {
		opacity: 0.4;
		cursor: not-allowed;
	}

	.action-btn.primary {
		border-color: var(--amber-600);
		color: var(--amber-400);
	}

	.action-btn.primary:hover:not(:disabled) {
		background: linear-gradient(180deg, var(--surface-400) 0%, var(--surface-500) 100%);
		border-color: var(--amber-500);
		color: var(--amber-300);
		text-shadow: var(--emphasis);
	}

	/* ===================================================================== */
	/* Responsive                                                            */
	/* ===================================================================== */

	@media (max-width: 639px) {
		.settings-header { padding: 12px 14px; gap: 10px; }
		.settings-header h1 { font-size: 12px; }
		.settings-content { padding: 12px 14px; }
		.header-spacer { display: none; }
		.field-label { width: 80px; }
		.action-buttons { margin-left: 0; width: 100%; }
		.action-btn { flex: 1; }
	}

	/* ===================================================================== */
	/* Analog theme                                                          */
	/* ===================================================================== */

	:global([data-theme="analog"]) .settings-header {
		background-color: var(--surface-800);
		background-image: var(--grain-fine), var(--grain-coarse);
		background-blend-mode: multiply, multiply;
		border-bottom-width: 2px;
	}

	:global([data-theme="analog"]) .settings-header h1 {
		font-family: 'Newsreader', Georgia, serif;
		text-transform: none;
		font-size: 18px;
		font-weight: 600;
		letter-spacing: 0;
	}

	:global([data-theme="analog"]) .section-title {
		font-family: 'Source Serif 4', Georgia, serif;
		text-transform: none;
		letter-spacing: 0;
		font-size: 13px;
	}

	:global([data-theme="analog"]) .back-link {
		font-family: 'Source Serif 4', Georgia, serif;
		text-transform: none;
		letter-spacing: 0;
	}

	:global([data-theme="analog"]) .seg-btn {
		font-family: 'Source Serif 4', Georgia, serif;
		text-transform: none;
		letter-spacing: 0;
	}

	:global([data-theme="analog"]) .action-btn {
		font-family: 'Source Serif 4', Georgia, serif;
		text-transform: none;
		letter-spacing: 0;
		border-width: 1.5px;
	}
</style>
