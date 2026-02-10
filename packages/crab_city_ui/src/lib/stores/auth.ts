/**
 * Authentication Store
 *
 * Manages current user state, CSRF token, and auth status.
 */

import { writable, derived } from 'svelte/store';
import { api } from '$lib/utils/api';

// =============================================================================
// Types
// =============================================================================

export interface AuthUser {
	id: string;
	username: string;
	display_name: string;
	is_admin: boolean;
}

interface MeResponse {
	user?: AuthUser;
	csrf_token?: string;
	needs_setup: boolean;
	auth_enabled: boolean;
}

interface AuthResponse {
	user: AuthUser;
	csrf_token: string;
}

// =============================================================================
// Stores
// =============================================================================

export const currentUser = writable<AuthUser | null>(null);
export const csrfToken = writable<string | null>(null);
export const authEnabled = writable<boolean>(false);
export const needsSetup = writable<boolean>(false);
export const authChecked = writable<boolean>(false);

export const isAuthenticated = derived(currentUser, ($user) => $user !== null);

// =============================================================================
// Functions
// =============================================================================

/**
 * Check current auth status by calling GET /api/auth/me.
 * Returns the auth state for routing decisions.
 */
export async function checkAuth(): Promise<{
	authenticated: boolean;
	needsSetup: boolean;
	authEnabled: boolean;
}> {
	try {
		const response = await fetch('/api/auth/me', {
			credentials: 'same-origin'
		});
		if (!response.ok) throw new Error('Auth check failed');

		const data: MeResponse = await response.json();

		authEnabled.set(data.auth_enabled);
		needsSetup.set(data.needs_setup);

		if (data.user) {
			currentUser.set(data.user);
			csrfToken.set(data.csrf_token ?? null);
		} else {
			currentUser.set(null);
			csrfToken.set(null);
		}

		authChecked.set(true);

		return {
			authenticated: !!data.user,
			needsSetup: data.needs_setup,
			authEnabled: data.auth_enabled
		};
	} catch (error) {
		console.error('Auth check failed:', error);
		authChecked.set(true);
		return { authenticated: false, needsSetup: false, authEnabled: false };
	}
}

/**
 * Log in with username and password.
 */
export async function login(
	username: string,
	password: string
): Promise<{ ok: boolean; error?: string }> {
	try {
		const response = await fetch('/api/auth/login', {
			method: 'POST',
			headers: { 'Content-Type': 'application/json' },
			credentials: 'same-origin',
			body: JSON.stringify({ username, password })
		});

		if (!response.ok) {
			const data = await response.json().catch(() => ({ error: 'Login failed' }));
			return { ok: false, error: data.error || 'Login failed' };
		}

		const data: AuthResponse = await response.json();
		currentUser.set(data.user);
		csrfToken.set(data.csrf_token);
		return { ok: true };
	} catch (error) {
		return { ok: false, error: 'Network error' };
	}
}

/**
 * Register a new user.
 */
export async function register(
	username: string,
	password: string,
	displayName?: string,
	inviteToken?: string
): Promise<{ ok: boolean; error?: string }> {
	try {
		const body: Record<string, string | undefined> = {
			username,
			password,
			display_name: displayName,
			invite_token: inviteToken
		};
		const response = await fetch('/api/auth/register', {
			method: 'POST',
			headers: { 'Content-Type': 'application/json' },
			credentials: 'same-origin',
			body: JSON.stringify(body)
		});

		if (!response.ok) {
			const data = await response.json().catch(() => ({ error: 'Registration failed' }));
			return { ok: false, error: data.error || 'Registration failed' };
		}

		const data: AuthResponse = await response.json();
		currentUser.set(data.user);
		csrfToken.set(data.csrf_token);
		return { ok: true };
	} catch (error) {
		return { ok: false, error: 'Network error' };
	}
}

/**
 * Change the current user's password.
 */
export async function changePassword(
	currentPassword: string,
	newPassword: string
): Promise<{ ok: boolean; error?: string }> {
	try {
		const response = await api('/api/auth/change-password', {
			method: 'POST',
			body: JSON.stringify({ current_password: currentPassword, new_password: newPassword })
		});

		if (!response.ok) {
			const data = await response.json().catch(() => ({ error: 'Failed to change password' }));
			return { ok: false, error: data.error || 'Failed to change password' };
		}

		return { ok: true };
	} catch (error) {
		return { ok: false, error: 'Network error' };
	}
}

/**
 * Log out the current user.
 */
export async function logout(): Promise<void> {
	try {
		await api('/api/auth/logout', { method: 'POST' });
	} catch {
		// Ignore errors - we'll clear state anyway
	}
	currentUser.set(null);
	csrfToken.set(null);
}
