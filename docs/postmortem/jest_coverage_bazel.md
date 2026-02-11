# Jest V8 Coverage Under Bazel: Investigation & Fix

## Problem Statement

Running `bazel coverage //packages/crab_city_ui:unit_tests` produced **0% coverage
across all metrics** despite all 156 tests passing. Running the same test binary
manually from the execroot produced 100% coverage. The coverage data was being
silently discarded.

## Background: How Jest Runs Under Bazel

Understanding the fix requires understanding three layers of indirection between
`bazel coverage` and Jest's V8 coverage collection.

### Bazel's Filesystem Layout

Bazel does not run tests from the source tree. It constructs an isolated
filesystem for each test invocation:

```
execroot/
  _main/
    packages/crab_city_ui/          # symlinks to source tree (source files)
    bazel-out/
      darwin_arm64-fastbuild/
        bin/
          packages/crab_city_ui/
            dist-test/              # ts_project compiled output (.js, .js.map)
            unit_tests_/
              unit_tests            # the test binary (shell wrapper)
              unit_tests.runfiles/
                _main/
                  packages/crab_city_ui/
                    dist-test/
                      fileLinkMatch.js      -> ../../../../dist-test/fileLinkMatch.js  (symlink to bin dir)
                      fileLinkMatch.js.map  -> ../../../../dist-test/fileLinkMatch.js.map
                    jest.config.cjs         -> ../../../../jest.config.cjs
                    ...
```

There are three important directory trees:

| Tree | Path | Contains |
|------|------|----------|
| **Source tree** | `execroot/_main/packages/crab_city_ui/` | Original `.ts` files (symlinks to workspace) |
| **Bin dir** | `execroot/_main/bazel-out/.../bin/packages/crab_city_ui/` | `ts_project` outputs (`.js`, `.js.map`) |
| **Runfiles** | `.../unit_tests.runfiles/_main/packages/crab_city_ui/` | Symlinks to bin dir outputs |

When the **sandbox** is enabled (`darwin-sandbox` strategy), Bazel adds a fourth
layer: the entire execroot is copied/symlinked into an ephemeral sandbox
directory, adding yet another level of path indirection.

### aspect_rules_jest Architecture

The `jest_test` rule from `aspect_rules_jest` generates a wrapper config
(`unit_tests__jest.config.mjs`) that:

1. Imports the user's `jest.config.cjs` from the **runfiles** tree
2. Sets `config.collectCoverage = true` when `COVERAGE_DIR` is set (i.e. `bazel coverage`)
3. Configures `coverageReporters` to write LCOV format
4. Injects a custom **haste map module** (`_unit_tests_bazel_haste_map_module.cjs`) that
   discovers test files from a static file list rather than crawling the filesystem

The haste map module discovers files via the **runfiles** tree:

```javascript
// _unit_tests_bazel_haste_map_module.cjs, line 118
const f = join(WORKSPACE_RUNFILES, file);
```

So Jest's module resolver receives paths rooted in the runfiles directory.

### Jest V8 Coverage Pipeline

Jest's V8 coverage collection works in three stages:

**Stage 1 — Gate** (`jest-runner/build/testWorker.js:263`):

```javascript
const collectV8Coverage =
  globalConfig.collectCoverage &&
  globalConfig.coverageProvider === 'v8' &&
  typeof environment.getVmContext === 'function';
```

If all three conditions are true, Jest calls `runtime.collectV8Coverage()` which
starts the V8 inspector's precise coverage via `Profiler.startPreciseCoverage`.

**Stage 2 — Collection**: V8 records execution counts for every function and
block in every module loaded into the VM context. When the test completes, Jest
calls `Profiler.takePreciseCoverage()` which returns an array of
`{ url, functions[] }` objects — one per loaded module. The `url` is the
**resolved, real filesystem path** that V8 used to load the module.

**Stage 3 — Filtering** (`jest-runtime/build/index.js:1058-1074`):

```javascript
getAllV8CoverageInfoCopy() {
  return this._v8CoverageResult
    .filter(res => res.url.startsWith('file://'))
    .map(res => ({ ...res, url: fileURLToPath(res.url) }))
    .filter(res =>
      res.url.startsWith(this._config.rootDir) &&
      shouldInstrument(res.url, this._coverageOptions, this._config,
        [...this._v8CoverageSources.keys()])
    )
    .map(result => {
      const transformedFile = this._v8CoverageSources.get(result.url);
      return { codeTransformResult: transformedFile, result };
    });
}
```

