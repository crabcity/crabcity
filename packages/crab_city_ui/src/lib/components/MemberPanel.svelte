<script lang="ts">
	import { get } from 'svelte/store';
	import { members, type Member, closeMemberPanel } from '$lib/stores/members';
	import { invites, lastCreatedInvite, type InviteInfo } from '$lib/stores/invites';
	import { currentIdentity } from '$lib/stores/auth';
	import { isConnected, sendMuxMessage } from '$lib/stores/websocket';

	interface Props {
		visible: boolean;
	}

	let { visible }: Props = $props();

	// Invite creation form
	let inviteCapability = $state('Collaborate');
	let inviteMaxUses = $state(5);
	let inviteExpiry = $state(3600); // seconds
	let creatingInvite = $state(false);

	// Copy feedback
	let copiedToken = $state(false);

	const memberList = $derived(
		Array.from(($members).values()).sort((a, b) => {
			const capOrder = ['Owner', 'Admin', 'Collaborate', 'View'];
			return capOrder.indexOf(a.capability) - capOrder.indexOf(b.capability);
		})
	);

	const activeInvites = $derived(
		($invites).filter(i => i.state === 'active')
	);

	const myFingerprint = $derived($currentIdentity?.fingerprint ?? '');
	const myCapability = $derived($currentIdentity?.capability ?? '');
	const isAdmin = $derived(myCapability === 'Owner' || myCapability === 'Admin');

	// Snapshot the nonce before sending so we can detect when a *new* invite arrives
	let nonceBeforeCreate = $state<string | undefined>(undefined);

	function createInvite() {
		nonceBeforeCreate = get(lastCreatedInvite)?.nonce;
		creatingInvite = true;
		sendMuxMessage({
			type: 'CreateInvite',
			capability: inviteCapability,
			max_uses: inviteMaxUses,
			expires_in_secs: inviteExpiry,
		});
	}

	// Reset button when a new invite arrives (nonce differs from snapshot)
	$effect(() => {
		if (creatingInvite && $lastCreatedInvite && $lastCreatedInvite.nonce !== nonceBeforeCreate) {
			creatingInvite = false;
		}
	});
	// Fallback timeout in case server response is lost
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

	function copyInviteToken() {
		const last = get(lastCreatedInvite);
		if (last?.token) {
			const joinUrl = `${window.location.origin}/join#${last.token}`;
			navigator.clipboard.writeText(joinUrl);
			copiedToken = true;
			setTimeout(() => { copiedToken = false; }, 2000);
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

	function requestMembers() {
		sendMuxMessage({ type: 'ListMembers' });
		sendMuxMessage({ type: 'ListInvites' });
	}

	$effect(() => {
		if (visible && isConnected()) {
			requestMembers();
		}
	});
</script>

{#if visible}
	<!-- svelte-ignore a11y_click_events_have_key_events -->
	<!-- svelte-ignore a11y_no_static_element_interactions -->
	<div class="panel-backdrop" onclick={closeMemberPanel}></div>

	<aside class="panel">
		<header class="panel-header">
			<h2>MEMBERS</h2>
			<button class="close-btn" onclick={closeMemberPanel} aria-label="Close">
				<svg width="14" height="14" viewBox="0 0 14 14" fill="none">
					<path d="M1 1L13 13M13 1L1 13" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>
				</svg>
			</button>
		</header>

		<div class="panel-body">
			<!-- Member list -->
			<section class="section">
				<h3 class="section-label">ACTIVE MEMBERS ({memberList.length})</h3>
				<div class="member-list">
					{#each memberList as member (member.fingerprint)}
						<div class="member-card" class:suspended={member.state === 'suspended'}>
							<div class="member-info">
								<code class="member-fp">{member.fingerprint}</code>
								<span class="member-name">{member.display_name}</span>
								<span class="badge {capBadgeClass(member.capability)}">{member.capability}</span>
								{#if member.state === 'suspended'}
									<span class="badge badge-suspended">SUSPENDED</span>
								{/if}
								{#if member.fingerprint === myFingerprint}
									<span class="badge badge-you">YOU</span>
								{/if}
							</div>

							{#if isAdmin && member.fingerprint !== myFingerprint && member.capability !== 'Owner'}
								{@const pk = member.public_key}
								{#if pk}
									<div class="member-actions">
										{#if member.state === 'suspended'}
											<button class="action-sm" onclick={() => reinstateMember(pk)}>
												REINSTATE
											</button>
										{:else}
											<button class="action-sm action-warn" onclick={() => suspendMember(pk)}>
												SUSPEND
											</button>
										{/if}
										<button class="action-sm action-danger" onclick={() => removeMember(pk)}>
											REMOVE
										</button>
									</div>
								{/if}
							{/if}
						</div>
					{:else}
						<p class="empty">No members yet</p>
					{/each}
				</div>
			</section>

			<!-- Invite section (admin only) -->
			{#if isAdmin}
				<section class="section">
					<h3 class="section-label">CREATE INVITE</h3>

					<div class="invite-form">
						<label class="inline-field">
							<span>CAPABILITY</span>
							<select bind:value={inviteCapability}>
								<option value="View">View</option>
								<option value="Collaborate">Collaborate</option>
								{#if myCapability === 'Owner'}
									<option value="Admin">Admin</option>
								{/if}
							</select>
						</label>

						<div class="invite-row">
							<label class="inline-field half">
								<span>MAX USES</span>
								<input type="number" bind:value={inviteMaxUses} min="1" max="100" />
							</label>
							<label class="inline-field half">
								<span>EXPIRES</span>
								<select bind:value={inviteExpiry}>
									<option value={3600}>1 hour</option>
									<option value={86400}>24 hours</option>
									<option value={604800}>7 days</option>
								</select>
							</label>
						</div>

						<button class="create-btn" onclick={createInvite} disabled={creatingInvite}>
							{creatingInvite ? 'CREATING...' : 'CREATE INVITE'}
						</button>
					</div>

					{#if $lastCreatedInvite?.token}
						<div class="invite-result">
							<span class="section-label" style="margin-bottom: 4px">INVITE LINK</span>
							<div class="invite-token-row">
								<code class="invite-token">{$lastCreatedInvite.token.slice(0, 16)}...</code>
								<button class="copy-btn" onclick={copyInviteToken}>
									{copiedToken ? 'COPIED' : 'COPY LINK'}
								</button>
							</div>
						</div>
					{/if}
				</section>

				{#if activeInvites.length > 0}
					<section class="section">
						<h3 class="section-label">ACTIVE INVITES ({activeInvites.length})</h3>
						<div class="invite-list">
							{#each activeInvites as inv (inv.nonce)}
								<div class="invite-card">
									<div class="invite-info">
										<span class="badge {capBadgeClass(inv.capability)}">{inv.capability}</span>
										<span class="invite-meta">
											{inv.use_count ?? 0}/{inv.max_uses} uses
										</span>
									</div>
									<button class="action-sm action-warn" onclick={() => revokeInvite(inv.nonce)}>
										REVOKE
									</button>
								</div>
							{/each}
						</div>
					</section>
				{/if}
			{/if}
		</div>
	</aside>
{/if}

<style>
	.panel-backdrop {
		position: fixed;
		inset: 0;
		background: var(--backdrop);
		z-index: 55;
		animation: fade-in 0.15s ease;
	}

	@keyframes fade-in {
		from { opacity: 0; }
		to { opacity: 1; }
	}

	.panel {
		position: fixed;
		top: 0;
		right: 0;
		bottom: 0;
		width: 380px;
		z-index: 56;
		display: flex;
		flex-direction: column;
		background: linear-gradient(180deg, var(--surface-700) 0%, var(--surface-800) 100%);
		border-left: 1px solid var(--surface-border);
		box-shadow: var(--shadow-panel);
		animation: slide-in 0.25s cubic-bezier(0.4, 0, 0.2, 1);
	}

	@keyframes slide-in {
		from { transform: translateX(100%); }
		to { transform: translateX(0); }
	}

	.panel-header {
		flex-shrink: 0;
		display: flex;
		align-items: center;
		justify-content: space-between;
		padding: 14px 16px;
		border-bottom: 1px solid var(--surface-border);
		background: var(--surface-700);
	}

	.panel-header h2 {
		margin: 0;
		font-size: 0.75em;
		font-weight: 700;
		letter-spacing: 0.15em;
		color: var(--amber-400);
	}

	.close-btn {
		display: flex;
		align-items: center;
		justify-content: center;
		width: 28px;
		height: 28px;
		background: none;
		border: 1px solid transparent;
		border-radius: 3px;
		color: var(--text-muted);
		cursor: pointer;
		transition: all 0.15s ease;
	}

	.close-btn:hover {
		background: var(--tint-hover);
		border-color: var(--surface-border);
		color: var(--text-primary);
	}

	.panel-body {
		flex: 1;
		overflow-y: auto;
		padding: 16px;
	}

	.section {
		margin-bottom: 20px;
	}

	.section-label {
		display: block;
		font-size: 0.6em;
		font-weight: 700;
		letter-spacing: 0.12em;
		color: var(--text-muted);
		margin-bottom: 8px;
	}

	.member-list {
		display: flex;
		flex-direction: column;
		gap: 4px;
	}

	.member-card {
		padding: 8px 10px;
		background: var(--surface-800);
		border: 1px solid var(--surface-border);
		border-radius: 3px;
		transition: border-color 0.15s ease;
	}

	.member-card:hover {
		border-color: var(--surface-border-light);
	}

	.member-card.suspended {
		opacity: 0.6;
	}

	.member-info {
		display: flex;
		align-items: center;
		gap: 6px;
		flex-wrap: wrap;
	}

	.member-fp {
		font-size: 0.7em;
		color: var(--amber-400);
		letter-spacing: 0.02em;
	}

	.member-name {
		font-size: 0.75em;
		color: var(--text-secondary);
	}

	.badge {
		display: inline-block;
		padding: 1px 5px;
		font-size: 0.55em;
		font-weight: 700;
		letter-spacing: 0.08em;
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
		font-size: 0.5em;
	}

	.member-actions {
		display: flex;
		gap: 4px;
		margin-top: 6px;
	}

	.action-sm {
		padding: 3px 8px;
		background: none;
		border: 1px solid var(--surface-border);
		border-radius: 2px;
		color: var(--text-muted);
		font-family: inherit;
		font-size: 0.58em;
		font-weight: 700;
		letter-spacing: 0.06em;
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

	.empty {
		font-size: 0.75em;
		color: var(--text-muted);
		font-style: italic;
	}

	/* Invite form */
	.invite-form {
		display: flex;
		flex-direction: column;
		gap: 8px;
	}

	.invite-row {
		display: flex;
		gap: 8px;
	}

	.half {
		flex: 1;
	}

	.inline-field {
		display: flex;
		flex-direction: column;
		gap: 3px;
	}

	.inline-field span {
		font-size: 0.58em;
		font-weight: 700;
		letter-spacing: 0.1em;
		color: var(--text-muted);
	}

	.inline-field select,
	.inline-field input {
		padding: 5px 8px;
		background: var(--surface-900);
		border: 1px solid var(--surface-border);
		border-radius: 3px;
		color: var(--text-primary);
		font-family: inherit;
		font-size: 0.78em;
	}

	.inline-field select:focus,
	.inline-field input:focus {
		outline: none;
		border-color: var(--amber-600);
	}

	.create-btn {
		padding: 7px;
		margin-top: 2px;
		background: var(--amber-600);
		border: none;
		border-radius: 3px;
		color: var(--surface-900);
		font-family: inherit;
		font-weight: 700;
		font-size: 0.68em;
		letter-spacing: 0.1em;
		cursor: pointer;
		transition: all 0.15s ease;
	}

	.create-btn:hover:not(:disabled) {
		background: var(--amber-500);
	}

	.create-btn:disabled {
		opacity: 0.4;
		cursor: not-allowed;
	}

	.invite-result {
		margin-top: 10px;
		padding: 8px 10px;
		background: var(--surface-900);
		border: 1px solid var(--amber-600);
		border-radius: 3px;
	}

	.invite-token-row {
		display: flex;
		align-items: center;
		gap: 8px;
	}

	.invite-token {
		flex: 1;
		font-size: 0.72em;
		color: var(--text-primary);
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.copy-btn {
		padding: 3px 8px;
		background: linear-gradient(180deg, var(--surface-500) 0%, var(--surface-600) 100%);
		border: 1px solid var(--surface-border);
		border-radius: 2px;
		color: var(--text-secondary);
		font-family: inherit;
		font-size: 0.6em;
		font-weight: 700;
		letter-spacing: 0.06em;
		cursor: pointer;
		transition: all 0.15s ease;
		flex-shrink: 0;
	}

	.copy-btn:hover {
		border-color: var(--amber-600);
		color: var(--amber-400);
	}

	.invite-list {
		display: flex;
		flex-direction: column;
		gap: 4px;
	}

	.invite-card {
		display: flex;
		align-items: center;
		justify-content: space-between;
		padding: 6px 10px;
		background: var(--surface-800);
		border: 1px solid var(--surface-border);
		border-radius: 3px;
	}

	.invite-info {
		display: flex;
		align-items: center;
		gap: 8px;
	}

	.invite-meta {
		font-size: 0.68em;
		color: var(--text-muted);
	}

	/* Responsive */
	@media (max-width: 639px) {
		.panel {
			width: 100%;
		}
	}

	@media (min-width: 640px) and (max-width: 1023px) {
		.panel {
			width: 85vw;
			max-width: 420px;
		}
	}
</style>
