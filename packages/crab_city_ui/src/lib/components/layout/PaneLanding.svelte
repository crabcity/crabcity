<script lang="ts">
	import SnakeTeaser from '../SnakeTeaser.svelte';
	import SnakeGame from '../SnakeGame.svelte';

	// Easter egg: triple-click the monitor icon to launch snake
	let clicks = $state(0);
	let clickTimer: ReturnType<typeof setTimeout> | null = null;
	let showSnake = $state(false);

	function onIconClick() {
		clicks++;
		if (clickTimer) clearTimeout(clickTimer);
		clickTimer = setTimeout(() => { clicks = 0; }, 2000);
		if (clicks >= 3) {
			clicks = 0;
			showSnake = true;
		}
	}
</script>

{#if showSnake}
	<SnakeGame onexit={() => { showSnake = false; }} />
{:else}
	<div class="landing">
		<div class="empty-content">
			<!-- svelte-ignore a11y_click_events_have_key_events -->
			<!-- svelte-ignore a11y_no_static_element_interactions -->
			<div
				class="empty-icon"
				onclick={onIconClick}
				style="opacity: {0.3 + clicks * 0.25}; filter: drop-shadow(0 0 {clicks * 8}px var(--amber-500));"
			>
				<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5">
					<path
						d="M9.75 17L9 20l-1 1h8l-1-1-.75-3M3 13h18M5 17h14a2 2 0 002-2V5a2 2 0 00-2-2H5a2 2 0 00-2 2v10a2 2 0 002 2z"
					/>
				</svg>
				<div class="monitor-screen">
					<SnakeTeaser />
				</div>
			</div>
			<h2>No Instance Selected</h2>
			<p>Select an instance from the header or sidebar</p>
		</div>
	</div>
{/if}

<style>
	.landing {
		display: flex;
		align-items: center;
		justify-content: center;
		flex: 1;
	}

	.empty-content {
		text-align: center;
		color: var(--text-muted);
	}

	.empty-icon {
		position: relative;
		width: 80px;
		height: 80px;
		margin: 0 auto 20px;
		opacity: 0.3;
		color: var(--amber-500);
		cursor: pointer;
		transition: opacity 0.2s ease, filter 0.2s ease;
	}

	.empty-icon svg {
		width: 100%;
		height: 100%;
	}

	.monitor-screen {
		position: absolute;
		left: 10px;
		top: 10px;
		width: 60px;
		height: 33px;
		overflow: hidden;
		border-radius: 1px;
	}

	.empty-content h2 {
		margin: 0 0 12px;
		font-size: 14px;
		font-weight: 600;
		letter-spacing: 0.1em;
		text-transform: uppercase;
		color: var(--text-secondary);
	}

	.empty-content p {
		margin: 0;
		font-size: 12px;
		letter-spacing: 0.05em;
	}
</style>