This is where coverage data is **silently dropped**. The critical filter is:

```javascript
res.url.startsWith(this._config.rootDir)
```

## Root Cause

Under Bazel, `rootDir` and V8's file URLs point to **different directory trees**.

### The Path Mismatch

Jest's `rootDir` defaults to the directory containing the config file. The config
file lives in the **runfiles** tree:

```
rootDir = .../unit_tests.runfiles/_main/packages/crab_city_ui/
```

But Node's `require()` / `vm.SourceTextModule` resolve symlinks when loading
modules. The runfiles entries are symlinks to the **bin dir**:

```
runfiles/.../dist-test/fileLinkMatch.js  →  bin/.../dist-test/fileLinkMatch.js
```

V8 records coverage against the **resolved (real) path**:

```
V8 URL = file:///...bazel-out/darwin_arm64-fastbuild/bin/packages/crab_city_ui/dist-test/fileLinkMatch.js
```

The `startsWith` check fails because the V8 URL starts with the **bin dir** path,
not the **runfiles** path:

```
bin/.../packages/crab_city_ui/dist-test/fileLinkMatch.js
                  does NOT startWith
runfiles/_main/packages/crab_city_ui/
```

**Every single coverage entry is filtered out. Result: 0%.**

### Why It Works Outside Bazel

Without Bazel, there are no symlinks. `rootDir` is the project directory, source
files live in the project directory, and V8 URLs point to the project directory.
The `startsWith` check trivially passes.

### Why It Works When Running the Binary Manually

Running the Bazel-built binary directly from the execroot (outside of `bazel test`)
doesn't set `COVERAGE_DIR`, so the wrapper config doesn't enable coverage. When
you manually set `--coverage`, the config file is resolved from the current
working directory rather than the runfiles tree, so `rootDir` matches the bin dir.

## Investigation Timeline

### Hypothesis 1: Coverage Not Being Collected

**Test**: Added `JS_BINARY__LOG_DEBUG=1` to dump the resolved Jest config.
Confirmed `collectCoverage: true` and `coverageProvider: 'v8'` were set.

**Result**: Disproved. Coverage collection was being requested.

### Hypothesis 2: V8 Profiler Not Starting

**Test**: Created a `setupFile` that probed the V8 inspector API directly:

```javascript
const { Session } = require('inspector');
const s = new Session(); s.connect();
s.post('Profiler.getBestEffortCoverage', (err, { result }) => { ... });
```

**Result**: Got "Precise coverage has not been started" — but this was a red
herring. Setup files run *before* Jest starts V8 coverage in the test worker.
The profiler starts after setup, during test execution.

### Hypothesis 3: Worker Process Isolation

**Test**: Set `run_in_band = True` to force single-process execution (no worker
pool). If coverage was being collected in a parent process but tests ran in
child workers, coverage data would be lost.

**Result**: Still 0%. Not a worker isolation issue.

### Hypothesis 4: `NODE_V8_COVERAGE` Conflict

Bazel's coverage infrastructure sets `NODE_V8_COVERAGE` for its own coverage
collection. Jest also uses V8 coverage internally. If these conflicted, coverage
might be lost.

**Test**: Ran `bazel test` (not `bazel coverage`) with `collectCoverage: true`
hardcoded in the config, removing Bazel's `NODE_V8_COVERAGE` from the equation.

**Result**: Still 0%. Not a `NODE_V8_COVERAGE` conflict.

### Hypothesis 5: File Discovery

Maybe Jest couldn't find the compiled `.js` files to report coverage for.

**Test**: Added `collectCoverageFrom: ['**/dist-test/**/*.js', '!**/*.test.js']`
to explicitly tell Jest which files to collect coverage from.

**Result**: Still 0%. Jest could find the files but coverage data was empty.

### Breakthrough: Reading jest-runtime Source

