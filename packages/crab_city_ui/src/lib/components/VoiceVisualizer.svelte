<script lang="ts">
	/**
	 * VoiceVisualizer — CRT phosphor spectrum analyzer
	 *
	 * Canvas-based frequency visualization driven by real FFT data.
	 * Reads from a shared Uint8Array buffer mutated externally at ~60fps.
	 */

	interface Props {
		/** Shared frequency data buffer — written by voice analyser at ~60fps */
		data: Uint8Array;
	}

	let { data }: Props = $props();

	let canvas: HTMLCanvasElement;
	let container: HTMLDivElement;

	const BAR_COUNT = 48;
	const BAR_GAP = 1.5;
	const BAR_RADIUS = 1.5;

	// Smoothed values for fluid animation
	const smoothed = new Float32Array(BAR_COUNT);
	const peaks = new Float32Array(BAR_COUNT);
	const peakHold = new Float32Array(BAR_COUNT);

	$effect(() => {
		if (!canvas || !container) return;
		const ctx = canvas.getContext('2d')!;
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

			const barW = (w - BAR_GAP * (BAR_COUNT - 1)) / BAR_COUNT;
			const binCount = data.length || 256;

			for (let i = 0; i < BAR_COUNT; i++) {
				// Sqrt-scale frequency mapping: more bars for bass/voice range
				const t0 = i / BAR_COUNT;
				const t1 = (i + 1) / BAR_COUNT;
				const startBin = Math.floor(t0 * t0 * binCount);
				const endBin = Math.max(startBin + 1, Math.floor(t1 * t1 * binCount));

				// Average the bins for this bar
				let sum = 0;
				for (let b = startBin; b < endBin && b < binCount; b++) {
					sum += data[b] ?? 0;
				}
				const target = sum / (endBin - startBin) / 255;

				// Smooth: fast attack, slow release
				if (target > smoothed[i]) {
					smoothed[i] += (target - smoothed[i]) * 0.4;
				} else {
					smoothed[i] += (target - smoothed[i]) * 0.06;
				}

				const val = smoothed[i];
				const barH = Math.max(0, val * h * 0.92);
				const x = i * (barW + BAR_GAP);
				const y = h - barH;

				if (barH > 1) {
					// Glow — intensity scales with level
					ctx.shadowBlur = 6 + val * 10;
					ctx.shadowColor = `rgba(245, 158, 11, ${0.25 + val * 0.45})`;

					// Bar gradient: dark base → bright top
					const grad = ctx.createLinearGradient(x, h, x, y);
					grad.addColorStop(0, 'rgba(146, 64, 14, 0.3)');
					grad.addColorStop(0.3, 'rgba(217, 119, 6, 0.75)');
					grad.addColorStop(0.7, 'rgba(245, 158, 11, 0.95)');
					grad.addColorStop(1, 'rgba(251, 191, 36, 1)');

					ctx.fillStyle = grad;
					ctx.beginPath();
					ctx.roundRect(x, y, barW, barH, [BAR_RADIUS, BAR_RADIUS, 0, 0]);
					ctx.fill();

					ctx.shadowBlur = 0;
				}

				// Peak indicators — bright caps that hold and fall
				if (val > peaks[i]) {
					peaks[i] = val;
					peakHold[i] = 30;
				} else if (peakHold[i] > 0) {
					peakHold[i]--;
				} else {
					peaks[i] -= 0.005;
					if (peaks[i] < 0) peaks[i] = 0;
				}

				if (peaks[i] > 0.03) {
					const peakY = h - peaks[i] * h * 0.92;
					ctx.shadowBlur = 4;
					ctx.shadowColor = 'rgba(254, 243, 199, 0.5)';
					ctx.fillStyle = 'rgba(254, 243, 199, 0.8)';
					ctx.fillRect(x, peakY - 2, barW, 2);
					ctx.shadowBlur = 0;
				}
			}

			// Subtle scanlines for CRT feel
			ctx.fillStyle = 'rgba(0, 0, 0, 0.04)';
			for (let y = 0; y < h; y += 2) {
				ctx.fillRect(0, y, w, 1);
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
		height: 32px;
	}

	canvas {
		display: block;
		width: 100%;
		height: 100%;
	}
</style>
