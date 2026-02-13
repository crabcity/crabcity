<script lang="ts">
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { base } from '$app/paths';
	import { page } from '$app/stores';
	import { fetchInstances, initFromUrl, selectInstance, clearSelection } from '$lib/stores/instances';
	import { fetchTasks, migrateFromLocalStorage } from '$lib/stores/tasks';
	import { initMultiplexedConnection, disconnectAll } from '$lib/stores/websocket';
	import { viewportWidth } from '$lib/stores/ui';
	import { toggleDebugPanel } from '$lib/stores/metrics';
	import {
		checkAuth, authChecked, authEnabled, isAuthenticated, needsSetup
	} from '$lib/stores/auth';
	import { theme } from '$lib/stores/settings';
	import { resolveAuthGuard } from '$lib/utils/authGuard';
	import DebugPanel from '$lib/components/DebugPanel.svelte';

	let appInitialized = $state(false);

	// Handle browser back/forward navigation
	function handlePopState() {
		const instanceId = initFromUrl();
		if (instanceId) {
			// Don't push history when responding to popstate
			selectInstance(instanceId, false);
		} else {
			clearSelection(false);
		}
	}

	// Handle keyboard shortcuts
	function handleKeydown(e: KeyboardEvent) {
		// Ctrl+Shift+D (or Cmd+Shift+D on Mac) toggles debug panel
		if ((e.ctrlKey || e.metaKey) && e.shiftKey && e.key === 'D') {
			e.preventDefault();
			toggleDebugPanel();
		}
	}

	// Frame jank detector - warns when main thread is blocked
	function startJankDetector() {
		let lastFrame = performance.now();
		let rafId: number;

		function check() {
			const now = performance.now();
			const delta = now - lastFrame;
			if (delta > 100) {
				console.warn(`[Jank] Frame blocked for ${delta.toFixed(0)}ms`);
			}
			lastFrame = now;
			rafId = requestAnimationFrame(check);
		}

		rafId = requestAnimationFrame(check);
		return () => cancelAnimationFrame(rafId);
	}

	// Ink transition — the UI is ruled by hand when entering analog mode.
	// Structural lines (sidebar edge, header borders) draw themselves in
	// with pen-pressure gradients, followed by faint notebook ruling across
	// the content area. Each line fades to reveal the real border underneath.
	function playInkTransition() {
		const container = document.createElement('div');
		container.className = 'ink-rules-container';

		const rules: { dir: 'h' | 'v'; css: string; light?: boolean }[] = [
			// Structural lines — heavier, drawn first
			// Sidebar right edge: the first margin ruled
			{ dir: 'v', css: 'left:259px;top:0;height:100%;--d:0ms;--dur:480ms' },
			// Sidebar header bottom
			{ dir: 'h', css: 'left:0;top:69px;width:260px;--d:100ms;--dur:320ms' },
			// Main header bottom rule
			{ dir: 'h', css: 'left:0;top:49px;width:100%;--d:180ms;--dur:420ms' },

			// Notebook ruling — lighter, drawn later, like guide lines on the page
			{ dir: 'h', css: 'left:280px;top:30%;width:calc(100% - 300px);--d:320ms;--dur:340ms', light: true },
			{ dir: 'h', css: 'left:280px;top:50%;width:calc(100% - 300px);--d:400ms;--dur:340ms', light: true },
			{ dir: 'h', css: 'left:280px;top:70%;width:calc(100% - 300px);--d:480ms;--dur:340ms', light: true },
		];

		for (const { dir, css, light } of rules) {
			const el = document.createElement('div');
			el.className = `ink-rule ink-rule-${dir}${light ? ' ink-rule-light' : ''}`;
			el.style.cssText = css;
			container.appendChild(el);
		}

		document.body.appendChild(container);

		// Cleanup after the last line finishes
		setTimeout(() => container.remove(), 1100);
	}

	// -----------------------------------------------------------------------
	// Reactive auth guard + app initialization.
	//
	// Pure decision in resolveAuthGuard(), side effects here.
	// Re-evaluates whenever $authChecked, $authEnabled, $isAuthenticated,
	// $needsSetup, or $page change — covers every path into every state.
	// -----------------------------------------------------------------------
	$effect(() => {
		const action = resolveAuthGuard({
			authChecked: $authChecked,
			authEnabled: $authEnabled,
			isAuthenticated: $isAuthenticated,
			needsSetup: $needsSetup,
			pathname: $page.url.pathname,
			basePath: base,
			appInitialized,
		});

		switch (action.kind) {
			case 'wait':
				break;
			case 'redirect':
				goto(action.to);
				break;
			case 'init_app':
				appInitialized = true;
				initMultiplexedConnection();
				fetchInstances().then(() => {
					const instanceId = initFromUrl();
					if (instanceId) selectInstance(instanceId, false);
				});
				fetchTasks().then(() => migrateFromLocalStorage());
				break;
			case 'noop':
				break;
		}
	});

	// -----------------------------------------------------------------------
	// One-time browser setup (event listeners, theme binding, jank detector).
	// Also kicks off the auth check that feeds the $effect above.
	// -----------------------------------------------------------------------
	onMount(() => {
		const stopJankDetector = startJankDetector();

		let prevTheme: string | null = null;
		const unsubTheme = theme.subscribe((t) => {
			const isSwitch = prevTheme !== null && prevTheme !== t;
			document.body.setAttribute('data-theme', t);
			if (isSwitch && t === 'analog') {
				playInkTransition();
			}
			prevTheme = t;
		});

		// Kick off auth — the result flows into stores, the $effect reacts.
		checkAuth();

		window.addEventListener('popstate', handlePopState);
		const handleResize = () => viewportWidth.set(window.innerWidth);
		window.addEventListener('resize', handleResize);
		window.addEventListener('keydown', handleKeydown);

		return () => {
			unsubTheme();
			stopJankDetector();
			disconnectAll();
			window.removeEventListener('popstate', handlePopState);
			window.removeEventListener('resize', handleResize);
			window.removeEventListener('keydown', handleKeydown);
		};
	});
