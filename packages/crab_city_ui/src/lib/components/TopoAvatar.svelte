<script lang="ts">
	import { getAvatarPaths } from '$lib/utils/avatarCache';

	interface Props {
		/** Unique identifier to seed the terrain (username, agent id, etc) */
		identity: string;
		/** Avatar type: 'human' for smooth contours, 'agent' for angular/spiky */
		type?: 'human' | 'agent';
		/** Visual variant affecting color scheme */
		variant?: 'user' | 'assistant' | 'thinking';
		/** Size in pixels */
		size?: number;
		/** Whether to show subtle animation */
		animated?: boolean;
	}

	let { identity, type = 'agent', variant = 'assistant', size = 28, animated = false }: Props = $props();

	// Get cached avatar paths (memoized - only regenerates on config change)
	const avatar = $derived(getAvatarPaths({ identity, type, variant, size }));

	// Color schemes for different variants
	const colors = $derived.by(() => {
		switch (variant) {
			case 'user':
				return {
					bg: '#030806',
					border: '#0d2a20',
					stroke: '#4ade80',
					glow: 'rgba(34, 197, 94, 0.5)'
				};
			case 'thinking':
				return {
					bg: '#06030a',
					border: '#1a102a',
					stroke: '#a78bfa',
					glow: 'rgba(139, 92, 246, 0.5)'
				};
			case 'assistant':
			default:
				return {
					bg: '#050302',
					border: '#2a1a0a',
					stroke: '#fbbf24',
					glow: 'rgba(251, 146, 60, 0.5)'
				};
		}
	});
</script>

<svg
	class="topo-avatar"
	class:animated
	viewBox="0 0 32 32"
	width={size}
	height={size}
	style="--glow-color: {colors.glow}; --border-color: {colors.border};"
>
	<defs>
		<clipPath id={avatar.clipId}>
			<circle cx="16" cy="16" r="15" />
		</clipPath>
	</defs>

	<!-- Background circle - solid color -->
	<circle cx="16" cy="16" r="15" fill={colors.bg} stroke={colors.border} stroke-width="1" />

	<!-- Contour lines (from cache) -->
	<g clip-path="url(#{avatar.clipId})">
		{#each avatar.paths as path, i}
			<path
				d={path}
				fill="none"
				stroke={colors.stroke}
				stroke-width={0.75}
				stroke-linecap="round"
				class="contour-line"
				style="--delay: {i * 0.06}s"
			/>
		{/each}
	</g>

	<!-- Subtle inner glow -->
	<circle cx="16" cy="16" r="14" fill="none" stroke={colors.stroke} stroke-width="0.5" stroke-opacity="0.2" />
</svg>

<style>
	.topo-avatar {
		display: block;
		border-radius: 50%;
		box-shadow:
			0 0 8px var(--glow-color),
			inset 0 0 4px var(--glow-color);
		transition: box-shadow 0.3s ease;
	}

	.topo-avatar:hover {
		box-shadow:
			0 0 12px var(--glow-color),
			0 0 20px var(--glow-color),
			inset 0 0 6px var(--glow-color);
	}

	.contour-line {
		vector-effect: non-scaling-stroke;
	}

	/* Subtle pulse animation */
	.topo-avatar.animated .contour-line {
		animation: contour-pulse 3s ease-in-out infinite;
		animation-delay: var(--delay);
	}

	@keyframes contour-pulse {
		0%,
		100% {
			stroke-opacity: 0.4;
		}
		50% {
			stroke-opacity: 0.8;
		}
	}

	/* Active/thinking state animation */
	.topo-avatar.animated {
		animation: avatar-glow 2s ease-in-out infinite;
	}

	@keyframes avatar-glow {
		0%,
		100% {
			box-shadow:
				0 0 8px var(--glow-color),
				inset 0 0 4px var(--glow-color);
		}
		50% {
			box-shadow:
				0 0 16px var(--glow-color),
				0 0 24px var(--glow-color),
				inset 0 0 8px var(--glow-color);
		}
	}
</style>
