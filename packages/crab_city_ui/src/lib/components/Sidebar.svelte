<script lang="ts">
	import { base } from '$app/paths';
	import { onMount } from 'svelte';
	import {
		instanceList,
		currentInstanceId,
		createInstance,
		deleteInstance,
		selectInstance
	} from '$lib/stores/instances';
	import { defaultCommand } from '$lib/stores/settings';
	import { closeSidebar, isDesktop } from '$lib/stores/ui';
	import { activityLevel, getInstanceVerb, clearInstanceVerbs } from '$lib/stores/activity';
	import { instancePresence } from '$lib/stores/websocket';
	import { tasks, pendingTasks, getTaskCount } from '$lib/stores/tasks';
	import { currentIdentity, isAuthenticated, clearIdentity } from '$lib/stores/auth';
	import { theme, toggleTheme } from '$lib/stores/settings';
	import type { ClaudeState, Instance } from '$lib/types';
	import InstanceItem from './sidebar/InstanceItem.svelte';

	async function handleLogout() {
		await clearIdentity();
		window.location.href = `${base}/join`;
	}

	let isCreating = false;

	// Track previous states for notifications
	let previousStates = new Map<string, string>();

	// Request notification permission on mount
	onMount(() => {
		if ('Notification' in window && Notification.permission === 'default') {
			Notification.requestPermission();
		}
	});

	// Send browser notification when instance becomes ready
	function notifyReady(instance: Instance) {
		if ('Notification' in window && Notification.permission === 'granted') {
			if (instance.id === $currentInstanceId) return;
			new Notification(`${instance.custom_name ?? instance.name} is ready`, {
				body: 'Claude is waiting for input',
				icon: '/favicon.png',
				tag: `ready-${instance.id}`,
				silent: false
			});
		}
	}

	// Check for state transitions and notify
	$: {
		for (const instance of $instanceList) {
			const prevState = previousStates.get(instance.id);
			const currentState = instance.claude_state?.type;

			if (prevState && currentState === 'WaitingForInput' && prevState !== 'WaitingForInput') {
				notifyReady(instance);
				clearInstanceVerbs(instance.id);
			}

			if (currentState) {
				previousStates.set(instance.id, currentState);
			}
		}
	}

	async function handleCreate() {
		if (isCreating) return;
		isCreating = true;

		const result = await createInstance({ command: $defaultCommand });
		if (result) {
			selectInstance(result.id);
			if (!$isDesktop) closeSidebar();
		}

		isCreating = false;
	}

	async function handleDelete(id: string, event: MouseEvent) {
		event.stopPropagation();
		await deleteInstance(id);
	}

	function handleSelectInstance(id: string) {
		selectInstance(id);
		if (!$isDesktop) closeSidebar();
	}

	function getStateInfo(
		instanceId: string,
		state: ClaudeState | undefined,
		stale: boolean = false
	): { label: string; color: string; animate: boolean; stale: boolean } {
		if (!state) {
			return { label: '', color: 'var(--text-muted)', animate: false, stale: false };
		}

		switch (state.type) {
			case 'Idle':
				return { label: '', color: 'var(--status-green)', animate: false, stale: false };
			case 'Thinking': {
				const verb = getInstanceVerb(instanceId, 'Thinking').toLowerCase();
				return { label: stale ? `${verb}?` : verb, color: 'var(--purple-500)', animate: !stale, stale };
			}
			case 'Responding': {
				const verb = getInstanceVerb(instanceId, 'Responding').toLowerCase();
				return { label: stale ? `${verb}?` : verb, color: 'var(--amber-500)', animate: !stale, stale };
			}
			case 'ToolExecuting':
				return { label: stale ? `${state.tool}?` : state.tool, color: 'var(--amber-400)', animate: !stale, stale };
			case 'WaitingForInput':
				return { label: 'ready', color: 'var(--status-green)', animate: false, stale: false };
			default:
				return { label: '', color: 'var(--text-muted)', animate: false, stale: false };
		}
	}

	function getQueueCount(instanceId: string, _tasks: typeof $tasks): number {
		return getTaskCount(instanceId);
	}
</script>

