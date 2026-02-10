import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

export default defineConfig({
	plugins: [sveltekit()],
	server: {
		// Proxy API requests to the Rust backend during development
		proxy: {
			'/api': {
				target: 'http://localhost:3000',
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
