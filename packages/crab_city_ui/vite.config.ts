import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';
import { readFileSync } from 'fs';
import { homedir } from 'os';
import { join } from 'path';

function getDaemonPort(): string {
	try {
		const portFile = join(homedir(), '.crabcity', 'state', 'daemon.port');
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
