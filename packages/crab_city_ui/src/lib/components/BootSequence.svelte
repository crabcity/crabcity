<script lang="ts">
	import { onMount } from 'svelte';

	interface Props {
		/** Called when boot sequence finishes */
		onComplete: () => void;
		/** Whether the WebSocket is connected — accelerates the sequence */
		wsConnected?: boolean;
	}

	let { onComplete, wsConnected = false }: Props = $props();

	// Lines that always appear, in order
	const HEADER = 'CRAB CITY v1.0';
	const FOOTER_LINES = ['WEBSOCKET LINK ESTABLISHED', 'READY.'];

	// Pool of fun middle lines — we pick a random subset each boot
	const MIDDLE_POOL = [
		'INITIALIZING PHOSPHOR DISPLAY...',
		'RETICULATING SPLINES...',
		'CALIBRATING PINCER SERVOS...',
		'WARMING UP VACUUM TUBES...',
		'LOADING EMPATHY MODULES...',
		'CONNECTING TO INSTANCE MANAGER...',
		'POLISHING EXOSKELETON...',
		'SCANNING FOR CRUSTACEAN ACTIVITY...',
		'NEGOTIATING WITH HERMIT CRABS...',
		'SYNCHRONIZING TIDE TABLES...',
		'DEFRAGMENTING SHELL MEMORY...',
		'DEPLOYING SANDCASTLE DEFENSES...',
		'TUNING AMBIENT FREQUENCIES...',
		'CHECKING SIDEWAYS LOCOMOTION...',
		'ESTABLISHING NEURAL HANDSHAKE...',
		'BUFFERING EMOTIONAL WAVELENGTHS...',
		'PRESSURIZING CRAB TANK...',
		'ALIGNING MOONLIGHT SENSORS...',
		'LOADING CONVERSATION MATRIX...',
		'COUNTING LEGS... 10 FOUND. OK',
	];

	function pickRandom<T>(arr: T[], n: number): T[] {
		const shuffled = [...arr].sort(() => Math.random() - 0.5);
		return shuffled.slice(0, n);
	}

	const BOOT_LINES = [HEADER, ...pickRandom(MIDDLE_POOL, 4), ...FOOTER_LINES];

	let visibleLines = $state<string[]>([]);
	let cursorVisible = $state(true);
	let fading = $state(false);

	onMount(() => {
		// Only play once per browser session
		if (sessionStorage.getItem('crab-city-booted')) {
			onComplete();
			return;
		}

		sessionStorage.setItem('crab-city-booted', '1');

		let lineIndex = 0;

		function showNextLine() {
			if (lineIndex < BOOT_LINES.length) {
				visibleLines = [...visibleLines, BOOT_LINES[lineIndex]];
				lineIndex++;
				setTimeout(showNextLine, wsConnected ? 20 : 100);
			} else {
				// "READY." shown — start fade out after a beat
				setTimeout(() => {
					fading = true;
					setTimeout(onComplete, 400);
				}, 300);
			}
		}

		// Start the sequence after a brief delay
		setTimeout(showNextLine, 150);

		// Cursor blink
		const cursorInterval = setInterval(() => {
			cursorVisible = !cursorVisible;
		}, 400);

		return () => clearInterval(cursorInterval);
	});
</script>

{#if !fading || fading}
<div class="boot-overlay" class:fading>
	<div class="boot-screen">
		{#each visibleLines as line, i}
			<div class="boot-line" class:ready={line === 'READY.'}>
				{line}
			</div>
		{/each}
		<span class="boot-cursor" class:visible={cursorVisible}>█</span>
	</div>
</div>
{/if}

<style>
	.boot-overlay {
		position: fixed;
		inset: 0;
		z-index: 10000;
		background: var(--surface-900);
		display: flex;
		align-items: center;
		justify-content: center;
		transition: opacity 0.4s ease;
	}

	.boot-overlay.fading {
		opacity: 0;
	}

	.boot-screen {
		font-family: var(--font-mono);
		font-size: 13px;
		color: var(--text-primary);
		text-shadow: var(--emphasis);
		max-width: 500px;
		padding: 24px;
	}

	.boot-line {
		margin-bottom: 4px;
		letter-spacing: 0.08em;
		animation: line-appear 0.08s ease-out;
	}

	.boot-line.ready {
		color: var(--status-green);
		text-shadow: var(--emphasis);
		font-weight: 700;
		margin-top: 8px;
	}

	@keyframes line-appear {
		from { opacity: 0; transform: translateY(2px); }
		to { opacity: 1; transform: translateY(0); }
	}

	.boot-cursor {
		opacity: 0;
		color: var(--amber-500);
		font-size: 13px;
	}

	.boot-cursor.visible {
		opacity: 1;
	}

	/* ANALOG THEME — colophon page, like the front matter of a fine press edition */
	:global([data-theme="analog"]) .boot-screen {
		font-family: 'Source Serif 4', Georgia, serif;
		font-size: 14px;
		letter-spacing: 0.02em;
		text-shadow: 0 0 1px rgba(42, 31, 24, 0.15);
	}
</style>
