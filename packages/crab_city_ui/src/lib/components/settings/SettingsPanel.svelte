<script lang="ts">
	import { userSettings, updateSetting, toggleTheme, DEFAULT_SETTINGS, type UserSettings } from '$lib/stores/settings';
	import { connectionStatus } from '$lib/stores/websocket';

	interface Props {
		embedded?: boolean;
		onback?: () => void;
	}

	let { embedded = false, onback }: Props = $props();

	function handleThemeToggle() {
		toggleTheme();
	}

	function handleDiffEngineChange(e: Event) {
		const value = (e.target as HTMLSelectElement).value as UserSettings['diffEngine'];
		updateSetting('diffEngine', value);
	}

	function handleDefaultCommandChange(e: Event) {
		const value = (e.target as HTMLInputElement).value;
		updateSetting('defaultCommand', value);
	}

	function handleFontSizeChange(e: Event) {
		const value = Number((e.target as HTMLInputElement).value);
		updateSetting('terminalFontSize', value);
	}

	function handleNotificationsToggle() {
		updateSetting('showNotifications', !$userSettings.showNotifications);
	}
</script>

<div class="settings-panel" class:embedded>
	<div class="settings-scroll">
		<div class="settings-header">
			{#if onback}
				<button class="back-btn" onclick={onback} title="Back" aria-label="Back">
					<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5">
						<path d="M10 3L5 8l5 5" />
					</svg>
				</button>
			{/if}
			<h1 class="settings-title">SETTINGS</h1>
		</div>

		<!-- Appearance -->
		<section class="settings-section">
			<h2 class="section-header">APPEARANCE</h2>

			<div class="setting-row">
				<div class="setting-info">
					<span class="setting-label">Theme</span>
					<span class="setting-desc">Visual style for the interface</span>
				</div>
				<button
					class="toggle-btn"
					class:active={$userSettings.theme === 'analog'}
					onclick={handleThemeToggle}
					aria-label="Toggle theme"
				>
					<span class="toggle-option" class:selected={$userSettings.theme === 'phosphor'}>Phosphor</span>
					<span class="toggle-divider">/</span>
					<span class="toggle-option" class:selected={$userSettings.theme === 'analog'}>Analog</span>
				</button>
			</div>

			<div class="setting-row">
				<div class="setting-info">
					<label class="setting-label" for="font-size">Terminal Font Size</label>
					<span class="setting-desc">{$userSettings.terminalFontSize}px</span>
				</div>
				<input
					id="font-size"
					type="range"
					min="12"
					max="24"
					step="1"
					value={$userSettings.terminalFontSize}
					oninput={handleFontSizeChange}
					class="range-input"
				/>
			</div>
		</section>

		<!-- Editor -->
		<section class="settings-section">
			<h2 class="section-header">EDITOR</h2>

			<div class="setting-row">
				<div class="setting-info">
					<label class="setting-label" for="diff-engine">Diff Engine</label>
					<span class="setting-desc">Algorithm for displaying code diffs</span>
				</div>
				<select
					id="diff-engine"
					class="setting-select"
					value={$userSettings.diffEngine}
					onchange={handleDiffEngineChange}
				>
					<option value="standard">Standard</option>
					<option value="patience">Patience</option>
					<option value="structural">Structural</option>
				</select>
			</div>
		</section>

		<!-- Terminal -->
		<section class="settings-section">
			<h2 class="section-header">TERMINAL</h2>

			<div class="setting-row">
				<div class="setting-info">
					<label class="setting-label" for="default-command">Default Command</label>
					<span class="setting-desc">Command used for new instances</span>
				</div>
				<input
					id="default-command"
					type="text"
					class="setting-input"
					value={$userSettings.defaultCommand}
					onchange={handleDefaultCommandChange}
					placeholder={DEFAULT_SETTINGS.defaultCommand}
				/>
			</div>

			<div class="setting-row">
				<div class="setting-info">
					<span class="setting-label">Notifications</span>
					<span class="setting-desc">Browser alerts when instances are ready</span>
				</div>
				<button
					class="indicator-btn"
					class:on={$userSettings.showNotifications}
					onclick={handleNotificationsToggle}
					aria-label="Toggle notifications"
				>
					<span class="indicator-dot"></span>
					<span class="indicator-label">{$userSettings.showNotifications ? 'ON' : 'OFF'}</span>
				</button>
			</div>
		</section>

		<!-- About -->
		<section class="settings-section">
			<h2 class="section-header">ABOUT</h2>

			<div class="about-row">
				<span class="about-key">Connection</span>
				<span class="about-value status-{$connectionStatus}">{$connectionStatus}</span>
			</div>
		</section>
	</div>
</div>

<style>
	.settings-panel {
		width: 100%;
		height: 100%;
		overflow: hidden;
		display: flex;
		flex-direction: column;
		background: var(--surface-800);
	}

	.settings-panel.embedded {
		border: none;
	}

	.settings-scroll {
		flex: 1;
		overflow-y: auto;
		padding: 24px;
		max-width: 560px;
	}

	.settings-scroll::-webkit-scrollbar {
		width: 4px;
	}

	.settings-scroll::-webkit-scrollbar-thumb {
		background: var(--surface-border);
		border-radius: 2px;
	}

	.settings-header {
		display: flex;
		align-items: center;
		gap: 8px;
		margin-bottom: 24px;
	}

	.back-btn {
		display: flex;
		align-items: center;
		justify-content: center;
		width: 24px;
		height: 24px;
		background: transparent;
		border: 1px solid var(--surface-border);
		border-radius: 4px;
		color: var(--text-muted);
		cursor: pointer;
		padding: 0;
		flex-shrink: 0;
		transition: all 0.15s ease;
	}

	.back-btn:hover {
		background: var(--tint-hover);
		border-color: var(--amber-600);
		color: var(--text-secondary);
	}

	.back-btn svg {
		width: 14px;
		height: 14px;
	}

	.settings-title {
		font-size: 14px;
		font-weight: 700;
		letter-spacing: 0.15em;
		color: var(--text-primary);
		margin: 0;
	}

	.settings-section {
		margin-bottom: 24px;
	}

	.section-header {
		font-size: 10px;
		font-weight: 700;
		letter-spacing: 0.1em;
		color: var(--amber-500);
		margin: 0 0 12px 0;
		padding-bottom: 6px;
		border-bottom: 1px solid var(--surface-border);
	}

	.setting-row {
		display: flex;
		align-items: center;
		justify-content: space-between;
		padding: 8px 0;
		gap: 16px;
	}

	.setting-info {
		display: flex;
		flex-direction: column;
		gap: 2px;
		min-width: 0;
	}

	.setting-label {
		font-size: 12px;
		font-weight: 600;
		color: var(--text-secondary);
		letter-spacing: 0.03em;
	}

	.setting-desc {
		font-size: 10px;
		color: var(--text-muted);
		letter-spacing: 0.02em;
	}

	/* Theme toggle button */
	.toggle-btn {
		display: flex;
		align-items: center;
		gap: 4px;
		background: var(--surface-700);
		border: 1px solid var(--surface-border);
		border-radius: 4px;
		padding: 4px 8px;
		cursor: pointer;
		font-family: inherit;
		font-size: 10px;
		letter-spacing: 0.05em;
		transition: border-color 0.15s ease;
	}

	.toggle-btn:hover {
		border-color: var(--amber-600);
	}

	.toggle-option {
		color: var(--text-muted);
		transition: color 0.15s ease;
	}

	.toggle-option.selected {
		color: var(--amber-400);
		font-weight: 700;
	}

	.toggle-divider {
		color: var(--text-muted);
		opacity: 0.3;
	}

	/* Range slider */
	.range-input {
		width: 100px;
		height: 4px;
		appearance: none;
		-webkit-appearance: none;
		background: var(--surface-600);
		border-radius: 2px;
		outline: none;
		cursor: pointer;
	}

	.range-input::-webkit-slider-thumb {
		-webkit-appearance: none;
		width: 12px;
		height: 12px;
		border-radius: 50%;
		background: var(--amber-500);
		border: none;
		cursor: pointer;
	}

	.range-input::-moz-range-thumb {
		width: 12px;
		height: 12px;
		border-radius: 50%;
		background: var(--amber-500);
		border: none;
		cursor: pointer;
	}

	/* Select */
	.setting-select {
		font-size: 11px;
		font-weight: 600;
		font-family: inherit;
		color: var(--text-secondary);
		background: var(--surface-700);
		border: 1px solid var(--surface-border);
		border-radius: 4px;
		padding: 4px 8px;
		cursor: pointer;
		outline: none;
	}

	.setting-select:hover {
		border-color: var(--amber-600);
	}

	.setting-select:focus {
		border-color: var(--amber-500);
	}

	.setting-select option {
		background: var(--surface-600);
		color: var(--text-primary);
	}

	/* Text input */
	.setting-input {
		font-size: 11px;
		font-family: inherit;
		color: var(--text-secondary);
		background: var(--surface-700);
		border: 1px solid var(--surface-border);
		border-radius: 4px;
		padding: 4px 8px;
		width: 140px;
		outline: none;
	}

	.setting-input:hover {
		border-color: var(--amber-600);
	}

	.setting-input:focus {
		border-color: var(--amber-500);
		color: var(--text-primary);
	}

	.setting-input::placeholder {
		color: var(--text-muted);
		opacity: 0.5;
	}

	/* Indicator toggle (ON/OFF) */
	.indicator-btn {
		display: flex;
		align-items: center;
		gap: 6px;
		background: var(--surface-700);
		border: 1px solid var(--surface-border);
		border-radius: 4px;
		padding: 4px 8px;
		cursor: pointer;
		font-family: inherit;
		transition: border-color 0.15s ease;
	}

	.indicator-btn:hover {
		border-color: var(--amber-600);
	}

	.indicator-dot {
		width: 6px;
		height: 6px;
		border-radius: 50%;
		background: var(--text-muted);
		opacity: 0.3;
		transition: all 0.15s ease;
	}

	.indicator-btn.on .indicator-dot {
		background: var(--amber-500);
		opacity: 1;
		box-shadow: 0 0 4px var(--amber-glow);
	}

	.indicator-label {
		font-size: 10px;
		font-weight: 700;
		letter-spacing: 0.08em;
		color: var(--text-muted);
	}

	.indicator-btn.on .indicator-label {
		color: var(--amber-400);
	}

	/* About section */
	.about-row {
		display: flex;
		align-items: center;
		justify-content: space-between;
		padding: 4px 0;
	}

	.about-key {
		font-size: 11px;
		color: var(--text-muted);
		letter-spacing: 0.03em;
	}

	.about-value {
		font-size: 11px;
		font-weight: 600;
		color: var(--text-secondary);
		letter-spacing: 0.03em;
	}

	.about-value.status-connected {
		color: var(--status-green);
	}

	.about-value.status-error,
	.about-value.status-server_gone {
		color: var(--status-red);
	}

	.about-value.status-reconnecting,
	.about-value.status-connecting {
		color: var(--amber-400);
	}
</style>
