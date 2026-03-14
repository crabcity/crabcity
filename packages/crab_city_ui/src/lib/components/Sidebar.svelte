<script lang="ts">
	import { base } from '$app/paths';
	import { onMount } from 'svelte';
	import {
		instanceList,
		currentInstanceId,
		selectInstance
	} from '$lib/stores/instances';
	import { projects, currentProject } from '$lib/stores/projects';
	import { clearInstanceVerbs } from '$lib/stores/activity';
	import { currentUser, isAuthenticated, logout } from '$lib/stores/auth';
	import QuickSettings from './settings/QuickSettings.svelte';
	import CreateInstanceModal from './CreateInstanceModal.svelte';
	import type { Instance } from '$lib/types';

	let showQuickSettings = $state(false);
	let showCreateModal = $state(false);

	async function handleLogout() {
		await logout();
		window.location.href = `${base}/login`;
	}

	// Track previous states for notifications
	const previousStates = new Map<string, string>();

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
	$effect(() => {
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
	});

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
			onclick={() => showCreateModal = true}
			title="New project"
			aria-label="Create new project"
		>
			<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
				<line x1="12" y1="5" x2="12" y2="19" />
				<line x1="5" y1="12" x2="19" y2="12" />
			</svg>
		</button>

		<button
			class="rail-btn"
			class:active={showQuickSettings}
			onclick={() => showQuickSettings = !showQuickSettings}
			title="Settings"
			aria-label="Settings"
		>
			<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5">
				<circle cx="8" cy="8" r="2.5" />
				<path d="M8 1.5v2M8 12.5v2M1.5 8h2M12.5 8h2M3.1 3.1l1.4 1.4M11.5 11.5l1.4 1.4M3.1 12.9l1.4-1.4M11.5 4.5l1.4-1.4" />
			</svg>
		</button>

		{#if showQuickSettings}
			<QuickSettings onclose={() => showQuickSettings = false} />
		{/if}

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

{#if showCreateModal}
	<CreateInstanceModal mode="project" onclose={() => showCreateModal = false} />
{/if}

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

	.rail-btn.active {
		background: var(--tint-active);
		border-color: var(--amber-600);
		color: var(--amber-400);
	}

	.rail-btn svg { width: 14px; height: 14px; }

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
