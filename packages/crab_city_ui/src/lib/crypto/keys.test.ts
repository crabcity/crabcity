/**
 * Tests for ed25519 keypair management.
 *
 * Covers: key generation, signing/verification, hex encoding/decoding,
 * fingerprint computation, key export/import, and Crockford base32.
 */

import {
	generateKeypair,
	sign,
	verify,
	hexEncode,
	hexDecode,
	computeFingerprint,
	exportKey,
	importKey,
} from './keys.js';

// =============================================================================
// Key Generation
// =============================================================================

describe('generateKeypair', () => {
	it('produces 32-byte public key', () => {
		const kp = generateKeypair();
		expect(kp.publicKey).toBeInstanceOf(Uint8Array);
		expect(kp.publicKey.length).toBe(32);
	});

	it('produces 32-byte private key seed', () => {
		const kp = generateKeypair();
		expect(kp.privateKey).toBeInstanceOf(Uint8Array);
		expect(kp.privateKey.length).toBe(32);
	});

	it('produces a crab_ fingerprint', () => {
		const kp = generateKeypair();
		expect(kp.fingerprint).toMatch(/^crab_[0-9A-Z]{8}$/);
	});

	it('generates unique keypairs', () => {
		const kp1 = generateKeypair();
		const kp2 = generateKeypair();
		expect(hexEncode(kp1.publicKey)).not.toBe(hexEncode(kp2.publicKey));
	});
});

// =============================================================================
// Sign / Verify
// =============================================================================

describe('sign and verify', () => {
	it('valid signature verifies', () => {
		const kp = generateKeypair();
		const msg = new TextEncoder().encode('hello world');
		const sig = sign(msg, kp.privateKey);
		expect(sig).toBeInstanceOf(Uint8Array);
		expect(sig.length).toBe(64);
		expect(verify(msg, sig, kp.publicKey)).toBe(true);
	});

	it('wrong message fails verification', () => {
		const kp = generateKeypair();
		const msg = new TextEncoder().encode('hello');
		const sig = sign(msg, kp.privateKey);
		const wrong = new TextEncoder().encode('wrong');
		expect(verify(wrong, sig, kp.publicKey)).toBe(false);
	});

	it('wrong key fails verification', () => {
		const kp1 = generateKeypair();
		const kp2 = generateKeypair();
		const msg = new TextEncoder().encode('test');
		const sig = sign(msg, kp1.privateKey);
		expect(verify(msg, sig, kp2.publicKey)).toBe(false);
	});

	it('tampered signature fails verification', () => {
		const kp = generateKeypair();
		const msg = new TextEncoder().encode('test');
		const sig = sign(msg, kp.privateKey);
		sig[0] = (sig[0] ?? 0) ^ 0xff;
		expect(verify(msg, sig, kp.publicKey)).toBe(false);
	});

	it('empty message can be signed and verified', () => {
		const kp = generateKeypair();
		const msg = new Uint8Array(0);
		const sig = sign(msg, kp.privateKey);
		expect(verify(msg, sig, kp.publicKey)).toBe(true);
	});

	it('32-byte nonce roundtrip (mimics challenge-response)', () => {
		const kp = generateKeypair();
		const nonce = new Uint8Array(32);
		crypto.getRandomValues(nonce);
		const sig = sign(nonce, kp.privateKey);

		// Simulate server-side verify: hex roundtrip
		const pkHex = hexEncode(kp.publicKey);
		const sigHex = hexEncode(sig);
		const nonceHex = hexEncode(nonce);

		const pkBytes = hexDecode(pkHex);
		const sigBytes = hexDecode(sigHex);
		const nonceBytes = hexDecode(nonceHex);

		expect(verify(nonceBytes, sigBytes, pkBytes)).toBe(true);
	});
});

// =============================================================================
// Hex Encoding
// =============================================================================

describe('hexEncode / hexDecode', () => {
	it('empty bytes', () => {
		expect(hexEncode(new Uint8Array(0))).toBe('');
		expect(hexDecode('')).toEqual(new Uint8Array(0));
	});

	it('single byte', () => {
		expect(hexEncode(new Uint8Array([0xff]))).toBe('ff');
		expect(hexDecode('ff')).toEqual(new Uint8Array([0xff]));
	});

	it('roundtrip', () => {
		const bytes = new Uint8Array([0, 1, 127, 128, 255]);
		expect(hexDecode(hexEncode(bytes))).toEqual(bytes);
	});

	it('lowercase output', () => {
		const hex = hexEncode(new Uint8Array([0xab, 0xcd]));
		expect(hex).toBe('abcd');
	});

	it('32-byte key roundtrip', () => {
		const kp = generateKeypair();
		const hex = hexEncode(kp.publicKey);
		expect(hex.length).toBe(64);
		expect(hexDecode(hex)).toEqual(kp.publicKey);
	});

	it('64-byte signature roundtrip', () => {
		const kp = generateKeypair();
		const sig = sign(new Uint8Array([42]), kp.privateKey);
		const hex = hexEncode(sig);
		expect(hex.length).toBe(128);
		expect(hexDecode(hex)).toEqual(sig);
	});
});

// =============================================================================
// Fingerprint
// =============================================================================

describe('computeFingerprint', () => {
	it('format is crab_ + 8 uppercase alphanumeric chars', () => {
		const kp = generateKeypair();
		expect(kp.fingerprint).toMatch(/^crab_[0-9A-Z]{8}$/);
	});

	it('same key produces same fingerprint', () => {
		const kp = generateKeypair();
		expect(computeFingerprint(kp.publicKey)).toBe(kp.fingerprint);
		expect(computeFingerprint(kp.publicKey)).toBe(kp.fingerprint);
	});

	it('different keys produce different fingerprints', () => {
		const kp1 = generateKeypair();
		const kp2 = generateKeypair();
		expect(kp1.fingerprint).not.toBe(kp2.fingerprint);
	});

	it('loopback key (all zeros) has a deterministic fingerprint', () => {
		const loopback = new Uint8Array(32);
		const fp = computeFingerprint(loopback);
		expect(fp).toBe('crab_00000000');
	});

	it('length is always 13 (crab_ + 8)', () => {
		for (let i = 0; i < 10; i++) {
			const kp = generateKeypair();
			expect(kp.fingerprint.length).toBe(13);
		}
	});
});

// =============================================================================
// Key Export / Import
// =============================================================================

describe('exportKey / importKey', () => {
	it('roundtrip preserves keypair', () => {
		const kp = generateKeypair();
		const exported = exportKey(kp);
		const imported = importKey(exported);

		expect(imported.fingerprint).toBe(kp.fingerprint);
		expect(hexEncode(imported.publicKey)).toBe(hexEncode(kp.publicKey));
	});

	it('exported key is base64', () => {
		const kp = generateKeypair();
		const exported = exportKey(kp);
		// base64 chars only
		expect(exported).toMatch(/^[A-Za-z0-9+/=]+$/);
	});

	it('imported key can sign and verify', () => {
		const kp = generateKeypair();
		const exported = exportKey(kp);
		const imported = importKey(exported);

		const msg = new TextEncoder().encode('test import');
		const sig = sign(msg, imported.privateKey);
		expect(verify(msg, sig, imported.publicKey)).toBe(true);
	});

	it('rejects invalid length', () => {
		const badKey = btoa(String.fromCharCode(...new Uint8Array(16)));
		expect(() => importKey(badKey)).toThrow('Invalid key length');
	});
});