At this point, all "is coverage being collected?" hypotheses were exhausted. The
coverage *was* being collected by V8 — the question was where it was being
**lost**. Reading `jest-runtime/build/index.js` revealed the
`getAllV8CoverageInfoCopy()` method and its `url.startsWith(this._config.rootDir)`
filter.

**Verification**: Printed `rootDir` and a sample V8 coverage URL during a test
run. They pointed to different directory trees, confirming the mismatch.

## The Fix

Three coordinated changes across three files:

### 1. Override `rootDir` to the Bin Directory (`jest.config.cjs`)

```javascript
let bazelBinDir;
if (process.env.JS_BINARY__EXECROOT &&
    process.env.JS_BINARY__BINDIR &&
    process.env.JS_BINARY__PACKAGE) {
  bazelBinDir = path.resolve(
    process.env.JS_BINARY__EXECROOT,
    process.env.JS_BINARY__BINDIR,
    process.env.JS_BINARY__PACKAGE
  );
}

module.exports = {
  ...(bazelBinDir ? { rootDir: bazelBinDir } : {})
};
```

This makes `rootDir` point to the **bin directory** where V8 sees the files,
so `url.startsWith(rootDir)` passes.

The environment variables are provided by `aspect_rules_js`:
- `JS_BINARY__EXECROOT`: absolute path to the Bazel execroot
- `JS_BINARY__BINDIR`: relative path like `bazel-out/darwin_arm64-fastbuild/bin`
- `JS_BINARY__PACKAGE`: the Bazel package, e.g. `packages/crab_city_ui`

When not running under Bazel (e.g. local development), these vars are unset and
the spread is a no-op — the config falls back to Jest's default `rootDir`.

### 2. Manually Configure Reporters (`jest.config.cjs` + `BUILD.bazel`)

Overriding `rootDir` breaks reporter resolution. Jest resolves reporter module
paths relative to `rootDir`. The `jest-junit` reporter is installed in
`node_modules` under the **runfiles** tree, not the bin directory. With `rootDir`
pointing to the bin dir, Jest can't find `jest-junit`:

```
Could not resolve a module for a custom reporter: jest-junit
```

Fix: set `auto_configure_reporters = False` in BUILD.bazel to prevent the
wrapper config from adding `jest-junit` as a string (which would be resolved
from rootDir). Instead, pre-resolve the reporter path in `jest.config.cjs`:

```javascript
let jestJunitPath;
try { jestJunitPath = require.resolve('jest-junit'); } catch {}

module.exports = {
  ...(bazelBinDir ? {
    rootDir: bazelBinDir,
    reporters: [
      'default',
      ...(jestJunitPath
        ? [[jestJunitPath, { outputFile: process.env.XML_OUTPUT_FILE || 'jest-junit.xml' }]]
        : []),
    ],
  } : {})
};
```

`require.resolve('jest-junit')` returns an **absolute path** that works
regardless of `rootDir`.

### 3. Disable Sandbox (`BUILD.bazel: tags = ["local"]`)

The `rootDir` override works when running without a sandbox (`--spawn_strategy=local`),
but fails under Bazel's default `darwin-sandbox` strategy. In the sandbox:

1. The sandbox creates a new directory tree with symlinks to the real execroot
2. `JS_BINARY__EXECROOT` points to the **sandbox** path
3. V8 resolves symlinks through the sandbox to the **real execroot** path
4. `rootDir` (sandbox path) doesn't match V8 URLs (real execroot path)

This is the same class of problem — symlink resolution causing path mismatches —
but at a different layer. There is no way to predict what V8 will resolve to
from inside the sandbox.

Fix: `tags = ["local"]` disables sandboxing for this test target. The test
runs directly in the execroot, eliminating the sandbox path layer.

```python
jest_test(
    name = "unit_tests",
    tags = ["local"],
    ...
)
```

### 4. Inline TypeScript Sources (`tsconfig.test.json`)

With coverage working, the next issue was source map resolution. The compiled
`.js.map` files reference original TypeScript sources:

```json
{ "sources": ["../src/lib/utils/fileLinkMatch.ts"] }
```

`v8-to-istanbul` resolves this relative to the `.js.map` file location (the bin
dir), yielding `<bindir>/src/lib/utils/fileLinkMatch.ts`. But `.ts` source files
only exist in the **source tree**, not the bin dir. This caused ENOENT errors
when generating the coverage report.

