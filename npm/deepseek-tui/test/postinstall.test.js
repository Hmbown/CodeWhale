const assert = require("node:assert/strict");
const test = require("node:test");

const pkg = require("../package.json");
const { _internal } = require("../scripts/install");

test("postinstall opts into optional install mode", () => {
  assert.equal(pkg.scripts.postinstall, "node scripts/install.js --optional");
});

test("optional install can be enabled by command-line flag or env", () => {
  assert.equal(_internal.isOptionalInstall(["--optional"], {}), true);
  assert.equal(_internal.isOptionalInstall([], {}), false);
  assert.equal(_internal.isOptionalInstall([], { DEEPSEEK_TUI_OPTIONAL_INSTALL: "1" }), true);
  assert.equal(_internal.isOptionalInstall([], { DEEPSEEK_OPTIONAL_INSTALL: "1" }), true);
});

test("optional mode only changes install-time defaults", () => {
  assert.equal(_internal.maxAttempts("install", { DEEPSEEK_TUI_OPTIONAL_INSTALL: "1" }), 1);
  assert.equal(_internal.maxAttempts("runtime", { DEEPSEEK_TUI_OPTIONAL_INSTALL: "1" }), 5);
  assert.equal(_internal.defaultTimeoutMs("install", { DEEPSEEK_TUI_OPTIONAL_INSTALL: "1" }), 15_000);
  assert.equal(_internal.defaultTimeoutMs("runtime", { DEEPSEEK_TUI_OPTIONAL_INSTALL: "1" }), 300_000);
  assert.equal(_internal.defaultStallMs("install", { DEEPSEEK_TUI_OPTIONAL_INSTALL: "1" }), 5_000);
  assert.equal(_internal.defaultStallMs("runtime", { DEEPSEEK_TUI_OPTIONAL_INSTALL: "1" }), 30_000);
});

test("pnpm optional postinstall skips install-time download", () => {
  assert.equal(
    _internal.shouldSkipOptionalPostinstall("install", ["--optional"], {
      npm_config_user_agent: "pnpm/10.11.0 npm/? node/v22.15.0 win32 x64",
    }),
    true,
  );
  assert.equal(
    _internal.shouldSkipOptionalPostinstall("runtime", ["--optional"], {
      npm_config_user_agent: "pnpm/10.11.0 npm/? node/v22.15.0 win32 x64",
    }),
    false,
  );
  assert.equal(
    _internal.shouldSkipOptionalPostinstall("install", [], {
      npm_config_user_agent: "pnpm/10.11.0 npm/? node/v22.15.0 win32 x64",
    }),
    false,
  );
  assert.equal(
    _internal.shouldSkipOptionalPostinstall("install", ["--optional"], {
      npm_config_user_agent: "npm/11.3.0 node/v22.15.0 win32 x64",
    }),
    false,
  );
});

test("optional install only swallows retryable download failures", () => {
  const socketHangUp = new Error("socket hang up");
  assert.equal(
    _internal.shouldIgnoreInstallFailure("install", socketHangUp, ["--optional"], {}),
    true,
  );

  const timedOut = new Error("download exceeded total timeout of 15000 ms");
  timedOut.code = "EDOWNLOADTIMEOUT";
  assert.equal(
    _internal.shouldIgnoreInstallFailure("install", timedOut, ["--optional"], {}),
    true,
  );

  const unsupported = new Error("Unsupported platform: freebsd");
  assert.equal(
    _internal.shouldIgnoreInstallFailure("install", unsupported, ["--optional"], {}),
    false,
  );

  const badChecksum = new Error("Checksum mismatch for deepseek-linux-x64");
  badChecksum.nonRetryable = true;
  assert.equal(
    _internal.shouldIgnoreInstallFailure("install", badChecksum, ["--optional"], {}),
    false,
  );

  const glibc = new Error("requires glibc 2.34 or newer");
  glibc.nonRetryable = true;
  assert.equal(
    _internal.shouldIgnoreInstallFailure("install", glibc, ["--optional"], {}),
    false,
  );
});

test("optional install still swallows wrapped http 5xx failures", async () => {
  const previous = process.env.DEEPSEEK_TUI_OPTIONAL_INSTALL;
  process.env.DEEPSEEK_TUI_OPTIONAL_INSTALL = "1";
  const http5xx = new Error("Request failed with status 502: https://example.invalid");
  http5xx.name = "HttpStatusError";
  http5xx.status = 502;

  try {
    await assert.rejects(
      _internal.withRetry("fetch https://example.invalid", async () => {
        throw http5xx;
      }, "install"),
      (wrapped) => {
        assert.equal(wrapped.name, "HttpStatusError");
        assert.equal(wrapped.status, 502);
        assert.equal(
          _internal.shouldIgnoreInstallFailure("install", wrapped, ["--optional"], {}),
          true,
        );
        return true;
      },
    );
  } finally {
    if (previous === undefined) {
      delete process.env.DEEPSEEK_TUI_OPTIONAL_INSTALL;
    } else {
      process.env.DEEPSEEK_TUI_OPTIONAL_INSTALL = previous;
    }
  }
});

test("withRetry prints install hint on first retryable failure", async () => {
  const previousWrite = process.stderr.write;
  const previousSetTimeout = global.setTimeout;
  let stderr = "";
  let attempts = 0;
  process.stderr.write = (chunk) => {
    stderr += String(chunk);
    return true;
  };
  global.setTimeout = (callback) => {
    callback();
    return 0;
  };

  try {
    const result = await _internal.withRetry(
      "fetch https://github.com/example",
      async () => {
        attempts += 1;
        if (attempts === 1) {
          const err = new Error("connect ETIMEDOUT 20.205.243.166:443");
          err.code = "ETIMEDOUT";
          throw err;
        }
        return "ok";
      },
      "runtime",
    );

    assert.equal(result, "ok");
    assert.equal(attempts, 2);
    assert.match(stderr, /deepseek-tui install hint:/);
    assert.match(stderr, /#npm-binary-download-times-out/);
  } finally {
    process.stderr.write = previousWrite;
    global.setTimeout = previousSetTimeout;
  }
});
