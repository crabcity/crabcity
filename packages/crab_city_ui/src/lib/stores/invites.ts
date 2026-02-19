/**
 * Invites Store
 *
 * Tracks invite tokens for admin panel display.
 */

import { writable } from 'svelte/store';

export interface InviteInfo {
	nonce: string;
	token?: string;
	capability: string;
	max_uses: number;
	use_count?: number;
	expires_at?: string;
	created_at?: string;
	state?: string;
	label?: string;
}

export const invites = writable<InviteInfo[]>([]);

/** The most recently created invite (for copy-to-clipboard). */
export const lastCreatedInvite = writable<InviteInfo | null>(null);

/** Handle interconnect invite messages from ws-handlers. */
export function handleInvitesMessage(msg: { type: string; [key: string]: unknown }): void {
	switch (msg.type) {
		case 'InviteCreated': {
			const inv = msg as unknown as { type: string; nonce: string; token?: string; capability: string; max_uses: number; expires_at?: string; label?: string };
			const info: InviteInfo = {
				nonce: inv.nonce,
				capability: inv.capability,
				max_uses: inv.max_uses,
				state: 'active',
				created_at: new Date().toISOString(),
				...(inv.token !== undefined ? { token: inv.token } : {}),
				...(inv.expires_at !== undefined ? { expires_at: inv.expires_at } : {}),
				...(inv.label !== undefined ? { label: inv.label } : {}),
			};
			invites.update((list) => [...list, info]);
			lastCreatedInvite.set(info);
			break;
		}
		case 'InviteRevoked': {
			const rev = msg as { nonce: string; type: string };
			invites.update((list) =>
				list.map((i) =>
					i.nonce === rev.nonce ? { ...i, state: 'revoked' } : i
				)
			);
			break;
		}
		case 'InviteList': {
			const list = (msg['invites'] as InviteInfo[]).map((inv) => ({
				...inv,
				state: inv.state ?? 'active',
				token: inv.token ?? inv.nonce,
			}));
			invites.set(list);
			break;
		}
	}
}
