/**
 * Authentication Store — Keypair Identity
 *
 * Identity is derived from a locally-stored ed25519 keypair.
 * The WS connection handles challenge-response auth;
 * this store tracks the resulting identity state.
 */

import { writable, derived, get } from 'svelte/store';
import {
	loadKeypair,
	saveKeypair,
	generateKeypair,
	deleteKeypair,
	importKey,
	exportKey,
	downloadKey,
	computeFingerprint,
	type KeyIdentity,
} from '$lib/crypto/keys';

// =============================================================================
// Types
// =============================================================================

export interface AuthIdentity {
	fingerprint: string;
	displayName: string;
	capability: string;
	publicKey: Uint8Array;
}

// =============================================================================
// Stores
// =============================================================================

/** Current authenticated identity (set after WS handshake succeeds). */
export const currentIdentity = writable<AuthIdentity | null>(null);

/** Auth error message (set when auth handshake fails). */
export const authError = writable<string | null>(null);

/** Local keypair (loaded from IndexedDB on init). */
export const localKeypair = writable<KeyIdentity | null>(null);

/** Whether the initial keypair load from IndexedDB is complete. */
export const authReady = writable<boolean>(false);

/** Derived: is the user authenticated? */
export const isAuthenticated = derived(currentIdentity, ($id) => $id !== null);

// Legacy compat — some components reference these
export const authEnabled = writable<boolean>(true);
export const needsSetup = writable<boolean>(false);
export const authChecked = writable<boolean>(false);

// =============================================================================
// Initialization
// =============================================================================

/**
 * Load the local keypair from IndexedDB.
 * Call this once on app mount.
 */
export async function initAuth(): Promise<KeyIdentity | null> {
	const kp = await loadKeypair();
	localKeypair.set(kp);
	authReady.set(true);
	authChecked.set(true);
	return kp;
}

// =============================================================================
// Identity lifecycle
// =============================================================================

/** Generate a new keypair and save it. */
export async function createIdentity(): Promise<KeyIdentity> {
	const kp = generateKeypair();
	await saveKeypair(kp);
	localKeypair.set(kp);
	return kp;
}

/** Import a keypair from a base64 string. */
export async function importIdentity(base64: string): Promise<KeyIdentity> {
	const kp = importKey(base64);
	await saveKeypair(kp);
	localKeypair.set(kp);
	return kp;
}

/** Export the current keypair as base64. */
export function exportIdentity(kp: KeyIdentity): string {
	return exportKey(kp);
}

/** Download the key as a .key file. */
export function downloadIdentity(kp: KeyIdentity): void {
	downloadKey(kp);
}

/** Delete the local keypair and clear identity. */
export async function clearIdentity(): Promise<void> {
	await deleteKeypair();
	localKeypair.set(null);
	currentIdentity.set(null);
}

/**
 * Set the authenticated identity after WS handshake succeeds.
 * Called from the WS auth flow.
 *
 * Validates that the server's fingerprint matches the local keypair
 * to prevent identity confusion.
 */
export function setAuthenticated(
	fingerprint: string,
	capability: string,
	displayName: string,
	publicKey: Uint8Array,
): void {
	// Validate fingerprint matches local keypair if we have one
	const kp = get(localKeypair);
	if (kp) {
		const localFp = computeFingerprint(kp.publicKey);
		if (localFp !== fingerprint) {
			const msg = `Fingerprint mismatch: server sent ${fingerprint}, local key is ${localFp}`;
			console.error('[Auth]', msg);
			authError.set(msg);
			return;
		}
	}

	authError.set(null);
	currentIdentity.set({ fingerprint, capability, displayName, publicKey });
}

/**
 * Clear authentication state (e.g., on WS disconnect or explicit logout).
 */
export function clearAuthentication(): void {
	currentIdentity.set(null);
	authError.set(null);
}

// =============================================================================
// Legacy compat stubs
// =============================================================================

/** Legacy checkAuth — returns auth state for routing. */
export async function checkAuth(): Promise<{
	authenticated: boolean;
	needsSetup: boolean;
	authEnabled: boolean;
}> {
	const kp = await initAuth();
	return {
		authenticated: kp !== null,
		needsSetup: false,
		authEnabled: true,
	};
}
