/**
 * Tests for the invites store.
 *
 * Covers: InviteCreated, InviteRevoked, InviteList message handling,
 * and the lastCreatedInvite store.
 */

import { get } from 'svelte/store';
import { invites, lastCreatedInvite, handleInvitesMessage, type InviteInfo } from './invites.js';

function resetStores(): void {
	invites.set([]);
	lastCreatedInvite.set(null);
}

beforeEach(resetStores);

// =============================================================================
// InviteCreated
// =============================================================================

describe('InviteCreated', () => {
	it('appends an invite to the list', () => {
		handleInvitesMessage({
			type: 'InviteCreated',
			nonce: 'abc123',
			token: 'tok_secret',
			capability: 'Collaborate',
			max_uses: 5,
			expires_at: '2026-12-31T23:59:59Z',
		});

		const list = get(invites);
		expect(list.length).toBe(1);
		expect(list[0]?.nonce).toBe('abc123');
		expect(list[0]?.capability).toBe('Collaborate');
		expect(list[0]?.max_uses).toBe(5);
		expect(list[0]?.state).toBe('active');
	});

	it('sets lastCreatedInvite', () => {
		handleInvitesMessage({
			type: 'InviteCreated',
			nonce: 'xyz789',
			token: 'tok_other',
			capability: 'Admin',
			max_uses: 1,
		});

		const last = get(lastCreatedInvite);
		expect(last).not.toBeNull();
		expect(last?.nonce).toBe('xyz789');
		expect(last?.token).toBe('tok_other');
	});

	it('appends multiple invites', () => {
		handleInvitesMessage({
			type: 'InviteCreated',
			nonce: 'inv1',
			capability: 'View',
			max_uses: 10,
		});
		handleInvitesMessage({
			type: 'InviteCreated',
			nonce: 'inv2',
			capability: 'Collaborate',
			max_uses: 3,
		});

		const list = get(invites);
		expect(list.length).toBe(2);
		expect(list[0]?.nonce).toBe('inv1');
		expect(list[1]?.nonce).toBe('inv2');
	});

	it('lastCreatedInvite tracks the most recent', () => {
		handleInvitesMessage({
			type: 'InviteCreated',
			nonce: 'first',
			capability: 'View',
			max_uses: 1,
		});
		handleInvitesMessage({
			type: 'InviteCreated',
			nonce: 'second',
			capability: 'Admin',
			max_uses: 1,
		});

		expect(get(lastCreatedInvite)?.nonce).toBe('second');
	});
});

// =============================================================================
// InviteRevoked
// =============================================================================

describe('InviteRevoked', () => {
	beforeEach(() => {
		handleInvitesMessage({
			type: 'InviteCreated',
			nonce: 'active1',
			capability: 'Collaborate',
			max_uses: 5,
		});
		handleInvitesMessage({
			type: 'InviteCreated',
			nonce: 'active2',
			capability: 'View',
			max_uses: 10,
		});
	});

	it('marks matching invite as revoked', () => {
		handleInvitesMessage({ type: 'InviteRevoked', nonce: 'active1' });

		const list = get(invites);
		expect(list.length).toBe(2);
		expect(list[0]?.state).toBe('revoked');
		expect(list[1]?.state).toBe('active');
	});

	it('leaves non-matching invites unchanged', () => {
		handleInvitesMessage({ type: 'InviteRevoked', nonce: 'nonexistent' });

		const list = get(invites);
		expect(list[0]?.state).toBe('active');
		expect(list[1]?.state).toBe('active');
	});
});

// =============================================================================
// InviteList
// =============================================================================

describe('InviteList', () => {
	it('replaces all invites', () => {
		// Pre-populate
		handleInvitesMessage({
			type: 'InviteCreated',
			nonce: 'old',
			capability: 'View',
			max_uses: 1,
		});

		const newList: InviteInfo[] = [
			{ nonce: 'new1', capability: 'Admin', max_uses: 1, state: 'active' },
			{ nonce: 'new2', capability: 'Collaborate', max_uses: 5, state: 'revoked' },
		];

		handleInvitesMessage({ type: 'InviteList', invites: newList });

		const list = get(invites);
		expect(list.length).toBe(2);
		expect(list[0]?.nonce).toBe('new1');
		expect(list[1]?.state).toBe('revoked');
	});

	it('handles empty list', () => {
		handleInvitesMessage({
			type: 'InviteCreated',
			nonce: 'pre',
			capability: 'View',
			max_uses: 1,
		});
		handleInvitesMessage({ type: 'InviteList', invites: [] });

		expect(get(invites).length).toBe(0);
	});
});

// =============================================================================
// Unknown message type
// =============================================================================

describe('unknown message type', () => {
	it('does not crash', () => {
		expect(() => {
			handleInvitesMessage({ type: 'SomeFutureMessage' });
		}).not.toThrow();
	});
});