<aside class="sidebar">
	<!-- Header -->
	<header class="sidebar-header">
		<div class="header-content">
			<h1>Crab City</h1>
			<p class="tagline">Claude Manager</p>
		</div>
		{#if !$isDesktop}
			<button class="close-btn" onclick={closeSidebar} aria-label="Close sidebar">
				<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
					<path d="M6 18L18 6M6 6l12 12" />
				</svg>
			</button>
		{/if}
	</header>

	<!-- New Instance Button -->
	<button class="new-instance-btn" onclick={handleCreate} disabled={isCreating} aria-label="Create new Claude instance">
		{#if isCreating}
			<span class="spinner"></span>
			Creating...
		{:else}
			<span class="icon">+</span>
			New Instance
		{/if}
	</button>

	<!-- Instances List -->
	<nav class="instances-list">
		{#each $instanceList as instance (instance.id)}
			{@const stateInfo = getStateInfo(instance.id, instance.claude_state, instance.claude_state_stale)}
			<InstanceItem
				{instance}
				isActive={$currentInstanceId === instance.id}
				{stateInfo}
				activityLevel={$activityLevel}
				presenceCount={$instancePresence.get(instance.id)?.length ?? 0}
				presenceNames={$instancePresence.get(instance.id)?.map(u => u.display_name).join(', ') ?? ''}
				queueCount={getQueueCount(instance.id, $tasks)}
				onselect={() => handleSelectInstance(instance.id)}
				ondelete={(e) => handleDelete(instance.id, e)}
			/>
		{:else}
			<div class="empty-list">
				<p>No instances yet</p>
				<p class="hint">Click "New Instance" to start</p>
			</div>
		{/each}
	</nav>

	<!-- Footer -->
	<footer class="sidebar-footer">
		{#if $isAuthenticated && $currentIdentity}
			<div class="user-info">
				<span class="user-display-name">{$currentIdentity.displayName}</span>
				<button class="logout-btn" onclick={handleLogout}>Logout</button>
			</div>
		{/if}
		<div class="footer-row">
			<div class="footer-links">
				<a href="{base}/tasks" class="footer-link">
					Tasks
					{#if $pendingTasks.length > 0}
						<span class="footer-badge">{$pendingTasks.length}</span>
					{/if}
				</a>
				<a href="{base}/history" class="footer-link">History</a>
				<a href="{base}/settings" class="footer-link">Settings</a>
			</div>
			<button
				class="theme-toggle"
				class:analog={$theme === 'analog'}
				onclick={toggleTheme}
				title={$theme === 'phosphor' ? 'Switch to analog (⇧⌘L)' : 'Switch to phosphor (⇧⌘L)'}
				aria-label="Toggle theme"
			>
				<span class="theme-toggle-track">
					<span class="theme-toggle-thumb">
						{#if $theme === 'phosphor'}
							<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5">
								<rect x="2" y="2" width="12" height="8" rx="1" />
								<path d="M5 13h6M8 10v3" />
								<line class="scanline" x1="3" y1="5" x2="13" y2="5" stroke-opacity="0.4" />
								<line class="scanline s2" x1="3" y1="7" x2="13" y2="7" stroke-opacity="0.25" />
							</svg>
						{:else}
							<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5">
								<path d="M8 14 L6 8 Q8 3 8 1 Q8 3 10 8 Z" />
								<line x1="8" y1="7" x2="8" y2="11" stroke-opacity="0.3" />
								<circle cx="8" cy="13" r="0.5" fill="currentColor" stroke="none" />
							</svg>
						{/if}
					</span>
				</span>
			</button>
		</div>
	</footer>
</aside>

<style>
	.sidebar {
		display: flex;
		flex-direction: column;
		width: 260px;
		background: linear-gradient(180deg, var(--surface-700) 0%, var(--surface-800) 100%);
		border-right: 1px solid var(--surface-border);
		height: 100%;
		flex-shrink: 0;
		position: relative;
	}

	.sidebar::after {
		content: '';
		position: absolute;
		top: 0;
		right: 0;
		bottom: 0;
		width: 1px;
		background: linear-gradient(180deg, transparent 0%, var(--tint-active) 50%, transparent 100%);
	}

	.sidebar-header {
		display: flex;
		align-items: center;
		justify-content: space-between;
		padding: 20px 16px;
		border-bottom: 1px solid var(--surface-border);
		background: var(--panel-inset);
	}

	.header-content { flex: 1; }

	.sidebar-header h1 {
		margin: 0;
		font-size: 16px;
		font-weight: 700;
		letter-spacing: 0.1em;
		color: var(--amber-500);
		text-shadow: var(--emphasis-strong);
		text-transform: uppercase;
		font-family: var(--font-display);
	}

	.tagline {
		margin: 6px 0 0;
		font-size: 10px;
		letter-spacing: 0.15em;
		color: var(--text-muted);
		text-transform: uppercase;
	}

	.close-btn {
		display: flex;
		align-items: center;
		justify-content: center;
		width: 36px;
		height: 36px;
		background: transparent;
		border: 1px solid var(--surface-border);
		border-radius: 4px;
		color: var(--text-secondary);
		cursor: pointer;
		transition: all 0.15s ease;
		flex-shrink: 0;
	}

	.close-btn:hover {
		border-color: var(--amber-600);
		color: var(--amber-400);
		background: var(--tint-active);
	}

	.close-btn svg { width: 18px; height: 18px; }

	.new-instance-btn {
		display: flex;
		align-items: center;
		justify-content: center;
		gap: 8px;
		margin: 12px;
		padding: 12px 16px;
		background: linear-gradient(180deg, var(--surface-500) 0%, var(--surface-600) 100%);
		border: var(--active-border);
		border-radius: 4px;
		color: var(--amber-400);
		font-size: 12px;
		font-weight: 600;
		font-family: inherit;
		letter-spacing: 0.05em;
		text-transform: uppercase;
		cursor: pointer;
		transition: all 0.15s ease;
		box-shadow: var(--depth-up);
	}

	.new-instance-btn:hover:not(:disabled) {
		background: linear-gradient(180deg, var(--surface-400) 0%, var(--surface-500) 100%);
		border-color: var(--amber-500);
		color: var(--amber-300);
		box-shadow: var(--elevation-high);
		text-shadow: var(--emphasis);
	}

	.new-instance-btn:disabled { opacity: 0.5; cursor: not-allowed; }

	.icon { font-size: 16px; font-weight: 400; }

	.spinner {
		width: 12px;
		height: 12px;
		border: 2px solid var(--spinner-track);
		border-top-color: var(--amber-500);
		border-radius: 50%;
		animation: spin 0.8s linear infinite;
	}

	@keyframes spin { to { transform: rotate(360deg); } }

	.instances-list {
		flex: 1;
		overflow-y: auto;
		padding: 8px;
	}

	.empty-list {
		padding: 32px 16px;
		text-align: center;
		color: var(--text-muted);
	}

	.empty-list p { margin: 0; font-size: 11px; letter-spacing: 0.05em; }

	.hint {
		margin-top: 8px !important;
		font-size: 10px !important;
		color: var(--text-muted) !important;
		opacity: 0.7;
	}

	.sidebar-footer {
		display: flex;
		flex-direction: column;
		gap: 8px;
		padding: 12px 16px;
		border-top: 1px solid var(--surface-border);
		background: var(--panel-inset);
	}

	.user-info {
		display: flex;
		align-items: center;
		justify-content: space-between;
		padding-bottom: 8px;
		border-bottom: 1px solid var(--surface-border);
	}

	.user-display-name {
		font-size: 11px;
		font-weight: 600;
		color: var(--text-primary);
		letter-spacing: 0.03em;
	}

	.logout-btn {
		background: none;
		border: 1px solid var(--surface-border);
		border-radius: 3px;
		padding: 2px 8px;
		font-size: 10px;
		font-family: inherit;
		color: var(--text-muted);
		cursor: pointer;
		text-transform: uppercase;
		letter-spacing: 0.05em;
		transition: all 0.15s ease;
	}

	.logout-btn:hover { border-color: var(--status-red); color: var(--status-red); }

	.footer-links { display: flex; gap: 16px; }

	.footer-link {
		font-size: 11px;
		letter-spacing: 0.05em;
		color: var(--text-muted);
		text-decoration: none;
		text-transform: uppercase;
		transition: all 0.15s ease;
	}

	.footer-link:hover { color: var(--amber-400); text-shadow: var(--emphasis); }

	.footer-badge {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		min-width: 14px;
		height: 14px;
		padding: 0 3px;
		margin-left: 3px;
		background: var(--tint-active-strong);
		border: 1px solid var(--amber-600);
		border-radius: 7px;
		font-size: 9px;
		font-weight: 700;
		color: var(--amber-400);
		font-variant-numeric: tabular-nums;
		vertical-align: middle;
	}

	/* Scrollbar */
	.instances-list::-webkit-scrollbar { width: 6px; }
	.instances-list::-webkit-scrollbar-track { background: transparent; }
	.instances-list::-webkit-scrollbar-thumb { background: var(--surface-border); border-radius: 3px; }
	.instances-list::-webkit-scrollbar-thumb:hover { background: var(--amber-600); }

	/* Theme toggle */
	.footer-row {
		display: flex;
		align-items: center;
		justify-content: space-between;
	}

	.theme-toggle {
		position: relative;
		width: 40px;
		height: 22px;
		padding: 0;
		background: none;
		border: none;
		cursor: pointer;
		flex-shrink: 0;
	}

	.theme-toggle-track {
		display: block;
		width: 100%;
		height: 100%;
		border-radius: 11px;
		background: var(--surface-500);
		border: 1px solid var(--surface-border);
		position: relative;
		transition: all 0.4s cubic-bezier(0.4, 0, 0.2, 1);
		overflow: hidden;
	}

	.theme-toggle:not(.analog) .theme-toggle-track {
		box-shadow: var(--recess), var(--elevation-low);
	}

	.theme-toggle.analog .theme-toggle-track {
		background: var(--surface-400);
		box-shadow: var(--recess), var(--elevation-low);
	}

	.theme-toggle-thumb {
		position: absolute;
		top: 1px;
		left: 1px;
		width: 18px;
		height: 18px;
		border-radius: 50%;
		background: var(--surface-700);
		border: 1px solid var(--surface-border-light);
		display: flex;
		align-items: center;
		justify-content: center;
		transition: all 0.4s cubic-bezier(0.4, 0, 0.2, 1);
		color: var(--amber-500);
	}

	.theme-toggle:not(.analog) .theme-toggle-thumb {
		transform: translateX(0);
		box-shadow: var(--elevation-low);
	}

	.theme-toggle.analog .theme-toggle-thumb {
		transform: translateX(18px);
		background: var(--surface-700);
		color: var(--amber-600);
		box-shadow: var(--elevation-low);
	}

	.theme-toggle-thumb svg { width: 10px; height: 10px; }

	.theme-toggle-thumb .scanline { animation: toggle-scan 1.2s linear infinite; }
	.theme-toggle-thumb .scanline.s2 { animation-delay: 0.6s; }

	@keyframes toggle-scan {
		0%, 100% { stroke-opacity: 0.15; }
		50% { stroke-opacity: 0.5; }
	}

	.theme-toggle:hover .theme-toggle-track { border-color: var(--surface-border-light); }
	.theme-toggle:not(.analog):hover .theme-toggle-thumb { box-shadow: var(--elevation-high); }
	.theme-toggle.analog:hover .theme-toggle-thumb { box-shadow: var(--elevation-high); }

	.theme-toggle:active .theme-toggle-thumb { transform: translateX(0) scale(0.9); }
	.theme-toggle.analog:active .theme-toggle-thumb { transform: translateX(18px) scale(0.9); }

	/* Analog theme */
	:global([data-theme="analog"]) .sidebar {
		background-color: var(--surface-800);
		background-image: var(--grain-fine), var(--grain-coarse), radial-gradient(ellipse at 50% 80%, rgba(42,31,24,0.04) 0%, transparent 60%);
		background-blend-mode: multiply, multiply, normal;
	}

	:global([data-theme="analog"]) .new-instance-btn {
		background-color: var(--surface-600);
		background-image: var(--grain-fine);
		background-blend-mode: multiply;
		border-width: 2px;
		box-shadow: var(--elevation-low), inset 0 1px 2px rgba(42, 31, 24, 0.06);
	}

	:global([data-theme="analog"]) .new-instance-btn:hover:not(:disabled) {
		background-color: var(--surface-500);
		background-image: var(--grain-fine);
		background-blend-mode: multiply;
		border-width: 2px;
		box-shadow: var(--elevation-high), inset 0 1px 3px rgba(42, 31, 24, 0.1);
	}

	:global([data-theme="analog"]) .sidebar::after {
		background: var(--amber-600);
		width: 1.5px;
		box-shadow: 1px 0 3px rgba(42, 31, 24, 0.08);
	}

	:global([data-theme="analog"]) .sidebar-footer {
		background-color: var(--surface-800);
		background-image: var(--grain-fine);
		background-blend-mode: multiply;
		border-top-width: 2px;
	}

	:global([data-theme="analog"]) .sidebar-header {
		background-color: var(--surface-800);
		background-image: var(--grain-coarse);
		background-blend-mode: multiply;
	}
</style>
