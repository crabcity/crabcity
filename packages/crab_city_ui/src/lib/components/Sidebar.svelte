<script lang="ts">
	import { base } from '$app/paths';
	import { onMount } from 'svelte';
	import {
		instanceList,
		currentInstanceId,
		createInstance,
		selectInstance
	} from '$lib/stores/instances';
	import { defaultCommand } from '$lib/stores/settings';
	import { projects, currentProject } from '$lib/stores/projects';
	import { clearInstanceVerbs } from '$lib/stores/activity';
	import { currentUser, isAuthenticated, logout } from '$lib/stores/auth';
	import { theme, toggleTheme } from '$lib/stores/settings';
	import type { Instance } from '$lib/types';

	async function handleLogout() {
		await logout();
		window.location.href = `${base}/login`;
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

		const result = await createInstance({
			command: $defaultCommand,
			working_dir: $currentProject?.workingDir
		});
		if (result) {
			selectInstance(result.id);
		}

		isCreating = false;
	}

	function handleSelectProject(workingDir: string) {
		const project = $projects.find(p => p.workingDir === workingDir);
		if (project && project.instances.length > 0) {
			selectInstance(project.instances[0].id);
		}
	}

	/** Get 2-letter abbreviation for a project name */
	function getProjectAbbr(name: string): string {
		const words = name.replace(/[^a-zA-Z0-9\s]/g, '').split(/[\s_-]+/).filter(Boolean);
		if (words.length >= 2) {
			return (words[0][0] + words[1][0]).toUpperCase();
		}
		return name.slice(0, 2).toUpperCase();
	}

	/** Color by index for project icons */
	const projectColors = [
		'var(--amber-500)',
		'var(--purple-400)',
		'var(--status-green)',
		'var(--status-red)',
	];
</script>

<aside class="sidebar-rail">
	<!-- Project icons -->
	<nav class="rail-projects">
		{#each $projects as project, i (project.id)}
			{@const isActive = $currentProject?.id === project.id}
			<button
				class="rail-project"
				class:active={isActive}
				onclick={() => handleSelectProject(project.workingDir)}
				title="{project.name} ({project.instances.length} instances)"
				aria-label="{project.name} project"
				style="--project-color: {projectColors[i % projectColors.length]}"
			>
				<span class="project-abbr">{getProjectAbbr(project.name)}</span>
			</button>
		{/each}
	</nav>

	<!-- Bottom actions -->
	<div class="rail-bottom">
		<button
			class="rail-btn"
			onclick={handleCreate}
			disabled={isCreating}
			title="New instance"
			aria-label="Create new instance"
		>
			{#if isCreating}
				<span class="rail-spinner"></span>
			{:else}
				<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
					<line x1="12" y1="5" x2="12" y2="19" />
					<line x1="5" y1="12" x2="19" y2="12" />
				</svg>
			{/if}
		</button>

		<button
			class="rail-btn theme-btn"
			class:analog={$theme === 'analog'}
			onclick={toggleTheme}
			title={$theme === 'phosphor' ? 'Switch to analog' : 'Switch to phosphor'}
			aria-label="Toggle theme"
		>
			{#if $theme === 'phosphor'}
				<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5">
					<rect x="2" y="2" width="12" height="8" rx="1" />
					<path d="M5 13h6M8 10v3" />
				</svg>
			{:else}
				<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5">
					<path d="M8 14 L6 8 Q8 3 8 1 Q8 3 10 8 Z" />
					<circle cx="8" cy="13" r="0.5" fill="currentColor" stroke="none" />
				</svg>
			{/if}
		</button>

		{#if $isAuthenticated && $currentUser}
			<button
				class="rail-btn user-btn"
				title="{$currentUser.display_name} — click to log out"
				onclick={handleLogout}
				aria-label="User: {$currentUser.display_name}"
			>
				<span class="user-initial">{$currentUser.display_name.charAt(0).toUpperCase()}</span>
			</button>
		{/if}
	</div>
</aside>

<style>
	.sidebar-rail {
		display: flex;
		flex-direction: column;
		width: 48px;
		background: linear-gradient(180deg, var(--surface-700) 0%, var(--surface-800) 100%);
		border-right: 1px solid var(--surface-border);
		height: 100%;
		flex-shrink: 0;
		align-items: center;
		padding: 8px 0;
	}

	.sidebar-rail::after {
		content: '';
		position: absolute;
		top: 0;
		right: 0;
		bottom: 0;
		width: 1px;
		background: linear-gradient(180deg, transparent 0%, var(--tint-active) 50%, transparent 100%);
	}

	.rail-projects {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 6px;
		flex: 1;
		overflow-y: auto;
		padding: 4px 0;
	}

	.rail-projects::-webkit-scrollbar { width: 0; }

	.rail-project {
		display: flex;
		align-items: center;
		justify-content: center;
		width: 32px;
		height: 32px;
		border-radius: 50%;
		background: var(--surface-600);
		border: 2px solid transparent;
		cursor: pointer;
		transition: all 0.15s ease;
		flex-shrink: 0;
	}

	.rail-project:hover {
		background: var(--surface-500);
		border-color: var(--surface-border-light);
	}

	.rail-project.active {
		border-color: var(--amber-500);
		background: var(--tint-active);
	}

	.project-abbr {
		font-size: 10px;
		font-weight: 700;
		letter-spacing: 0.05em;
		color: var(--project-color, var(--text-secondary));
	}

	.rail-project.active .project-abbr {
		color: var(--amber-400);
	}

	.rail-bottom {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 6px;
		padding-top: 8px;
		border-top: 1px solid var(--surface-border);
	}

	.rail-btn {
		display: flex;
		align-items: center;
		justify-content: center;
		width: 28px;
		height: 28px;
		background: transparent;
		border: 1px solid transparent;
		border-radius: 4px;
		color: var(--text-muted);
		cursor: pointer;
		transition: all 0.15s ease;
		flex-shrink: 0;
		padding: 0;
	}

	.rail-btn:hover {
		background: var(--tint-hover);
		border-color: var(--surface-border);
		color: var(--text-secondary);
	}

	.rail-btn:disabled { opacity: 0.5; cursor: not-allowed; }

	.rail-btn svg { width: 14px; height: 14px; }

	.rail-spinner {
		width: 10px;
		height: 10px;
		border: 1.5px solid var(--surface-border);
		border-top-color: var(--amber-500);
		border-radius: 50%;
		animation: spin 0.8s linear infinite;
	}

	@keyframes spin { to { transform: rotate(360deg); } }

	.user-btn {
		border-radius: 50%;
		width: 28px;
		height: 28px;
	}

	.user-initial {
		font-size: 11px;
		font-weight: 700;
		color: var(--text-secondary);
	}

	/* Analog theme */
	:global([data-theme="analog"]) .sidebar-rail {
		background-color: var(--surface-800);
		background-image: var(--grain-fine), var(--grain-coarse);
		background-blend-mode: multiply, multiply;
	}

	:global([data-theme="analog"]) .sidebar-rail::after {
		background: var(--amber-600);
		width: 1.5px;
	}
</style>