</script>

<slot />
<DebugPanel />

<style>
	@import url('https://fonts.googleapis.com/css2?family=JetBrains+Mono:wght@400;500;600;700&display=swap');
	@import url('https://fonts.googleapis.com/css2?family=Source+Serif+4:ital,opsz,wght@0,8..60,300;0,8..60,400;0,8..60,500;0,8..60,600;0,8..60,700;1,8..60,400;1,8..60,500&display=swap');
	@import url('https://fonts.googleapis.com/css2?family=Newsreader:ital,opsz,wght@0,6..72,400;0,6..72,500;0,6..72,600;0,6..72,700;1,6..72,400;1,6..72,500&display=swap');

	:global(*) {
		box-sizing: border-box;
	}

	:global(:root) {
		/* Amber phosphor palette */
		--amber-600: #d97706;
		--amber-500: #fb923c;
		--amber-400: #fdba74;
		--amber-300: #fed7aa;
		--amber-glow: rgba(251, 146, 60, 0.35);
		--amber-glow-strong: rgba(251, 146, 60, 0.6);

		/* Purple (thinking state) */
		--purple-500: #8b5cf6;
		--purple-400: #a78bfa;
		--purple-glow: rgba(139, 92, 246, 0.5);

		/* Surfaces */
		--surface-900: #0a0806;
		--surface-800: #0f0c0a;
		--surface-700: #15110d;
		--surface-600: #1a1510;
		--surface-500: #201a14;
		--surface-400: #2a231a;
		--surface-border: #3a2a1a;
		--surface-border-light: #4a3a2a;

		/* Text */
		--text-primary: #fdba74;
		--text-secondary: #a08060;
		--text-muted: #6a5040;

		/* Status */
		--status-green: #22c55e;
		--status-red: #ef4444;
		--status-yellow: #fbbf24;

		/* Responsive spacing */
		--spacing-xs: 4px;
		--spacing-sm: 8px;
		--spacing-md: 12px;
		--spacing-lg: 16px;
		--spacing-xl: 20px;

		/* Sidebar width */
		--sidebar-width: 260px;

		/* Touch targets */
		--touch-target-min: 44px;

		/* Ambient state - default is idle (dimmer, cooler amber) */
		--ambient-glow: rgba(251, 146, 60, 0.35);
		--ambient-accent: #d97706;
		--ambient-tint: rgba(251, 146, 60, 0.01);
		--ambient-scanline-opacity: 0.08;

		/* =======================================================
		   SEMANTIC TOKENS — intent, not appearance.
		   Components use THESE. Themes set THESE.
		   ======================================================= */

		/* Typography stacks */
		--font-body: 'JetBrains Mono', 'SF Mono', 'Consolas', 'Monaco', monospace;
		--font-display: 'JetBrains Mono', 'SF Mono', 'Consolas', 'Monaco', monospace;
		--font-mono: 'JetBrains Mono', 'SF Mono', 'Consolas', 'Monaco', monospace;

		/* Interactive tints — background color on state changes */
		--tint-hover: rgba(251, 146, 60, 0.05);
		--tint-active: rgba(251, 146, 60, 0.1);
		--tint-active-strong: rgba(251, 146, 60, 0.15);
		--tint-focus: rgba(251, 146, 60, 0.2);
		--tint-subtle: rgba(251, 146, 60, 0.02);
		--tint-thinking: rgba(139, 92, 246, 0.06);
		--tint-thinking-strong: rgba(139, 92, 246, 0.1);
		--tint-selection: rgba(251, 146, 60, 0.3);

		/* Effect: emphasis — text-shadow for glowing/prominent text */
		--emphasis: 0 0 10px var(--ambient-glow);
		--emphasis-strong: 0 0 15px var(--ambient-glow);

		/* Effect: elevation — box-shadow for raised elements */
		--elevation-low: 0 0 10px rgba(251, 146, 60, 0.1);
		--elevation-high: 0 0 20px rgba(251, 146, 60, 0.15);

		/* Effect: recess — box-shadow for inset/sunken elements */
		--recess: inset 0 0 30px rgba(251, 146, 60, 0.04);
		--recess-border: inset 0 1px 0 rgba(251, 146, 60, 0.06);

		/* Effect: depth — combined for cards/panels */
		--depth-up: 0 0 15px rgba(251, 146, 60, 0.1), inset 0 1px 0 rgba(251, 146, 60, 0.1);
		--depth-down: inset 0 0 20px rgba(0, 0, 0, 0.15);

		/* Overlay textures */
		--texture-overlay: repeating-linear-gradient(0deg, transparent, transparent 2px, black 2px, black 4px);
		--texture-opacity: var(--ambient-scanline-opacity, 0.08);
		--vignette: var(--ambient-tint);

		/* Multi-user presence indicators */
		--tint-presence: rgba(139, 92, 246, 0.2);
		--tint-presence-border: rgba(139, 92, 246, 0.3);

		/* Panel header/footer — recessed panel background */
		--panel-inset: rgba(0, 0, 0, 0.2);

		/* Spinner track */
		--spinner-track: rgba(251, 146, 60, 0.3);

		/* Backdrops & overlays */
		--backdrop: rgba(0, 0, 0, 0.7);
		--shadow-panel: -4px 0 20px rgba(0, 0, 0, 0.4);
		--shadow-dropdown: 0 4px 16px rgba(0, 0, 0, 0.5);

		/* Primary action buttons */
		--btn-primary-bg: linear-gradient(180deg, var(--amber-600) 0%, #b45309 100%);
		--btn-primary-text: #000;
		--btn-primary-text-shadow: 0 1px 0 rgba(255, 255, 255, 0.1);

		/* Status tints — functional color at alpha */
		--status-red-tint: rgba(239, 68, 68, 0.1);
		--status-red-border: rgba(239, 68, 68, 0.2);
		--status-red-strong: rgba(239, 68, 68, 0.2);
		--status-red-text: #f87171;
		--status-green-tint: rgba(34, 197, 94, 0.1);
		--status-green-border: rgba(34, 197, 94, 0.2);
		--status-green-text: #6ee7b7;
		--status-blue: #60a5fa;
		--status-blue-tint: rgba(96, 165, 250, 0.2);
		--status-blue-text: #93c5fd;

		/* Active indicator — how "this is selected" is expressed */
		--active-border: 1px solid var(--amber-600);
		--active-accent-width: 0px;
	}

	/* Ambient state overrides - the whole room shifts */
	:global([data-claude-state="thinking"]) {
		--amber-glow: rgba(139, 92, 246, 0.5);
		--amber-glow-strong: rgba(139, 92, 246, 0.8);
		--ambient-glow: rgba(139, 92, 246, 0.5);
		--ambient-accent: #8b5cf6;
		--ambient-tint: rgba(139, 92, 246, 0.02);
		--ambient-scanline-opacity: 0.06;
	}

	:global([data-claude-state="responding"]) {
		--amber-glow: rgba(251, 146, 60, 0.5);
		--amber-glow-strong: rgba(251, 146, 60, 0.8);
		--ambient-glow: rgba(251, 146, 60, 0.5);
		--ambient-accent: #fb923c;
		--ambient-tint: rgba(251, 146, 60, 0.025);
		--ambient-scanline-opacity: 0.08;
	}

	:global([data-claude-state="tool_executing"]) {
		--amber-glow: rgba(251, 191, 36, 0.5);
		--amber-glow-strong: rgba(251, 191, 36, 0.8);
		--ambient-glow: rgba(251, 191, 36, 0.5);
		--ambient-accent: #fbbf24;
		--ambient-tint: rgba(251, 191, 36, 0.02);
		--ambient-scanline-opacity: 0.10;
	}

	:global([data-claude-state="active"]) {
		--amber-glow: rgba(251, 146, 60, 0.5);
		--amber-glow-strong: rgba(251, 146, 60, 0.8);
		--ambient-glow: rgba(251, 146, 60, 0.5);
		--ambient-accent: #fb923c;
		--ambient-tint: rgba(251, 146, 60, 0.02);
		--ambient-scanline-opacity: 0.08;
	}

	/* Signal lost — the whole UI loses signal when disconnected */
	:global([data-connection="error"]),
	:global([data-connection="disconnected"]) {
		--amber-glow: rgba(251, 146, 60, 0.15);
		--amber-glow-strong: rgba(251, 146, 60, 0.3);
		--ambient-scanline-opacity: 0.15;
		--ambient-glow: rgba(251, 146, 60, 0.2);
		--ambient-tint: rgba(0, 0, 0, 0.03);
	}

	:global([data-connection="reconnecting"]) {
		--amber-glow: rgba(251, 191, 36, 0.3);
		--amber-glow-strong: rgba(251, 191, 36, 0.5);
		--ambient-scanline-opacity: 0.12;
		--ambient-glow: rgba(251, 191, 36, 0.4);
		--ambient-tint: rgba(251, 191, 36, 0.01);
	}

	/* ==========================================================================
	   ANALOG THEME — Ink on Paper
	   Material shift: phosphor screen → printed page
	   ========================================================================== */

	:global([data-theme="analog"]) {
		/* === INK — fountain pen, real pigment, pools and bleeds === */
		/* Iron gall ink — the classic. Near-black with blue-violet undertone. */
		--amber-600: #2a1f18;      /* concentrated iron gall — nearly black */
		--amber-500: #4a3528;      /* mid-stroke density */
		--amber-400: #3a2a1e;      /* nib-lift, slightly less saturated */
		--amber-300: #1a1410;      /* full saturation pool */
		--amber-glow: rgba(42, 31, 24, 0.08);
		--amber-glow-strong: rgba(42, 31, 24, 0.18);

		/* Thinking — Pilot Iroshizuku Kon-peki (deep cerulean) */
		--purple-500: #1a4a7a;
		--purple-400: #2a5a8a;
		--purple-glow: rgba(26, 74, 122, 0.12);

		/* === PAPER — pure white stock === */
		--surface-900: #ffffff;    /* top sheet — white */
		--surface-800: #fafafa;    /* underlayer */
		--surface-700: #f3f3f1;    /* inset/recessed */
		--surface-600: #eaeae7;    /* deeper inset */
		--surface-500: #dadadb;    /* pressed/debossed */
		--surface-400: #c4c4c0;    /* heavy impression */
		--surface-border: #9a9080;  /* ruled line — drawn with a straightedge, real graphite */
		--surface-border-light: #b5aa96;

		/* === TEXT — fountain pen ink density === */
		--text-primary: #141210;   /* like fresh Noodler's Heart of Darkness */
		--text-secondary: #3a3630; /* second pass, nib running dry */
		--text-muted: #6a645c;     /* pencil annotation, HB graphite */

		/* Status — real pigments, from the watercolor tray */
		--status-green: #1a5e2a;   /* Hooker's green deep */
		--status-red: #8a2020;     /* alizarin crimson */
		--status-yellow: #6a5510;  /* yellow ochre concentrate */

		/* Ambient — ink bleeding into paper fibers */
		--ambient-glow: rgba(42, 31, 24, 0.06);
		--ambient-accent: #2a1f18;
		--ambient-tint: rgba(42, 31, 24, 0.008);
		--ambient-scanline-opacity: 0;

		/* === SEMANTIC TOKENS — drafting table material === */

		/* Typography — pen-friendly faces */
		--font-body: 'Source Serif 4', 'Georgia', 'Times New Roman', serif;
		--font-display: 'Newsreader', 'Georgia', serif;
		--font-mono: 'JetBrains Mono', 'SF Mono', 'Consolas', monospace;

		/* Tints — ink wash spreading through wet paper */
		--tint-hover: rgba(42, 31, 24, 0.04);
		--tint-active: rgba(42, 31, 24, 0.08);
		--tint-active-strong: rgba(42, 31, 24, 0.14);
		--tint-focus: rgba(42, 31, 24, 0.12);
		--tint-subtle: rgba(42, 31, 24, 0.02);
		--tint-thinking: rgba(26, 74, 122, 0.06);
		--tint-thinking-strong: rgba(26, 74, 122, 0.12);
		--tint-selection: rgba(42, 31, 24, 0.12);

		/* Emphasis — ink bleed, not glow. Slight feathering at edges. */
		--emphasis: 0 0 2px rgba(42, 31, 24, 0.25);
		--emphasis-strong: 0 0 3px rgba(42, 31, 24, 0.35), 0 0 6px rgba(42, 31, 24, 0.08);

		/* Elevation — paper cockle, not floating */
		--elevation-low: 0 1px 2px rgba(20, 18, 16, 0.08), 0 0 1px rgba(20, 18, 16, 0.12);
		--elevation-high: 0 2px 4px rgba(20, 18, 16, 0.1), 0 0 1px rgba(20, 18, 16, 0.15);

		/* Recess — pressed into the page, ink pooling in the valley */
		--recess: inset 0 1px 4px rgba(20, 18, 16, 0.06), inset 0 0 1px rgba(20, 18, 16, 0.08);
		--recess-border: inset 0 1px 0 rgba(20, 18, 16, 0.05);

		/* Depth — card stock stacking */
		--depth-up: 0 1px 3px rgba(20, 18, 16, 0.08), 0 0 1px rgba(20, 18, 16, 0.12);
		--depth-down: inset 0 1px 4px rgba(20, 18, 16, 0.06);

		/* Overlay — heavy paper grain (cold-press watercolor stock) */
		--texture-overlay: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='400' height='400'%3E%3Cfilter id='f'%3E%3CfeTurbulence type='fractalNoise' baseFrequency='0.85' numOctaves='8' stitchTiles='stitch'/%3E%3CfeColorMatrix type='saturate' values='0'/%3E%3C/filter%3E%3Crect width='400' height='400' filter='url(%23f)' opacity='0.18'/%3E%3C/svg%3E");
		--texture-opacity: 1;
		--vignette: radial-gradient(ellipse at 50% 40%, transparent 0%, transparent 40%, rgba(120, 110, 90, 0.04) 70%, rgba(80, 70, 55, 0.10) 100%);

		/* Multi-user presence — cerulean ink */
		--tint-presence: rgba(26, 74, 122, 0.08);
		--tint-presence-border: rgba(26, 74, 122, 0.2);

		/* Panel header/footer — paper shadow */
		--panel-inset: rgba(20, 18, 16, 0.04);

		/* Spinner track — graphite */
		--spinner-track: var(--surface-border);

		/* Backdrops & overlays — vellum over the page */
		--backdrop: rgba(254, 254, 254, 0.75);
		--shadow-panel: -2px 0 6px rgba(20, 18, 16, 0.1);
		--shadow-dropdown: 0 2px 6px rgba(20, 18, 16, 0.12), 0 0 1px rgba(20, 18, 16, 0.15);

		/* Primary action buttons — ink stamp, heavy impression */
		--btn-primary-bg: linear-gradient(180deg, #3a2a1e 0%, #1a1410 100%);
		--btn-primary-text: var(--surface-900);
		--btn-primary-text-shadow: none;

		/* Status tints — watercolor pigment dropped on wet paper */
		--status-red-tint: rgba(138, 32, 32, 0.08);
		--status-red-border: rgba(138, 32, 32, 0.25);
		--status-red-strong: rgba(138, 32, 32, 0.14);
		--status-red-text: #8a2020;
		--status-green-tint: rgba(26, 94, 42, 0.08);
		--status-green-border: rgba(26, 94, 42, 0.25);
		--status-green-text: #1a5e2a;
		--status-blue: #1a4a7a;
		--status-blue-tint: rgba(26, 74, 122, 0.1);
		--status-blue-text: #1a4a7a;

		/* Active indicator — heavy pen stroke down the left margin */
		--active-border: 2px solid var(--amber-600);
		--active-accent-width: 3px;

		/* === SCROLLING GRAIN — applied directly to element backgrounds === */
		/* Fine fiber — sharp micro-detail for small elements (buttons, badges, inline code) */
		--grain-fine: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='200' height='200'%3E%3Cfilter id='f'%3E%3CfeTurbulence type='fractalNoise' baseFrequency='0.9' numOctaves='8' stitchTiles='stitch'/%3E%3CfeColorMatrix type='saturate' values='0'/%3E%3C/filter%3E%3Crect width='200' height='200' filter='url(%23f)' opacity='0.14'/%3E%3C/svg%3E");
		/* Coarse pulp — large-scale paper texture for panels and surfaces */
		--grain-coarse: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='150' height='150'%3E%3Cfilter id='g'%3E%3CfeTurbulence type='turbulence' baseFrequency='0.35' numOctaves='2' stitchTiles='stitch'/%3E%3CfeColorMatrix type='saturate' values='0'/%3E%3C/filter%3E%3Crect width='150' height='150' filter='url(%23g)' opacity='0.08'/%3E%3C/svg%3E");
		/* Ink wash — asymmetric radial gradient like diluted ink pooling */
		--ink-wash: radial-gradient(ellipse at 30% 20%, rgba(42,31,24,0.03) 0%, transparent 70%);
	}

	/* Analog ambient overrides — ink changes character under pressure */
	:global([data-theme="analog"][data-claude-state="thinking"]) {
		--amber-glow: rgba(26, 74, 122, 0.1);
		--amber-glow-strong: rgba(26, 74, 122, 0.2);
		--ambient-glow: rgba(26, 74, 122, 0.1);
		--ambient-accent: #1a4a7a;
		--ambient-tint: rgba(26, 74, 122, 0.015);
	}

	:global([data-theme="analog"][data-claude-state="responding"]) {
		--amber-glow: rgba(42, 31, 24, 0.12);
		--amber-glow-strong: rgba(42, 31, 24, 0.22);
		--ambient-glow: rgba(42, 31, 24, 0.12);
		--ambient-accent: #4a3528;
		--ambient-tint: rgba(42, 31, 24, 0.02);
	}

	:global([data-theme="analog"][data-claude-state="tool_executing"]) {
		--amber-glow: rgba(106, 85, 16, 0.1);
		--amber-glow-strong: rgba(106, 85, 16, 0.2);
		--ambient-glow: rgba(106, 85, 16, 0.1);
		--ambient-accent: #6a5510;
		--ambient-tint: rgba(106, 85, 16, 0.015);
	}

	:global([data-theme="analog"][data-claude-state="active"]) {
		--amber-glow: rgba(42, 31, 24, 0.12);
		--amber-glow-strong: rgba(42, 31, 24, 0.22);
		--ambient-glow: rgba(42, 31, 24, 0.12);
		--ambient-accent: #4a3528;
		--ambient-tint: rgba(42, 31, 24, 0.015);
	}

	:global([data-theme="analog"][data-connection="error"]),
	:global([data-theme="analog"][data-connection="disconnected"]) {
		--amber-glow: rgba(42, 31, 24, 0.03);
		--amber-glow-strong: rgba(42, 31, 24, 0.06);
		--ambient-glow: rgba(42, 31, 24, 0.03);
		--ambient-tint: rgba(20, 18, 16, 0.02);
	}

	:global([data-theme="analog"][data-connection="reconnecting"]) {
		--amber-glow: rgba(106, 85, 16, 0.06);
		--amber-glow-strong: rgba(106, 85, 16, 0.12);
		--ambient-glow: rgba(106, 85, 16, 0.06);
		--ambient-tint: rgba(106, 85, 16, 0.01);
	}

	/* Analog: font family swap — serif for body, keep mono for code */
	:global([data-theme="analog"] html),
	:global([data-theme="analog"] body) {
		font-family: 'Source Serif 4', 'Georgia', 'Times New Roman', serif;
		background: var(--surface-900);
		color: var(--text-primary);
	}

	/* Ink-stained edges — like a well-used drawing pad.
	   Darker vignette, more intentional. Ink fingerprints on the margins. */
	:global([data-theme="analog"] body::before) {
		background:
			/* Main vignette — tighter, more dramatic */
			radial-gradient(ellipse at 50% 45%,
				transparent 0%,
				transparent 35%,
				rgba(100, 90, 70, 0.05) 60%,
				rgba(70, 60, 45, 0.12) 85%,
				rgba(40, 35, 25, 0.18) 100%
			),
			/* Ink blot — top left corner, like a pen was rested there */
			radial-gradient(circle at 8% 6%,
				rgba(42, 31, 24, 0.06) 0%,
				rgba(42, 31, 24, 0.02) 40%,
				transparent 60%
			),
			/* Ink blot — bottom right, accidental touch */
			radial-gradient(circle at 92% 88%,
				rgba(42, 31, 24, 0.04) 0%,
				rgba(42, 31, 24, 0.01) 30%,
				transparent 50%
			);
		transition: background 1.2s ease;
	}

	/* Paper grain is now on the elements themselves (via --grain-fine / --grain-coarse
	   background-image), so kill the fixed overlay — it doesn't scroll and flattens
	   the per-element texture. */
	:global([data-theme="analog"] body::after) {
		display: none;
	}

	/* Scrollbar — slim, understated, like a page edge */
	:global([data-theme="analog"] ::-webkit-scrollbar) {
		width: 6px;
	}

	:global([data-theme="analog"] ::-webkit-scrollbar-track) {
		background: transparent;
	}

	:global([data-theme="analog"] ::-webkit-scrollbar-thumb) {
		background: var(--surface-400);
		border-radius: 3px;
		border: none;
	}

	:global([data-theme="analog"] ::-webkit-scrollbar-thumb:hover) {
		background: var(--text-muted);
	}

	/* Selection — ink wash, like watercolor dragged across the page */
	:global([data-theme="analog"] ::selection) {
		background: rgba(42, 31, 24, 0.15);
		color: var(--text-primary);
	}

	/* File links — underline drawn with a ruling pen, ink bleeds on hover */
	:global([data-theme="analog"] .file-link) {
		color: var(--text-primary);
		border-bottom: 1.5px solid var(--amber-600);
		transition: border-color 0.2s ease, text-shadow 0.3s ease;
	}

	:global([data-theme="analog"] .file-link:hover) {
		background: transparent;
		border-bottom-width: 2px;
		border-bottom-color: var(--text-primary);
		text-shadow: 0 0 3px rgba(42, 31, 24, 0.2);
	}

	:global([data-theme="analog"] .file-link:focus) {
		background: rgba(42, 31, 24, 0.06);
		box-shadow: none;
	}

	/* ==========================================================================
	   Analog Syntax Highlighting — Ink on Paper
	   Restrained, editorial color palette
	   ========================================================================== */

	/* Code blocks: like a plate inset on the page, ruled left margin,
	   ink-bleed shadow instead of clean edges. Scrolling grain. */
	:global([data-theme="analog"] .hljs) {
		background-color: var(--surface-700);
		background-image: var(--grain-fine), var(--grain-coarse);
		background-blend-mode: multiply, multiply;
		color: #201c18;
		border-left: 3px solid var(--amber-600);
		box-shadow: inset 2px 0 4px rgba(42, 31, 24, 0.06);
	}

	/* Comments — light pencil, graphite gray. The drafter's notes. */
	:global([data-theme="analog"] .hljs-comment),
	:global([data-theme="analog"] .hljs-quote) {
		color: #8a8478;
		font-style: italic;
	}

	/* Keywords — heavy nib pressure, dense ink, the backbone of the text */
	:global([data-theme="analog"] .hljs-keyword),
	:global([data-theme="analog"] .hljs-selector-tag) {
		color: #141210;
		font-weight: 700;
	}

	:global([data-theme="analog"] .hljs-tag) {
		color: #3a2a1e;
	}

	/* Strings — Diamine Sherwood Green, classic fountain pen ink */
	:global([data-theme="analog"] .hljs-string),
	:global([data-theme="analog"] .hljs-addition) {
		color: #155e28;
	}

	:global([data-theme="analog"] .hljs-template-tag),
	:global([data-theme="analog"] .hljs-template-variable) {
		color: #105020;
	}

	/* Numbers — Diamine Ancient Copper */
	:global([data-theme="analog"] .hljs-number),
	:global([data-theme="analog"] .hljs-literal) {
		color: #8a3a08;
	}

	/* Built-ins — raw umber, earthier */
	:global([data-theme="analog"] .hljs-built_in),
	:global([data-theme="analog"] .hljs-type) {
		color: #5a3e10;
	}

	:global([data-theme="analog"] .hljs-variable) {
		color: #2a2420;
	}

	/* Attributes — Kon-peki blue-black */
	:global([data-theme="analog"] .hljs-attr) {
		color: #1a4a7a;
	}

	/* Functions — iron gall ink, heavy stroke, the important words */
	:global([data-theme="analog"] .hljs-title),
	:global([data-theme="analog"] .hljs-title.function_),
	:global([data-theme="analog"] .hljs-section) {
		color: #2a1f18;
		font-weight: 700;
	}

	/* Classes — Diamine Damson, plum with weight */
	:global([data-theme="analog"] .hljs-title.class_),
	:global([data-theme="analog"] .hljs-class .hljs-title) {
		color: #4a2050;
		font-weight: 600;
	}

	/* Regex — alizarin, like a correction mark */
	:global([data-theme="analog"] .hljs-regexp),
	:global([data-theme="analog"] .hljs-symbol) {
		color: #701830;
	}

	:global([data-theme="analog"] .hljs-deletion) {
		color: #701818;
		background: rgba(112, 24, 24, 0.08);
		text-decoration: line-through;
		text-decoration-color: rgba(112, 24, 24, 0.3);
	}

	:global([data-theme="analog"] .hljs-addition) {
		background: rgba(21, 94, 40, 0.08);
	}

	:global([data-theme="analog"] .hljs-meta) {
		color: #1a4a4a;
	}

	:global([data-theme="analog"] .hljs-meta .hljs-keyword) {
		color: #1a4a4a;
		font-weight: 700;
	}

	:global([data-theme="analog"] .hljs-meta .hljs-string) {
		color: #155e28;
	}

	/* Punctuation — nib barely touching paper, light and delicate */
	:global([data-theme="analog"] .hljs-punctuation) {
		color: #8a8478;
	}

	:global([data-theme="analog"] .hljs-operator) {
		color: #141210;
		font-weight: 500;
	}

	:global([data-theme="analog"] .hljs-property) {
		color: #3a2a1e;
	}

	:global([data-theme="analog"] .hljs-params) {
		color: #4a4540;
	}

	:global([data-theme="analog"] .hljs-strong) {
		font-weight: 800;
		color: #141210;
	}

	:global([data-theme="analog"] .hljs-emphasis) {
		font-style: italic;
		color: #2a2520;
	}

	:global([data-theme="analog"] .hljs-link) {
		color: #1a4a7a;
		text-decoration: underline;
		text-underline-offset: 2px;
	}

	:global([data-theme="analog"] .hljs-selector-class),
	:global([data-theme="analog"] .hljs-selector-id) {
		color: #2a1f18;
		font-weight: 600;
	}

	:global([data-theme="analog"] .hljs-selector-pseudo) {
		color: #4a2050;
	}

	:global([data-theme="analog"] .hljs-namespace) {
		color: #4a2050;
		opacity: 0.9;
	}

	/* Hover: ink bleed shadow, not phosphor glow */
	:global([data-theme="analog"] pre code.hljs:hover) {
		box-shadow: inset 3px 0 8px rgba(42, 31, 24, 0.08);
	}

	/* Line numbers — pencil guides, barely there */
	:global([data-theme="analog"] .code-line::before) {
		color: rgba(20, 18, 16, 0.12);
	}

	/* ==========================================================================
	   Analog Theme — Global Fallbacks
	   Typography overrides for generic HTML elements.
	   Component-specific overrides live in their own .svelte files.
	   ========================================================================== */

	/* Buttons: ink stamp impression — dark textured background with grain */
	:global([data-theme="analog"] button) {
		text-shadow: none;
	}

	/* Blockquotes: marginalia with ink wash and visible paper */
	:global([data-theme="analog"] blockquote) {
		background-color: var(--surface-800);
		background-image: var(--grain-fine), var(--ink-wash);
		background-blend-mode: multiply, normal;
		border-left: 3px solid var(--amber-600);
	}

	/* Keep monospace for code elements (global HTML, not component-scoped) */
	:global([data-theme="analog"] code),
	:global([data-theme="analog"] pre) {
		font-family: 'JetBrains Mono', 'SF Mono', 'Consolas', monospace;
	}

	/* Headings: use the display serif */
	:global([data-theme="analog"] h1),
	:global([data-theme="analog"] h2),
	:global([data-theme="analog"] h3) {
		font-family: 'Newsreader', Georgia, serif;
	}

	/* Mobile-first responsive adjustments */
	@media (max-width: 639px) {
		:global(:root) {
			--spacing-md: 10px;
			--spacing-lg: 12px;
			--spacing-xl: 16px;
		}
	}

	:global(html, body) {
		margin: 0;
		padding: 0;
		height: 100%;
		overflow: hidden;
		background: var(--surface-900);
		color: var(--text-primary);
		font-family: var(--font-body);
	}

	/* Vignette/tint overlay */
	:global(body::before) {
		content: '';
		position: fixed;
		inset: 0;
		background: var(--vignette);
		pointer-events: none;
		z-index: 9998;
		transition: background 0.8s ease;
	}

	/* Texture overlay — scanlines or paper grain */
	:global(body::after) {
		content: '';
		position: fixed;
		inset: 0;
		background: var(--texture-overlay);
		opacity: var(--texture-opacity);
		pointer-events: none;
		z-index: 9999;
		transition: opacity 0.8s ease;
	}

	/* Custom scrollbar styles - amber themed */
	:global(::-webkit-scrollbar) {
		width: 8px;
		height: 8px;
	}

	:global(::-webkit-scrollbar-track) {
		background: var(--surface-800);
	}

	:global(::-webkit-scrollbar-thumb) {
		background: var(--surface-400);
		border-radius: 4px;
		border: 1px solid var(--surface-border);
	}

	:global(::-webkit-scrollbar-thumb:hover) {
		background: var(--ambient-accent, var(--amber-600));
		border-color: var(--ambient-accent, var(--amber-500));
		transition: background 0.8s ease, border-color 0.8s ease;
	}

	/* Selection color */
	:global(::selection) {
		background: var(--tint-selection);
		color: var(--amber-300);
	}

	/* ==========================================================================
	   Syntax Highlighting - High Contrast CRT Theme
	   Distinct colors for readability while maintaining retro aesthetic
	   ========================================================================== */

	:global(.hljs) {
		background: var(--surface-700);
		color: #e8dcc8;
	}

	/* Comments - visible but subdued olive */
	:global(.hljs-comment),
	:global(.hljs-quote) {
		color: #7a8a6a;
		font-style: italic;
	}

	/* Keywords - electric cyan (stands out clearly) */
	:global(.hljs-keyword),
	:global(.hljs-selector-tag) {
		color: #5ccfe6;
		font-weight: 500;
	}

	/* HTML/XML tags - coral */
	:global(.hljs-tag) {
		color: #f29e74;
	}

	/* Strings - bright lime green */
	:global(.hljs-string),
	:global(.hljs-addition) {
		color: #a5e075;
	}

	/* Template strings */
	:global(.hljs-template-tag),
	:global(.hljs-template-variable) {
		color: #95d865;
	}

	/* Numbers, booleans - vivid orange */
	:global(.hljs-number),
	:global(.hljs-literal) {
		color: #ffae57;
	}

	/* Built-in types, primitives - gold */
	:global(.hljs-built_in),
	:global(.hljs-type) {
		color: #ffd580;
	}

	/* Variables - light amber */
	:global(.hljs-variable) {
		color: #f5d9a8;
	}

	/* Attributes - peach */
	:global(.hljs-attr) {
		color: #f0b090;
	}

	/* Functions - bright yellow (very prominent) */
	:global(.hljs-title),
	:global(.hljs-title.function_),
	:global(.hljs-section) {
		color: #ffd866;
		font-weight: 500;
	}

	/* Classes, types - soft purple */
	:global(.hljs-title.class_),
	:global(.hljs-class .hljs-title) {
		color: #d4a5ff;
	}

	/* Regex, special chars - pink/magenta */
	:global(.hljs-regexp),
	:global(.hljs-symbol) {
		color: #ff80bf;
	}

	/* Deletion - bright red */
	:global(.hljs-deletion) {
		color: #ff6b6b;
		background: rgba(255, 107, 107, 0.1);
	}

	/* Addition background */
	:global(.hljs-addition) {
		background: rgba(165, 224, 117, 0.1);
	}

	/* Meta, preprocessor - teal */
	:global(.hljs-meta) {
		color: #80cbc4;
	}

	:global(.hljs-meta .hljs-keyword) {
		color: #80cbc4;
		font-weight: 500;
	}

	:global(.hljs-meta .hljs-string) {
		color: #a5e075;
	}

	/* Punctuation - subtle but visible */
	:global(.hljs-punctuation) {
		color: #b0a090;
	}

	/* Operators - brighter for visibility */
	:global(.hljs-operator) {
		color: #ff9d6f;
	}

	/* Property names - warm tan */
	:global(.hljs-property) {
		color: #e8c090;
	}

	/* Parameters - distinct from variables */
	:global(.hljs-params) {
		color: #c4b8a8;
	}

	/* Bold and emphasis */
	:global(.hljs-strong) {
		font-weight: 700;
		color: #ffe4a0;
	}

	:global(.hljs-emphasis) {
		font-style: italic;
		color: #c8d8c8;
	}

	/* Links - blue that pops */
	:global(.hljs-link) {
		color: #73d0ff;
		text-decoration: underline;
	}

	/* Language-specific: JSON keys */
	:global(.hljs-attr) {
		color: #5ccfe6;
	}

	/* Language-specific: CSS selectors */
	:global(.hljs-selector-class),
	:global(.hljs-selector-id) {
		color: #ffd866;
	}

	:global(.hljs-selector-pseudo) {
		color: #d4a5ff;
	}

	/* Namespace */
	:global(.hljs-namespace) {
		color: #d4a5ff;
		opacity: 0.9;
	}

	/* Subtle glow on hover for interactive feel */
	:global(pre code.hljs:hover) {
		box-shadow: inset 0 0 30px rgba(251, 146, 60, 0.03);
	}

	/* ==========================================================================
	   Line Numbers
	   ========================================================================== */

	:global(code.hljs) {
		counter-reset: line;
	}

	:global(.code-line) {
		display: block;
		position: relative;
		counter-increment: line;
	}

	:global(.code-line::before) {
		content: counter(line);
		position: absolute;
		left: calc(-1 * var(--gutter-width, 2.5em) - 0.5em);
		width: var(--gutter-width, 2.5em);
		text-align: right;
		color: rgba(232, 220, 200, 0.2);
		font-size: inherit;
		line-height: inherit;
		user-select: none;
		pointer-events: none;
	}

	/* ==========================================================================
	   File Links - Interactive file path styling
	   ========================================================================== */

	:global(.file-link) {
		color: var(--amber-400);
		text-decoration: none;
		border-bottom: 1px dashed var(--amber-600);
		cursor: pointer;
		transition: all 0.15s ease;
		padding: 0 2px;
		margin: 0 -2px;
		border-radius: 2px;
	}

	:global(.file-link:hover) {
		background: rgba(251, 146, 60, 0.15);
		border-bottom-color: var(--amber-400);
		text-shadow: 0 0 8px var(--amber-glow);
	}

	:global(.file-link:focus) {
		outline: none;
		background: rgba(251, 146, 60, 0.2);
		box-shadow: 0 0 0 2px rgba(251, 146, 60, 0.3);
	}

	:global(.file-link:active) {
		background: rgba(251, 146, 60, 0.25);
	}

	/* ==========================================================================
	   Ink Transition — ruled-line drawing
	   The UI's structural lines draw themselves in like a ruling pen on
	   fresh paper. Notebook guide lines follow. Each fades to reveal the
	   real border underneath.
	   ========================================================================== */

	:global(.ink-rules-container) {
		position: fixed;
		inset: 0;
		z-index: 100000;
		pointer-events: none;
	}

	/* Base rule — positioned by JS, animated by direction class */
	:global(.ink-rule) {
		position: absolute;
		pointer-events: none;
	}

	/* Horizontal rule — pen-pressure gradient: heavier at the nib start */
	:global(.ink-rule-h) {
		height: 1.5px;
		background: linear-gradient(90deg,
			rgba(42, 31, 24, 0.5) 0%,
			rgba(42, 31, 24, 0.35) 50%,
			rgba(42, 31, 24, 0.12) 100%
		);
		transform-origin: left center;
		transform: scaleX(0) rotate(0.12deg);
		animation: rule-draw-h var(--dur, 420ms) cubic-bezier(0.25, 0.8, 0.25, 1) var(--d, 0ms) forwards;
	}

	/* Vertical rule — pressure fades toward the bottom */
	:global(.ink-rule-v) {
		width: 2px;
		background: linear-gradient(180deg,
			rgba(42, 31, 24, 0.5) 0%,
			rgba(42, 31, 24, 0.35) 50%,
			rgba(42, 31, 24, 0.12) 100%
		);
		transform-origin: center top;
		transform: scaleY(0) rotate(-0.08deg);
		animation: rule-draw-v var(--dur, 480ms) cubic-bezier(0.25, 0.8, 0.25, 1) var(--d, 0ms) forwards;
	}

	/* Light ruling lines — notebook guide lines, much fainter */
	:global(.ink-rule-light.ink-rule-h) {
		height: 1px;
		background: linear-gradient(90deg,
			rgba(42, 31, 24, 0.15) 0%,
			rgba(42, 31, 24, 0.12) 50%,
			rgba(42, 31, 24, 0.04) 100%
		);
	}

	@keyframes rule-draw-h {
		0%   { transform: scaleX(0)   rotate(0.12deg); opacity: 1; }
		55%  { transform: scaleX(1)   rotate(0.12deg); opacity: 0.8; }
		75%  { transform: scaleX(1)   rotate(0.12deg); opacity: 0.5; }
		100% { transform: scaleX(1)   rotate(0.12deg); opacity: 0; }
	}

	@keyframes rule-draw-v {
		0%   { transform: scaleY(0)   rotate(-0.08deg); opacity: 1; }
		55%  { transform: scaleY(1)   rotate(-0.08deg); opacity: 0.8; }
		75%  { transform: scaleY(1)   rotate(-0.08deg); opacity: 0.5; }
		100% { transform: scaleY(1)   rotate(-0.08deg); opacity: 0; }
	}
</style>
