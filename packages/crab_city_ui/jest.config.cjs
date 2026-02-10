/** @type {import('jest').Config} */
module.exports = {
	testEnvironment: 'node',
	// Use the compiled JS from ts_project
	testMatch: ['**/dist-test/**/*.test.js'],
	// No transforms - files pre-compiled by ts_project
	transform: {},
	// Module resolution
	moduleFileExtensions: ['js', 'mjs', 'json'],
	// Snapshot config
	snapshotFormat: {
		escapeString: true,
		printBasicPrototype: true
	},
	// Coverage settings
	collectCoverageFrom: ['dist-test/**/*.js', '!**/*.test.js'],
	coverageThreshold: {
		global: {
			branches: 80,
			functions: 80,
			lines: 80,
			statements: 80
		}
	},
	// Clear mocks between tests
	clearMocks: true,
	// Verbose output
	verbose: true,
	// Inject Jest globals
	injectGlobals: true
};
