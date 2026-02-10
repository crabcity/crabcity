import { writable } from 'svelte/store';
import { browser } from '$app/environment';

// Persisted settings
export function createPersistedStore<T>(key: string, defaultValue: T) {
	const initialValue = browser ? (localStorage.getItem(key) ?? null) : null;

	const parsed = initialValue ? (JSON.parse(initialValue) as T) : defaultValue;
	const store = writable<T>(parsed);

	if (browser) {
		store.subscribe((value) => {
			localStorage.setItem(key, JSON.stringify(value));
		});
	}

	return store;
}

// Settings stores
export const defaultCommand = createPersistedStore('crab_city_default_command', 'claude');
export const theme = createPersistedStore<'phosphor' | 'analog'>('crab_city_theme', 'phosphor');

export function toggleTheme(): void {
	theme.update((t) => (t === 'phosphor' ? 'analog' : 'phosphor'));
}
export const drawerWidth = createPersistedStore('crab_city_drawer_width', 400);
export const diffEngine = createPersistedStore<'standard' | 'patience' | 'structural'>('crab_city_diff_engine', 'structural');
export const drawerOpen = writable(false);

// Actions
export function toggleDrawer(): void {
	drawerOpen.update((open) => !open);
}

export function setDrawerOpen(open: boolean): void {
	drawerOpen.set(open);
}

export function setDrawerWidth(width: number): void {
	// Clamp between 200 and 800
	const clamped = Math.max(200, Math.min(800, width));
	drawerWidth.set(clamped);
}