Fix: `"inlineSources": true` in `tsconfig.test.json` embeds the full TypeScript
source text directly into the `.js.map` file's `sourcesContent` array.
`v8-to-istanbul` reads the source from the map itself and never hits the
filesystem:

```json
{
  "compilerOptions": {
    "sourceMap": true,
    "inlineSources": true
  }
}
```

### 5. Exclude Runfiles Duplicates (`jest.config.cjs`)

With all the above in place, coverage reported each file **twice**: once from the
bin dir path (100%) and once from the runfiles path (0%). This happened because
the runfiles directory is a subdirectory of the bin dir:

```
<bindir>/packages/crab_city_ui/unit_tests_/unit_tests.runfiles/_main/packages/crab_city_ui/dist-test/
```

Since `rootDir` is `<bindir>/packages/crab_city_ui/`, both paths pass the
`startsWith(rootDir)` check. The runfiles copies were loaded by the haste map
module but never executed (the actual execution used the resolved bin dir paths),
so they reported 0%.

Fix: `coveragePathIgnorePatterns` excludes the runfiles tree:

```javascript
coveragePathIgnorePatterns: ['/node_modules/', '\\.runfiles/'],
```

## Final Configuration

### `jest.config.cjs`

```javascript
const path = require('path');

let bazelBinDir;
if (process.env.JS_BINARY__EXECROOT && process.env.JS_BINARY__BINDIR && process.env.JS_BINARY__PACKAGE) {
  bazelBinDir = path.resolve(
    process.env.JS_BINARY__EXECROOT, process.env.JS_BINARY__BINDIR, process.env.JS_BINARY__PACKAGE
  );
}

let jestJunitPath;
try { jestJunitPath = require.resolve('jest-junit'); } catch {}

module.exports = {
  testEnvironment: 'node',
  testMatch: ['**/dist-test/**/*.test.js'],
  transform: {},
  coverageProvider: 'v8',
  moduleFileExtensions: ['js', 'mjs', 'json'],
  collectCoverageFrom: ['**/dist-test/**/*.js', '!**/*.test.js'],
  coveragePathIgnorePatterns: ['/node_modules/', '\\.runfiles/'],
  coverageThreshold: {
    global: { branches: 80, functions: 80, lines: 80, statements: 80 }
  },
  ...(bazelBinDir ? {
    rootDir: bazelBinDir,
    reporters: [
      'default',
      ...(jestJunitPath ? [[jestJunitPath, { outputFile: process.env.XML_OUTPUT_FILE || 'jest-junit.xml' }]] : []),
    ],
  } : {})
};
```

### `BUILD.bazel` (jest_test section)

```python
jest_test(
    name = "unit_tests",
    config = "jest.config.cjs",
    data = [
        "package.json",
        ":node_modules/@jest/globals",
        ":node_modules/jest",
        ":node_modules/jest-cli",
        ":node_modules/jest-junit",
        ":test_lib",
    ],
    node_modules = ":node_modules",
    auto_configure_reporters = False,
    node_options = ["--experimental-vm-modules"],
    patch_node_fs = False,
    run_in_band = True,
    tags = ["local"],
)
```

### `tsconfig.test.json` (added `inlineSources`)

```json
{
  "compilerOptions": {
    "sourceMap": true,
    "inlineSources": true,
    "rootDir": "src/lib/utils",
    "outDir": "dist-test"
  }
}
```

## Result

```
File              | % Stmts | % Branch | % Funcs | % Lines
------------------|---------|----------|---------|--------
All files         |     100 |    97.41 |     100 |     100
 fileLinkMatch.ts |     100 |    85.71 |     100 |     100
 fuzzy.ts         |     100 |      100 |     100 |     100
 noise.ts         |     100 |    93.75 |     100 |     100
 virtualList.ts   |     100 |      100 |     100 |     100
 wrapLines.ts     |     100 |      100 |     100 |     100

Test Suites: 5 passed, 5 total
Tests:       156 passed, 156 total
```

Both `bazel test` and `bazel coverage` pass. Coverage reports show TypeScript
source files with line-level data. LCOV output is generated for integration
with coverage tooling.
