/**
 * Tests for the members store.
 *
 * Covers: MembersList, MemberJoined, MemberUpdated, MemberSuspended,
 * MemberReinstated, MemberRemoved, and InviteRedeemed message handling.
 */

import { get } from 'svelte/store';
import { members, handleMembersMessage, type Member } from './members.js';

function resetMembers(): void {
	members.set(new Map());
}

beforeEach(resetMembers);

// Helper to add a member via MemberJoined (server format: { member: { ... } })
function addMember(fingerprint: string, display_name: string, capability: string): void {
	handleMembersMessage({
		type: 'MemberJoined',
		member: { fingerprint, display_name, capability, state: 'active', public_key: fingerprint + '_pk' },
	});
}

// =============================================================================
// MembersList
// =============================================================================

describe('MembersList', () => {
	it('replaces the entire map', () => {
		const list: Member[] = [
			{ fingerprint: 'crab_AAAAAAAA', display_name: 'Alice', capability: 'Admin', state: 'active' },
			{ fingerprint: 'crab_BBBBBBBB', display_name: 'Bob', capability: 'Collaborate', state: 'active' },
		];
		handleMembersMessage({ type: 'MembersList', members: list });

		const m = get(members);
		expect(m.size).toBe(2);
		expect(m.get('crab_AAAAAAAA')?.display_name).toBe('Alice');
		expect(m.get('crab_BBBBBBBB')?.capability).toBe('Collaborate');
	});

	it('overwrites previous members', () => {
		members.set(new Map([['crab_OLD00000', { fingerprint: 'crab_OLD00000', display_name: 'Old', capability: 'View', state: 'active' }]]));

		handleMembersMessage({ type: 'MembersList', members: [
			{ fingerprint: 'crab_NEW00000', display_name: 'New', capability: 'Admin', state: 'active' },
		] });

		const m = get(members);
		expect(m.size).toBe(1);
		expect(m.has('crab_OLD00000')).toBe(false);
		expect(m.has('crab_NEW00000')).toBe(true);
	});

	it('handles empty list', () => {
		members.set(new Map([['crab_X0000000', { fingerprint: 'crab_X0000000', display_name: 'X', capability: 'View', state: 'active' }]]));

		handleMembersMessage({ type: 'MembersList', members: [] });
		expect(get(members).size).toBe(0);
	});
});

// =============================================================================
// MemberJoined
// =============================================================================

describe('MemberJoined', () => {
	it('adds a new member via { member: ... } wrapper', () => {
		addMember('crab_CCCCCCCC', 'Carol', 'Collaborate');

		const m = get(members);
		expect(m.size).toBe(1);
		const carol = m.get('crab_CCCCCCCC');
		expect(carol?.display_name).toBe('Carol');
		expect(carol?.state).toBe('active');
	});

	it('overwrites existing member with same fingerprint', () => {
		addMember('crab_DDDDDDDD', 'Dan v1', 'View');
		addMember('crab_DDDDDDDD', 'Dan v2', 'Admin');

		const m = get(members);
		expect(m.size).toBe(1);
		expect(m.get('crab_DDDDDDDD')?.display_name).toBe('Dan v2');
		expect(m.get('crab_DDDDDDDD')?.capability).toBe('Admin');
	});

	it('ignores message with missing member field', () => {
		handleMembersMessage({ type: 'MemberJoined' });
		expect(get(members).size).toBe(0);
	});
});

// =============================================================================
// MemberUpdated
// =============================================================================

