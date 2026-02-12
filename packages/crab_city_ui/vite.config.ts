import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';
import { readFileSync } from 'fs';
import { join, dirname } from 'path';
import { fileURLToPath } from 'url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const projectRoot = join(__dirname, '..', '..');

function getDaemonPort(): string {
	try {
		const portFile = join(projectRoot, 'local', 'state', 'dev', 'daemon.port');
		return readFileSync(portFile, 'utf-8').trim();
	} catch {
		return '3000';
	}
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
