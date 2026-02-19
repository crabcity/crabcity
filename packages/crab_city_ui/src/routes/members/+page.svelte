<script lang="ts">
	import { get } from 'svelte/store';
	import { base } from '$app/paths';
	import { members, type Member } from '$lib/stores/members';
	import { invites, lastCreatedInvite, type InviteInfo } from '$lib/stores/invites';
	import { currentIdentity, authEnabled } from '$lib/stores/auth';
	import { muxConnected, initMultiplexedConnection, sendMuxMessage } from '$lib/stores/websocket';

	// Invite creation form
	let inviteCapability = $state('Collaborate');
	let inviteMaxUses = $state(5);
	let inviteExpiry = $state(3600);
	let inviteLabel = $state('');
	let creatingInvite = $state(false);

	const memberList = $derived(
		Array.from(($members).values()).sort((a, b) => {
			const capOrder = ['Owner', 'Admin', 'Collaborate', 'View'];
			return capOrder.indexOf(a.capability) - capOrder.indexOf(b.capability);
		})
	);

	const activeInvites = $derived(
		($invites)
			.filter(i => i.state === 'active')
			.sort((a, b) => (b.created_at ?? '').localeCompare(a.created_at ?? ''))
	);

	const inactiveInvites = $derived(
		($invites)
			.filter(i => i.state !== 'active')
			.sort((a, b) => (b.created_at ?? '').localeCompare(a.created_at ?? ''))
	);

	let showInactive = $state(false);

	const myFingerprint = $derived($currentIdentity?.fingerprint ?? '');
	const myCapability = $derived($currentIdentity?.capability ?? '');
	const isAdmin = $derived(!$authEnabled || myCapability === 'Owner' || myCapability === 'Admin');
	const isOwner = $derived(!$authEnabled || myCapability === 'Owner');

	let nonceBeforeCreate = $state<string | undefined>(undefined);

	function createInvite() {
		nonceBeforeCreate = get(lastCreatedInvite)?.nonce;
		creatingInvite = true;
		const msg: Record<string, unknown> = {
			type: 'CreateInvite',
			capability: inviteCapability,
			max_uses: inviteMaxUses,
			expires_in_secs: inviteExpiry,
		};
		if (inviteLabel.trim()) {
			msg.label = inviteLabel.trim();
		}
		sendMuxMessage(msg);
	}

	$effect(() => {
		if (creatingInvite && $lastCreatedInvite && $lastCreatedInvite.nonce !== nonceBeforeCreate) {
			creatingInvite = false;
			expandedInvite = $lastCreatedInvite.nonce;
			inviteLabel = '';
		}
	});

	$effect(() => {
		if (creatingInvite) {
			const timeout = setTimeout(() => { creatingInvite = false; }, 5000);
			return () => clearTimeout(timeout);
		}
	});

	function revokeInvite(nonce: string) {
		sendMuxMessage({ type: 'RevokeInvite', nonce });
	}

	function suspendMember(pk: string) {
		sendMuxMessage({ type: 'SuspendMember', public_key: pk });
	}

	function reinstateMember(pk: string) {
		sendMuxMessage({ type: 'ReinstateMember', public_key: pk });
	}

	function removeMember(pk: string) {
		sendMuxMessage({ type: 'RemoveMember', public_key: pk });
	}

	let copiedNonce = $state<string | null>(null);
	let expandedInvite = $state<string | null>(null);

	function toggleInvite(nonce: string) {
		expandedInvite = expandedInvite === nonce ? null : nonce;
	}

	function inviteUrl(token: string): string {
		return `${window.location.origin}/join#${token}`;
	}

	function copyInviteLink(token: string) {
		navigator.clipboard.writeText(inviteUrl(token));
		copiedNonce = token;
		setTimeout(() => { copiedNonce = null; }, 2000);
	}

	function formatExpiry(expiresAt?: string): string {
		if (!expiresAt) return 'No expiry';
		const exp = new Date(expiresAt);
		const now = Date.now();
		const diff = exp.getTime() - now;
		if (diff <= 0) return 'Expired';
		const hours = Math.floor(diff / 3600000);
		const days = Math.floor(hours / 24);
		if (days > 0) return `${days}d ${hours % 24}h remaining`;
		if (hours > 0) return `${hours}h remaining`;
		const mins = Math.floor(diff / 60000);
		return `${mins}m remaining`;
	}

	function stateBadgeClass(state: string): string {
		switch (state) {
			case 'revoked': return 'badge-suspended';
			case 'expired': return 'badge-view';
			case 'exhausted': return 'badge-view';
			default: return 'badge-view';
		}
	}

	function capBadgeClass(cap: string): string {
		switch (cap) {
			case 'Owner': return 'badge-owner';
			case 'Admin': return 'badge-admin';
			case 'Collaborate': return 'badge-collab';
			default: return 'badge-view';
		}
	}

	// Ensure the mux WS is connected (standalone pages skip layout init_app)
	initMultiplexedConnection();

	$effect(() => {
		if ($muxConnected) {
			sendMuxMessage({ type: 'ListMembers' });
			sendMuxMessage({ type: 'ListInvites' });
		}
	});
