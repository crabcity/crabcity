import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';
import { readFileSync } from 'fs';
import { join, dirname } from 'path';
import { fileURLToPath } from 'url';

const __dirname = dirname(fileURLToPath(import.meta.url));
// BUILD_WORKSPACE_DIRECTORY is set by `bazel run`; fall back to relative __dirname
const projectRoot = process.env.BUILD_WORKSPACE_DIRECTORY ?? join(__dirname, '..', '..');

function getDataDir(): string {
	return process.env.CRAB_CITY_DATA_DIR ?? join(projectRoot, 'local', 'state', 'dev');
}

function getDaemonPort(): string {
	const dataDir = getDataDir();
	// Server writes to <data_dir>/state/daemon.port
	const candidates = [
		join(dataDir, 'state', 'daemon.port'),
		join(dataDir, 'daemon.port'),
	];
	for (const portFile of candidates) {
		try {
			return readFileSync(portFile, 'utf-8').trim();
		} catch {
			// try next
		}
	}
	return '3000';
}

export default defineConfig({
	plugins: [sveltekit()],
	server: {
		// Proxy API requests to the Rust backend during development
		proxy: {
			'/api': {
				target: `http://localhost:${getDaemonPort()}`,
				changeOrigin: true,
				ws: true
			}
		},
		// Allow Bazel runfiles tree access
		fs: {
			strict: false
		}
	}
});
