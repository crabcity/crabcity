<script lang="ts">
	/**
	 * VoiceVisualizer — Phosphor decay spectrum analyzer
	 *
	 * Modeled after hardware audio gear: discrete phosphor segments stacked
	 * vertically per frequency band. Each segment is independently energized
	 * and decays at its own rate — hot white-amber when first lit, cooling
	 * through warm amber to dim orange as the phosphor dies. Segments have
	 * visible gaps, like a real segmented VU display.
	 */

	interface Props {
		/** Shared frequency data buffer — written by voice analyser at ~60fps */
		data: Uint8Array;
	}

	let { data }: Props = $props();

	let canvas: HTMLCanvasElement;
	let container: HTMLDivElement;

	const BAR_COUNT = 64;
	const BAR_GAP = 1;
	const ROWS = 12; // discrete phosphor segments per column
	const ROW_GAP = 1; // gap between segments for hardware look
	const DECAY = 0.97; // per-frame phosphor decay (slow, warm fade)

	// 2D phosphor grid: each cell has its own brightness 0–1
	const phosphor: Float32Array[] = Array.from(
		{ length: BAR_COUNT },
		() => new Float32Array(ROWS),
	);

	$effect(() => {
		if (!canvas || !container) return;
		const ctx = canvas.getContext('2d', { alpha: true })!;
		let alive = true;
		const dpr = window.devicePixelRatio || 1;

		function resize() {
			const rect = container.getBoundingClientRect();
			canvas.width = rect.width * dpr;
			canvas.height = rect.height * dpr;
			ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
		}

		resize();
		const ro = new ResizeObserver(resize);
		ro.observe(container);

		function render() {
			if (!alive) return;

			const w = canvas.width / dpr;
			const h = canvas.height / dpr;

			ctx.clearRect(0, 0, w, h);

			const binCount = data.length || 256;
			const usableBins = Math.floor(binCount * 0.4);
			const cellH = (h - (ROWS - 1) * ROW_GAP) / ROWS;

			for (let i = 0; i < BAR_COUNT; i++) {
				const x = (i / BAR_COUNT) * w;
				const xNext = ((i + 1) / BAR_COUNT) * w;
				const lineW = xNext - x - BAR_GAP;

				// Compute target level from frequency data
				const startBin = Math.floor((i / BAR_COUNT) * usableBins);
				const endBin = Math.max(startBin + 1, Math.floor(((i + 1) / BAR_COUNT) * usableBins));
				let sum = 0;
				for (let b = startBin; b < endBin && b < binCount; b++) {
					sum += data[b] ?? 0;
				}
				const target = sum / (endBin - startBin) / 255;
				const litRows = Math.floor(target * ROWS);
				const tipRow = litRows > 0 ? ROWS - litRows : -1;

				const col = phosphor[i];

				for (let r = 0; r < ROWS; r++) {
					const isLit = r >= ROWS - litRows;

					if (isLit) {
						col[r] = Math.max(col[r], 0.3 + target * 0.7);
					} else {
						col[r] *= DECAY;
					}

					const brightness = col[r];
					if (brightness < 0.01) {
						col[r] = 0;
						continue;
					}

					const cy = r * (cellH + ROW_GAP);
					const isTip = r === tipRow;

					if (isTip) {
						// Hot tip — bright white-amber leading edge
						ctx.shadowBlur = 8 + brightness * 14;
						ctx.shadowColor = `rgba(255, 180, 40, ${brightness * 0.7})`;
						ctx.fillStyle = `rgba(255, 235, 180, ${0.8 + brightness * 0.2})`;
					} else if (brightness > 0.7) {
						// Hot phosphor — bright amber-orange glow
						ctx.shadowBlur = 6 + brightness * 10;
						ctx.shadowColor = `rgba(245, 140, 20, ${brightness * 0.6})`;
						const g = Math.floor(130 + brightness * 50);
						ctx.fillStyle = `rgba(255, ${g}, 20, ${brightness})`;
					} else if (brightness > 0.3) {
						// Warm orange
						ctx.shadowBlur = 3;
						ctx.shadowColor = `rgba(200, 90, 6, ${brightness * 0.3})`;
						const g = Math.floor(70 + brightness * 60);
						ctx.fillStyle = `rgba(230, ${g}, 8, ${brightness * 0.85})`;
					} else {
						// Dying phosphor — deep orange ember
						ctx.shadowBlur = 0;
						const alpha = brightness * 2.5;
						ctx.fillStyle = `rgba(140, 45, 3, ${alpha})`;
					}

					ctx.fillRect(x, cy, lineW, cellH);
					ctx.shadowBlur = 0;
				}
			}

			requestAnimationFrame(render);
		}

		requestAnimationFrame(render);

		return () => {
			alive = false;
			ro.disconnect();
		};
	});
</script>

<div class="visualizer" bind:this={container}>
	<canvas bind:this={canvas}></canvas>
</div>

<style>
	.visualizer {
		width: 100%;
		min-width: 0;
		height: 32px;
		align-self: stretch;
	}

	canvas {
		display: block;
		width: 100%;
		height: 100%;
	}
</style>