</script>

<div class="members-page">
	<header class="members-header">
		<a href="{base}/settings" class="back-link">
			<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
				<path d="M19 12H5M12 19l-7-7 7-7" />
			</svg>
			Settings
		</a>
		<h1>Members</h1>
		<div class="header-spacer"></div>
	</header>

	<div class="members-content">
		{#if !$muxConnected}
			<div class="empty-state">
				<p>Connecting...</p>
			</div>
		{:else}
			<!-- Member list -->
			<section class="section">
				<h2 class="section-title">Active Members ({memberList.length})</h2>
				<div class="member-list">
					{#each memberList as member (member.fingerprint)}
						<div class="member-card" class:suspended={member.state === 'suspended'}>
							<div class="member-info">
								<code class="member-fp">{member.fingerprint}</code>
								<span class="member-name">{member.display_name}</span>
								<span class="badge {capBadgeClass(member.capability)}">{member.capability}</span>
								{#if member.state === 'suspended'}
									<span class="badge badge-suspended">Suspended</span>
								{/if}
								{#if member.fingerprint === myFingerprint}
									<span class="badge badge-you">You</span>
								{/if}
							</div>

							{#if isAdmin && member.fingerprint !== myFingerprint && member.capability !== 'Owner'}
								{@const pk = member.public_key}
								{#if pk}
									<div class="member-actions">
										{#if member.state === 'suspended'}
											<button class="action-sm" onclick={() => reinstateMember(pk)}>
												Reinstate
											</button>
										{:else}
											<button class="action-sm action-warn" onclick={() => suspendMember(pk)}>
												Suspend
											</button>
										{/if}
										<button class="action-sm action-danger" onclick={() => removeMember(pk)}>
											Remove
										</button>
									</div>
								{/if}
							{/if}
						</div>
					{:else}
						<p class="hint">No members yet</p>
					{/each}
				</div>
			</section>

			<!-- Invite section (admin only) -->
			{#if isAdmin}
				<section class="section">
					<h2 class="section-title">Create Invite</h2>

					<div class="invite-form">
						<div class="field-row">
							<label class="field-label" for="inv-cap">Capability</label>
							<select id="inv-cap" class="field-select" bind:value={inviteCapability}>
								<option value="View">View</option>
								<option value="Collaborate">Collaborate</option>
								{#if isOwner}
									<option value="Admin">Admin</option>
								{/if}
							</select>
						</div>

						<div class="field-row">
							<label class="field-label" for="inv-uses">Max Uses</label>
							<input id="inv-uses" type="number" class="field-input narrow" bind:value={inviteMaxUses} min="1" max="100" />
						</div>

						<div class="field-row">
							<label class="field-label" for="inv-exp">Expires</label>
							<select id="inv-exp" class="field-select" bind:value={inviteExpiry}>
								<option value={3600}>1 hour</option>
								<option value={86400}>24 hours</option>
								<option value={604800}>7 days</option>
							</select>
						</div>

						<div class="field-row">
							<label class="field-label" for="inv-label">Label</label>
							<input id="inv-label" type="text" class="field-input" bind:value={inviteLabel} placeholder="e.g. For Bob, QA team..." />
						</div>

						<button class="create-btn" onclick={createInvite} disabled={creatingInvite}>
							{creatingInvite ? 'Creating...' : 'Create Invite'}
						</button>
					</div>

				</section>

				{#if activeInvites.length > 0}
					<section class="section">
						<h2 class="section-title">Active Invites ({activeInvites.length})</h2>
						<div class="invite-list">
							{#each activeInvites as inv (inv.nonce)}
								<div class="invite-card" class:expanded={expandedInvite === inv.nonce}>
									<button class="invite-row" onclick={() => toggleInvite(inv.nonce)}>
										<div class="invite-info">
											<span class="badge {capBadgeClass(inv.capability)}">{inv.capability}</span>
											{#if inv.label}
												<span class="invite-label">{inv.label}</span>
											{/if}
											<span class="invite-meta">
												{inv.use_count ?? 0}/{inv.max_uses} uses
											</span>
											<span class="invite-meta">{formatExpiry(inv.expires_at)}</span>
										</div>
										<svg class="expand-chevron" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
											<path d="M6 9l6 6 6-6" />
										</svg>
									</button>
									{#if expandedInvite === inv.nonce}
										{@const token = inv.token ?? inv.nonce}
										{@const copied = copiedNonce === token}
										<div class="invite-detail">
											<button class="invite-link-btn" onclick={() => copyInviteLink(token)}>
												<code class="invite-link" class:copied>{copied ? 'Copied to clipboard' : inviteUrl(token)}</code>
											</button>
											<div class="invite-actions">
												<button class="action-sm" onclick={() => copyInviteLink(token)}>
													{copied ? 'Copied' : 'Copy Link'}
												</button>
												<button class="action-sm action-warn" onclick={() => revokeInvite(inv.nonce)}>
													Revoke
												</button>
											</div>
										</div>
									{/if}
								</div>
							{/each}
						</div>
					</section>
				{/if}

				{#if inactiveInvites.length > 0}
					<section class="section">
						<button class="section-toggle" onclick={() => showInactive = !showInactive}>
							<h2 class="section-title" style="margin:0">
								Inactive Invites ({inactiveInvites.length})
							</h2>
							<svg class="expand-chevron" class:open={showInactive} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
								<path d="M6 9l6 6 6-6" />
							</svg>
						</button>

						{#if showInactive}
							<div class="invite-list" style="margin-top: 12px">
								{#each inactiveInvites as inv (inv.nonce)}
									<div class="invite-card inactive">
										<div class="invite-row">
											<div class="invite-info">
												<span class="badge {capBadgeClass(inv.capability)}">{inv.capability}</span>
												{#if inv.label}
													<span class="invite-label">{inv.label}</span>
												{/if}
												<span class="badge {stateBadgeClass(inv.state ?? '')}">{inv.state}</span>
												<span class="invite-meta">
													{inv.use_count ?? 0}/{inv.max_uses} uses
												</span>
											</div>
										</div>
									</div>
								{/each}
							</div>
						{/if}
					</section>
				{/if}
			{/if}
		{/if}
	</div>
</div>

<style>
	.members-page {
		display: flex;
		flex-direction: column;
		height: 100vh;
		height: 100dvh;
		background: var(--surface-800);
	}

	.members-header {
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

	.members-header h1 {
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

	.members-content {
		flex: 1;
		overflow-y: auto;
		padding: 20px;
		width: 100%;
		max-width: 600px;
		margin: 0 auto;
	}

	.members-content::-webkit-scrollbar { width: 8px; }
	.members-content::-webkit-scrollbar-track { background: transparent; }
	.members-content::-webkit-scrollbar-thumb { background: var(--surface-border); border-radius: 4px; }
	.members-content::-webkit-scrollbar-thumb:hover { background: var(--amber-600); }

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

	/* Empty / disconnected state */
	.empty-state {
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		height: 100%;
		color: var(--text-muted);
		text-align: center;
		font-size: 12px;
	}

	.empty-state p { margin: 0 0 4px; }

	.hint {
		font-size: 12px;
		color: var(--text-muted);
	}

	/* Member list */
	.member-list {
		display: flex;
		flex-direction: column;
		gap: 4px;
	}

	.member-card {
		padding: 10px 12px;
		background: var(--surface-900);
		border: 1px solid var(--surface-border);
		border-radius: 3px;
		transition: border-color 0.15s ease;
	}

	.member-card:hover { border-color: var(--surface-border-light); }

	.member-card.suspended { opacity: 0.6; }

	.member-info {
		display: flex;
		align-items: center;
		gap: 8px;
		flex-wrap: wrap;
	}

	.member-fp {
		font-size: 11px;
		color: var(--amber-400);
		letter-spacing: 0.02em;
	}

	.member-name {
		font-size: 12px;
		color: var(--text-secondary);
	}

	.badge {
		display: inline-block;
		padding: 1px 5px;
		font-size: 10px;
		font-weight: 700;
		letter-spacing: 0.06em;
		border-radius: 2px;
		text-transform: uppercase;
	}

	.badge-owner {
		background: rgba(251, 146, 60, 0.2);
		color: var(--amber-400);
		border: 1px solid rgba(251, 146, 60, 0.3);
	}

	.badge-admin {
		background: rgba(139, 92, 246, 0.15);
		color: var(--purple-400);
		border: 1px solid rgba(139, 92, 246, 0.25);
	}

	.badge-collab {
		background: rgba(34, 197, 94, 0.12);
		color: var(--status-green);
		border: 1px solid rgba(34, 197, 94, 0.2);
	}

	.badge-view {
		background: rgba(160, 128, 96, 0.12);
		color: var(--text-muted);
		border: 1px solid rgba(160, 128, 96, 0.2);
	}

	.badge-suspended {
		background: rgba(239, 68, 68, 0.12);
		color: var(--status-red);
		border: 1px solid rgba(239, 68, 68, 0.2);
	}

	.badge-you {
		background: rgba(251, 146, 60, 0.1);
		color: var(--text-muted);
		border: 1px solid rgba(251, 146, 60, 0.15);
		font-size: 9px;
	}

	.member-actions {
		display: flex;
		gap: 4px;
		margin-top: 8px;
	}

	.action-sm {
		padding: 4px 10px;
		background: none;
		border: 1px solid var(--surface-border);
		border-radius: 2px;
		color: var(--text-muted);
		font-family: inherit;
		font-size: 11px;
		font-weight: 600;
		letter-spacing: 0.05em;
		text-transform: uppercase;
		cursor: pointer;
		transition: all 0.15s ease;
	}

	.action-sm:hover {
		background: var(--tint-hover);
		border-color: var(--surface-border-light);
		color: var(--text-secondary);
	}

	.action-sm.action-warn:hover {
		border-color: var(--status-yellow);
		color: var(--status-yellow);
	}

	.action-sm.action-danger:hover {
		border-color: var(--status-red);
		color: var(--status-red);
	}

	/* Invite form */
	.invite-form {
		display: flex;
		flex-direction: column;
		gap: 8px;
	}

	.field-row {
		display: flex;
		align-items: center;
		gap: 12px;
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

	.field-select,
	.field-input {
		flex: 1;
		padding: 5px 10px;
		background: var(--surface-900);
		border: 1px solid var(--surface-border);
		border-radius: 3px;
		color: var(--text-primary);
		font-size: 12px;
		font-family: inherit;
		outline: none;
	}

	.field-select:focus,
	.field-input:focus { border-color: var(--amber-600); }

	.field-input.narrow { max-width: 100px; }

	.field-input[type="number"]::-webkit-outer-spin-button,
	.field-input[type="number"]::-webkit-inner-spin-button {
		-webkit-appearance: none;
		margin: 0;
	}
	.field-input[type="number"] { -moz-appearance: textfield; }

	.create-btn {
		margin-top: 4px;
		padding: 8px 14px;
		background: var(--amber-600);
		border: none;
		border-radius: 3px;
		color: var(--surface-900);
		font-family: inherit;
		font-weight: 700;
		font-size: 11px;
		letter-spacing: 0.08em;
		text-transform: uppercase;
		cursor: pointer;
		transition: all 0.15s ease;
	}

	.create-btn:hover:not(:disabled) { background: var(--amber-500); }

	.create-btn:disabled { opacity: 0.4; cursor: not-allowed; }

	/* Invite list */
	.invite-list {
		display: flex;
		flex-direction: column;
		gap: 4px;
	}

	.invite-card {
		display: flex;
		flex-direction: column;
		background: var(--surface-900);
		border: 1px solid var(--surface-border);
		border-radius: 3px;
		transition: border-color 0.15s ease;
	}

	.invite-card:hover:not(.inactive) { border-color: var(--surface-border-light); }
	.invite-card.expanded { border-color: var(--amber-600); }
	.invite-card.inactive { opacity: 0.5; }

	.invite-row {
		display: flex;
		align-items: center;
		justify-content: space-between;
		width: 100%;
		padding: 8px 12px;
		background: none;
		border: none;
		cursor: pointer;
		text-align: left;
		font-family: inherit;
		font-size: inherit;
		color: inherit;
	}

	.expand-chevron {
		width: 14px;
		height: 14px;
		color: var(--text-muted);
		flex-shrink: 0;
		transition: transform 0.15s ease;
	}

	.invite-card.expanded .expand-chevron { transform: rotate(180deg); }
	.expand-chevron.open { transform: rotate(180deg); }

	.section-toggle {
		display: flex;
		align-items: center;
		justify-content: space-between;
		width: 100%;
		background: none;
		border: none;
		padding: 0;
		cursor: pointer;
		color: inherit;
		font-family: inherit;
	}

	.section-toggle:hover .section-title { color: var(--text-secondary); }
	.section-toggle:hover .expand-chevron { color: var(--text-secondary); }

	.invite-info {
		display: flex;
		align-items: center;
		gap: 8px;
	}

	.invite-label {
		font-size: 12px;
		font-weight: 600;
		color: var(--text-primary);
	}

	.invite-meta {
		font-size: 11px;
		color: var(--text-muted);
	}

	.invite-detail {
		padding: 0 12px 8px;
		border-top: 1px solid var(--surface-border);
		display: flex;
		flex-direction: column;
		gap: 8px;
		margin-top: 0;
		padding-top: 8px;
	}

	.invite-link-btn {
		display: block;
		width: 100%;
		padding: 6px 8px;
		background: var(--surface-800);
		border: 1px dashed var(--surface-border);
		border-radius: 2px;
		cursor: pointer;
		text-align: left;
		transition: all 0.15s ease;
	}

	.invite-link-btn:hover {
		border-color: var(--amber-600);
		background: rgba(251, 146, 60, 0.05);
	}

	.invite-link {
		font-size: 11px;
		font-family: var(--font-mono, monospace);
		color: var(--amber-400);
		word-break: break-all;
	}

	.invite-link.copied {
		color: var(--status-green, #22c55e);
	}

	.invite-actions {
		display: flex;
		gap: 4px;
	}

	/* Responsive */
	@media (max-width: 639px) {
		.members-header { padding: 12px 14px; gap: 10px; }
		.members-header h1 { font-size: 12px; }
		.members-content { padding: 12px 14px; }
		.header-spacer { display: none; }
		.field-label { width: 80px; }
	}

	/* Analog theme */
	:global([data-theme="analog"]) .members-header {
		background-color: var(--surface-800);
		background-image: var(--grain-fine), var(--grain-coarse);
		background-blend-mode: multiply, multiply;
		border-bottom-width: 2px;
	}

	:global([data-theme="analog"]) .members-header h1 {
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

	:global([data-theme="analog"]) .create-btn {
		font-family: 'Source Serif 4', Georgia, serif;
		text-transform: none;
		letter-spacing: 0;
	}
</style>
