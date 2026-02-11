const path = require('path');
const fs = require('fs');

// Under Bazel, rootDir defaults to the runfiles tree (where the config lives),
// but compiled files live in the bin directory. V8 coverage resolves symlinks
// to their real paths, so file URLs point to the real execroot bin dir — not the
// runfiles tree. Without this override, Jest's rootDir filter in
// getAllV8CoverageInfoCopy() silently drops all V8 coverage data.
//
// The jest_test target uses tags=["local"] to avoid sandboxing, which allows
// rootDir to point to the real bin dir path.
let bazelBinDir;
if (process.env.JS_BINARY__EXECROOT && process.env.JS_BINARY__BINDIR && process.env.JS_BINARY__PACKAGE) {
	bazelBinDir = path.resolve(
		process.env.JS_BINARY__EXECROOT, process.env.JS_BINARY__BINDIR, process.env.JS_BINARY__PACKAGE
	);
}

// Pre-resolve reporters so they work regardless of rootDir.
// (auto_configure_reporters=False in BUILD.bazel)
let jestJunitPath;
try { jestJunitPath = require.resolve('jest-junit'); } catch { /* not available outside Bazel */ }

/** @type {import('jest').Config} */
module.exports = {
	testEnvironment: 'node',
	testMatch: ['**/dist-test/**/*.test.js'],
	transform: {},
	coverageProvider: 'v8',
	moduleFileExtensions: ['js', 'mjs', 'json'],
	snapshotFormat: { escapeString: true, printBasicPrototype: true },
	collectCoverageFrom: ['**/dist-test/**/*.js', '!**/*.test.js'],
	coveragePathIgnorePatterns: ['/node_modules/', '\\.runfiles/'],
	coverageThreshold: {
		global: { branches: 80, functions: 80, lines: 80, statements: 80 }
	},
	clearMocks: true,
	verbose: true,
	injectGlobals: true,
	...(bazelBinDir ? {
		rootDir: bazelBinDir,
		reporters: [
			'default',
			...(jestJunitPath ? [[jestJunitPath, { outputFile: process.env.XML_OUTPUT_FILE || 'jest-junit.xml' }]] : []),
		],
	} : {})
};
