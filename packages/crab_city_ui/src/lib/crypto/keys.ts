/**
 * Ed25519 keypair management for browser-side identity.
 *
 * Uses @noble/ed25519 for key generation and signing.
 * Keys are stored as plaintext in IndexedDB.
 *
 * // TODO: encrypt private key at rest with WebCrypto AES-256-GCM
 */

import * as ed from '@noble/ed25519';
import { sha512 } from '@noble/hashes/sha2.js';

// noble/ed25519 v2+ requires setting the sha512 hash function
ed.hashes.sha512 = sha512;

// =============================================================================
// Types
// =============================================================================

export interface KeyIdentity {
	publicKey: Uint8Array; // 32 bytes
	privateKey: Uint8Array; // 32-byte ed25519 seed
	fingerprint: string; // crab_XXXXXXXX
}

// =============================================================================
// Key Operations
// =============================================================================

/** Generate a new ed25519 keypair. */
export function generateKeypair(): KeyIdentity {
	const privateKey = ed.utils.randomSecretKey(); // 32-byte seed
	const publicKey = ed.getPublicKey(privateKey);
	return {
		publicKey,
		privateKey, // 32-byte seed
		fingerprint: computeFingerprint(publicKey),
	};
}

/** Sign a message with a private key. Returns 64-byte signature. */
export function sign(message: Uint8Array, privateKey: Uint8Array): Uint8Array {
	return ed.sign(message, privateKey);
}

/** Verify a signature. */
export function verify(
	message: Uint8Array,
	signature: Uint8Array,
	publicKey: Uint8Array
): boolean {
	return ed.verify(signature, message, publicKey);
}

/** Compute the crab_ fingerprint from a 32-byte public key. */
export function computeFingerprint(publicKey: Uint8Array): string {
	const encoded = crockfordEncode(publicKey);
	return `crab_${encoded.slice(0, 8)}`;
}

// =============================================================================
// Hex encoding (matches server's hex format)
// =============================================================================

export function hexEncode(bytes: Uint8Array): string {
	return Array.from(bytes)
		.map((b) => b.toString(16).padStart(2, '0'))
		.join('');
}

export function hexDecode(hex: string): Uint8Array {
	const bytes = new Uint8Array(hex.length / 2);
	for (let i = 0; i < hex.length; i += 2) {
		bytes[i / 2] = parseInt(hex.slice(i, i + 2), 16);
	}
	return bytes;
}

// =============================================================================
// IndexedDB Storage
// =============================================================================

const DB_NAME = 'crab_city_keys';
const STORE_NAME = 'identity';
const KEY_ID = 'primary';

function openDB(): Promise<IDBDatabase> {
	return new Promise((resolve, reject) => {
		const req = indexedDB.open(DB_NAME, 1);
		req.onupgradeneeded = () => {
			req.result.createObjectStore(STORE_NAME, { keyPath: 'id' });
		};
		req.onsuccess = () => resolve(req.result);
		req.onerror = () => reject(req.error);
	});
}

/** Save a keypair to IndexedDB. */
export async function saveKeypair(identity: KeyIdentity): Promise<void> {
	const db = await openDB();
	const tx = db.transaction(STORE_NAME, 'readwrite');
	const store = tx.objectStore(STORE_NAME);
	store.put({
		id: KEY_ID,
		publicKey: Array.from(identity.publicKey),
		privateKey: Array.from(identity.privateKey),
		fingerprint: identity.fingerprint,
	});
	return new Promise((resolve, reject) => {
		tx.oncomplete = () => resolve();
		tx.onerror = () => reject(tx.error);
	});
}

/** Load keypair from IndexedDB. Returns null if not found. */
export async function loadKeypair(): Promise<KeyIdentity | null> {
	try {
		const db = await openDB();
		const tx = db.transaction(STORE_NAME, 'readonly');
		const store = tx.objectStore(STORE_NAME);
		const req = store.get(KEY_ID);
		return new Promise((resolve, reject) => {
			req.onsuccess = () => {
				if (!req.result) {
					resolve(null);
					return;
				}
				resolve({
					publicKey: new Uint8Array(req.result.publicKey),
					privateKey: new Uint8Array(req.result.privateKey),
					fingerprint: req.result.fingerprint,
				});
			};
			req.onerror = () => reject(req.error);
		});
	} catch {
		return null;
	}
}

/** Delete the stored keypair. */
export async function deleteKeypair(): Promise<void> {
	const db = await openDB();
	const tx = db.transaction(STORE_NAME, 'readwrite');
	tx.objectStore(STORE_NAME).delete(KEY_ID);
	return new Promise((resolve, reject) => {
		tx.oncomplete = () => resolve();
		tx.onerror = () => reject(tx.error);
	});
}

// =============================================================================
// Key Export/Import (for backup)
// =============================================================================

/** Export private key as base64 string (for backup). */
export function exportKey(identity: KeyIdentity): string {
	return btoa(String.fromCharCode(...identity.privateKey));
}

/** Import private key from base64 string. */
export function importKey(base64: string): KeyIdentity {
	const raw = Uint8Array.from(atob(base64), (c) => c.charCodeAt(0));
	if (raw.length !== 32 && raw.length !== 64) {
		throw new Error('Invalid key length');
	}
	const seed = raw.length === 64 ? raw.slice(0, 32) : raw;
	const publicKey = ed.getPublicKey(seed);
	return {
		publicKey,
		privateKey: seed,
		fingerprint: computeFingerprint(publicKey),
	};
}

/** Download a key as a .key file. */
export function downloadKey(identity: KeyIdentity): void {
	const b64 = exportKey(identity);
	const blob = new Blob([b64], { type: 'text/plain' });
	const url = URL.createObjectURL(blob);
	const a = document.createElement('a');
	a.href = url;
	a.download = `${identity.fingerprint}.key`;
	a.click();
	URL.revokeObjectURL(url);
}

// =============================================================================
// Crockford Base32 (matches server implementation)
// =============================================================================

const CROCKFORD_ALPHABET = '0123456789ABCDEFGHJKMNPQRSTVWXYZ';

function crockfordEncode(data: Uint8Array): string {
	let bits = 0;
	let value = 0;
	let result = '';

	for (const byte of data) {
		value = (value << 8) | byte;
		bits += 8;

		while (bits >= 5) {
			bits -= 5;
			result += CROCKFORD_ALPHABET[(value >> bits) & 0x1f];
		}
	}

	if (bits > 0) {
		result += CROCKFORD_ALPHABET[(value << (5 - bits)) & 0x1f];
	}

	return result;
}
