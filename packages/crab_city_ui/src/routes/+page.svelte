<script lang="ts">
	import Sidebar from '$lib/components/Sidebar.svelte';
	import MainView from '$lib/components/MainView.svelte';
	import FileViewer from '$lib/components/FileViewer.svelte';
	import FileExplorer from '$lib/components/FileExplorer.svelte';
	import ChatPanel from '$lib/components/ChatPanel.svelte';
	import TaskPanel from '$lib/components/TaskPanel.svelte';
	import MemberPanel from '$lib/components/MemberPanel.svelte';
	import BootSequence from '$lib/components/BootSequence.svelte';
	import ChannelChange from '$lib/components/ChannelChange.svelte';
	import { sidebarOpen, closeSidebar, isDesktop } from '$lib/stores/ui';
	import { toggleExplorer, isExplorerOpen, closeExplorer } from '$lib/stores/files';
	import { isChatOpen, closeChat, toggleChat, composeOpen, closeCompose, selectionMode, exitSelectionMode } from '$lib/stores/chat';
	import { isTaskPanelOpen, closeTaskPanel, toggleTaskPanel } from '$lib/stores/tasks';
	import { isMemberPanelOpen, closeMemberPanel, toggleMemberPanel } from '$lib/stores/members';
	import { isFileViewerOpen, closeFileViewer } from '$lib/stores/files';
	import { claudeState } from '$lib/stores/claude';
	import { connectionStatus } from '$lib/stores/websocket';
	import { instanceList, selectInstance, showTerminal, setTerminalMode } from '$lib/stores/instances';
	import { toggleTheme } from '$lib/stores/settings';

	let showBoot = $state(true);

	// Map Claude state to data attribute for ambient CSS
	const claudeStateAttr = $derived(
		$claudeState.type === 'Thinking' ? 'thinking' :
		$claudeState.type === 'Responding' ? 'responding' :
		$claudeState.type === 'ToolExecuting' ? 'tool_executing' :
		'idle'
	);

	// Bind data-claude-state to document.body for global CSS ambient shifts
	$effect(() => {
		document.body.setAttribute('data-claude-state', claudeStateAttr);
	});

	// Bind connection status for signal-lost visual effects
	$effect(() => {
		document.body.setAttribute('data-connection', $connectionStatus);
	});



	function handleOverlayClick() {
		closeSidebar();
	}

	function handleKeydown(e: KeyboardEvent) {
		// Cmd/Ctrl+Shift+L toggles theme
		if ((e.metaKey || e.ctrlKey) && e.shiftKey && e.key === 'L') {
			e.preventDefault();
			toggleTheme();
			return;
		}

		// Escape closes fileViewer → compose → selection → members → chat → tasks → explorer → sidebar (priority order)
		if (e.key === 'Escape') {
			if ($isFileViewerOpen) {
				closeFileViewer();
			} else if ($composeOpen) {
				closeCompose();
			} else if ($selectionMode) {
				exitSelectionMode();
			} else if ($isMemberPanelOpen) {
				closeMemberPanel();
			} else if ($isChatOpen) {
				closeChat();
			} else if ($isTaskPanelOpen) {
				closeTaskPanel();
			} else if ($isExplorerOpen) {
				closeExplorer();
			} else if ($sidebarOpen) {
				closeSidebar();
			}
		}
		// Cmd/Ctrl+E toggles file explorer
		if ((e.metaKey || e.ctrlKey) && e.key === 'e') {
			e.preventDefault();
			toggleExplorer();
		}

		// --- Non-input shortcuts (disabled when typing or with modifiers) ---
		const tag = (document.activeElement?.tagName ?? '').toLowerCase();
		if (tag === 'input' || tag === 'textarea') return;
		if (e.metaKey || e.ctrlKey || e.altKey) return;

		// / to focus message input
		if (e.key === '/') {
			e.preventDefault();
			const textarea = document.querySelector('textarea');
			textarea?.focus();
		}
		// f to toggle file explorer
		if (e.key === 'f') {
			e.preventDefault();
			toggleExplorer();
		}
		// t to toggle terminal
		if (e.key === 't') {
			e.preventDefault();
			setTerminalMode(!$showTerminal);
		}
		// c to toggle chat (closes members — same right-side slot)
		if (e.key === 'c') {
			e.preventDefault();
			if ($isMemberPanelOpen) closeMemberPanel();
			toggleChat();
		}
		// q to toggle task panel
		if (e.key === 'q') {
			e.preventDefault();
			toggleTaskPanel();
		}
		// m to toggle members panel (closes chat — same right-side slot)
		if (e.key === 'm') {
			e.preventDefault();
			if ($isChatOpen) closeChat();
			toggleMemberPanel();
		}
		// 1-9 to switch instances
		const num = parseInt(e.key);
		if (num >= 1 && num <= 9) {
			const list = $instanceList;
			if (num <= list.length) {
				e.preventDefault();
				selectInstance(list[num - 1].id);
			}
		}
	}
</script>

<svelte:window onkeydown={handleKeydown} />

<div class="app-container">
	<!-- Mobile overlay -->
	{#if $sidebarOpen && !$isDesktop}
		<!-- svelte-ignore a11y_click_events_have_key_events -->
		<!-- svelte-ignore a11y_no_static_element_interactions -->
		<div class="sidebar-overlay" onclick={handleOverlayClick}></div>
	{/if}

	<!-- Sidebar: visible on desktop, slide-out on mobile -->
	<div class="sidebar-wrapper" class:open={$sidebarOpen} class:desktop={$isDesktop}>
		<Sidebar />
	</div>

	<MainView />

	<!-- File explorer panel -->
	<FileExplorer />

	<!-- Chat panel -->
	<ChatPanel />

	<!-- Task panel (left slide-out) -->
	<TaskPanel />

	<!-- Members panel (right slide-out) -->
	<MemberPanel visible={$isMemberPanelOpen} />

	<!-- Global file viewer overlay -->
	<FileViewer />
</div>

<ChannelChange />

{#if showBoot}
	<BootSequence onComplete={() => showBoot = false} wsConnected={$connectionStatus === 'connected'} />
{/if}

<style>
	.app-container {
		display: flex;
		height: 100vh;
		height: 100dvh; /* Dynamic viewport height for mobile browsers */
		width: 100vw;
		overflow: hidden;
		position: relative;
	}

	/* Mobile overlay when sidebar is open */
	.sidebar-overlay {
		position: fixed;
		inset: 0;
		background: rgba(0, 0, 0, 0.7);
		backdrop-filter: blur(2px);
		z-index: 40;
		animation: fade-in 0.2s ease;
	}

	@keyframes fade-in {
		from { opacity: 0; }
		to { opacity: 1; }
	}

	/* Sidebar wrapper - handles responsive positioning */
	.sidebar-wrapper {
		position: fixed;
		top: 0;
		left: 0;
		bottom: 0;
		width: var(--sidebar-width);
		z-index: 50;
		transform: translateX(-100%);
		transition: transform 0.25s cubic-bezier(0.4, 0, 0.2, 1);
		will-change: transform;
	}

	.sidebar-wrapper.open {
		transform: translateX(0);
	}

	/* Desktop: sidebar is always visible and in flow */
	.sidebar-wrapper.desktop {
		position: relative;
		transform: none;
		flex-shrink: 0;
	}

	/* Responsive breakpoints */
	@media (min-width: 1024px) {
		.sidebar-wrapper {
			position: relative;
			transform: none;
			flex-shrink: 0;
		}

		.sidebar-overlay {
			display: none;
		}
	}
</style>