describe('MemberUpdated', () => {
	beforeEach(() => {
		addMember('crab_EEEEEEEE', 'Eve', 'Collaborate');
	});

	it('updates capability', () => {
		handleMembersMessage({
			type: 'MemberUpdated',
			member: { fingerprint: 'crab_EEEEEEEE', capability: 'Admin', public_key: 'pk' },
		});

		const eve = get(members).get('crab_EEEEEEEE');
		expect(eve?.capability).toBe('Admin');
		expect(eve?.display_name).toBe('Eve'); // unchanged
	});

	it('updates display_name', () => {
		handleMembersMessage({
			type: 'MemberUpdated',
			member: { fingerprint: 'crab_EEEEEEEE', display_name: 'Eve Updated', public_key: 'pk' },
		});

		const eve = get(members).get('crab_EEEEEEEE');
		expect(eve?.display_name).toBe('Eve Updated');
		expect(eve?.capability).toBe('Collaborate'); // unchanged
	});

	it('ignores update for unknown fingerprint', () => {
		handleMembersMessage({
			type: 'MemberUpdated',
			member: { fingerprint: 'crab_UNKNOWN0', capability: 'Owner', public_key: 'pk' },
		});

		// Should not create a new entry
		expect(get(members).has('crab_UNKNOWN0')).toBe(false);
		expect(get(members).size).toBe(1);
	});
});

// =============================================================================
// MemberSuspended / MemberReinstated
// =============================================================================

describe('MemberSuspended', () => {
	beforeEach(() => {
		addMember('crab_FFFFFFFF', 'Frank', 'Collaborate');
	});

	it('sets state to suspended', () => {
		handleMembersMessage({ type: 'MemberSuspended', fingerprint: 'crab_FFFFFFFF', display_name: 'Frank', public_key: 'pk' });

		const frank = get(members).get('crab_FFFFFFFF');
		expect(frank?.state).toBe('suspended');
		expect(frank?.display_name).toBe('Frank'); // unchanged
	});

	it('ignores unknown fingerprint', () => {
		handleMembersMessage({ type: 'MemberSuspended', fingerprint: 'crab_NOPE0000', display_name: 'X', public_key: 'pk' });
		expect(get(members).size).toBe(1);
	});
});

describe('MemberReinstated', () => {
	beforeEach(() => {
		addMember('crab_GGGGGGGG', 'Grace', 'Collaborate');
		handleMembersMessage({ type: 'MemberSuspended', fingerprint: 'crab_GGGGGGGG', display_name: 'Grace', public_key: 'pk' });
	});

	it('restores state to active', () => {
		expect(get(members).get('crab_GGGGGGGG')?.state).toBe('suspended');

		handleMembersMessage({ type: 'MemberReinstated', fingerprint: 'crab_GGGGGGGG', display_name: 'Grace', public_key: 'pk' });

		expect(get(members).get('crab_GGGGGGGG')?.state).toBe('active');
	});
});

// =============================================================================
// MemberRemoved
// =============================================================================

describe('MemberRemoved', () => {
	it('removes a member from the map', () => {
		addMember('crab_HHHHHHHH', 'Hank', 'View');
		expect(get(members).size).toBe(1);

		handleMembersMessage({ type: 'MemberRemoved', fingerprint: 'crab_HHHHHHHH', display_name: 'Hank', public_key: 'pk' });
		expect(get(members).size).toBe(0);
	});

	it('ignores unknown fingerprint gracefully', () => {
		handleMembersMessage({ type: 'MemberRemoved', fingerprint: 'crab_NOPE0000', display_name: 'X', public_key: 'pk' });
		expect(get(members).size).toBe(0);
	});
});

// =============================================================================
// InviteRedeemed (adds member)
// =============================================================================

describe('InviteRedeemed', () => {
	it('adds a new member', () => {
		handleMembersMessage({
			type: 'InviteRedeemed',
			fingerprint: 'crab_IIIIIIII',
			display_name: 'Ivy',
			capability: 'Collaborate',
			public_key: 'pk_ivy',
		});

		const m = get(members);
		expect(m.size).toBe(1);
		const ivy = m.get('crab_IIIIIIII');
		expect(ivy?.display_name).toBe('Ivy');
		expect(ivy?.state).toBe('active');
	});
});

// =============================================================================
// Unknown message type (no-op)
// =============================================================================

describe('unknown message type', () => {
	it('does not crash', () => {
		expect(() => {
			handleMembersMessage({ type: 'SomeFutureMessage' });
		}).not.toThrow();
	});
});
