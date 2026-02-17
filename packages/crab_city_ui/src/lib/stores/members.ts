/**
 * Members Store
 *
 * Tracks the membership list received from interconnect RPCs.
 */

import { writable, get } from 'svelte/store';

// =============================================================================
// Member Panel UI State
// =============================================================================

export const isMemberPanelOpen = writable<boolean>(false);

export function openMemberPanel(): void { isMemberPanelOpen.set(true); }
export function closeMemberPanel(): void { isMemberPanelOpen.set(false); }
export function toggleMemberPanel(): void {
	if (get(isMemberPanelOpen)) {
		closeMemberPanel();
	} else {
		openMemberPanel();
	}
}

// =============================================================================
// Member Data
// =============================================================================

export interface Member {
	fingerprint: string;
	display_name: string;
	capability: string;
	state: string;
	public_key?: string;
}

export const members = writable<Map<string, Member>>(new Map());

/** Handle interconnect membership messages from ws-handlers. */
export function handleMembersMessage(msg: { type: string; [key: string]: unknown }): void {
	switch (msg.type) {
		case 'MembersList': {
			const list = msg['members'] as Member[];
			members.set(new Map(list.map((m) => [m.fingerprint, m])));
			break;
		}
		case 'MemberJoined': {
			// Server sends { type: "MemberJoined", member: { public_key, fingerprint, ... } }
			const m = msg['member'] as Member;
			if (m?.fingerprint) {
				members.update((map) => {
					const entry: Member = {
						fingerprint: m.fingerprint,
						display_name: m.display_name,
						capability: m.capability,
						state: m.state ?? 'active',
					};
					if (m.public_key) entry.public_key = m.public_key;
					map.set(m.fingerprint, entry);
					return new Map(map);
				});
			}
			break;
		}
		case 'MemberUpdated': {
			// Server sends { type: "MemberUpdated", member: { public_key, fingerprint, ... } }
			const u = msg['member'] as { fingerprint: string; capability?: string; display_name?: string };
			if (u?.fingerprint) {
				members.update((map) => {
					const existing = map.get(u.fingerprint);
					if (existing) {
						map.set(u.fingerprint, {
							...existing,
							...(u.capability ? { capability: u.capability } : {}),
							...(u.display_name ? { display_name: u.display_name } : {}),
						});
					}
					return new Map(map);
				});
			}
			break;
		}
		case 'MemberSuspended': {
			const s = msg as { fingerprint: string; type: string };
			members.update((map) => {
				const existing = map.get(s.fingerprint);
				if (existing) {
					map.set(s.fingerprint, { ...existing, state: 'suspended' });
				}
				return new Map(map);
			});
			break;
		}
		case 'MemberReinstated': {
			const r = msg as { fingerprint: string; type: string };
			members.update((map) => {
				const existing = map.get(r.fingerprint);
				if (existing) {
					map.set(r.fingerprint, { ...existing, state: 'active' });
				}
				return new Map(map);
			});
			break;
		}
		case 'MemberRemoved': {
			const rm = msg as { fingerprint: string; type: string };
			members.update((map) => {
				map.delete(rm.fingerprint);
				return new Map(map);
			});
			break;
		}
		case 'InviteRedeemed': {
			// A new member joined via invite — add them
			const ir = msg as { fingerprint: string; display_name: string; capability: string; public_key?: string; type: string };
			members.update((map) => {
				const entry: Member = {
					fingerprint: ir.fingerprint,
					display_name: ir.display_name,
					capability: ir.capability,
					state: 'active',
				};
				if (ir.public_key) entry.public_key = ir.public_key;
				map.set(ir.fingerprint, entry);
				return new Map(map);
			});
			break;
		}
	}
}
