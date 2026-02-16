/**
 * API utility for HTTP requests.
 *
 * With keypair auth, most state flows over WebSocket. HTTP endpoints are
 * primarily used for loopback access (local CLI/TUI). This wrapper adds
 * content-type headers and basic error handling.
 */

/**
 * Fetch wrapper that:
 * - Sets Content-Type to application/json for requests with body
 * - Returns the raw Response for caller to handle
 */
export async function api(path: string, options: RequestInit = {}): Promise<Response> {
	const headers = new Headers(options.headers);

	if (options.body && !headers.has('Content-Type')) {
		headers.set('Content-Type', 'application/json');
	}

	return fetch(path, {
		...options,
		headers,
	});
}

/**
 * Convenience: GET JSON from API.
 */
export async function apiGet<T>(path: string): Promise<T> {
	const response = await api(path);
	if (!response.ok) throw new Error(`API error: ${response.status}`);
	return response.json();
}

/**
 * Convenience: POST JSON to API.
 */
export async function apiPost<T>(path: string, body?: unknown): Promise<T> {
	const response = await api(path, {
		method: 'POST',
		body: body ? JSON.stringify(body) : undefined
	});
	if (!response.ok) throw new Error(`API error: ${response.status}`);
	return response.json();
}

/**
 * Convenience: DELETE to API.
 */
export async function apiDelete(path: string): Promise<void> {
	const response = await api(path, { method: 'DELETE' });
	if (!response.ok) throw new Error(`API error: ${response.status}`);
}
