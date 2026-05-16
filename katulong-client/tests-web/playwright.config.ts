import { defineConfig, devices } from "@playwright/test";
import * as os from "node:os";
import * as path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));

// The tests drive `katulong-client serve`, which connects to a real
// katulong. CI / local runs need a hermetic katulong for the assertions
// to be deterministic, so this config spawns one as a sibling webServer
// (isolated tmux socket + tempdir state) and points `serve` at it via
// the standard --url / --api-key flags.
const katulongRepo = process.env.KATULONG_REPO;
if (!katulongRepo) {
  console.warn(
    "[katulong-client/tests-web] KATULONG_REPO is unset. Set it to the path of a katulong checkout (e.g. ~/Projects/dorky_robot/katulong) or webServer-dependent tests will time out."
  );
}

const repoRoot = path.resolve(__dirname, "../..");
const NOTEBOOK_PORT = parseInt(process.env.NOTEBOOK_PORT ?? "8765", 10);
const KATULONG_PORT = parseInt(process.env.KATULONG_PORT ?? "51999", 10);
const KATULONG_URL = `http://127.0.0.1:${KATULONG_PORT}`;
// `isLocalRequest` in katulong's auth middleware bypasses the API
// key check for 127.0.0.1 callers, so any non-empty string works.
const KATULONG_API_KEY = "tests-localhost";
// Per-run isolation so parallel `npx playwright test` invocations (or
// a leftover server from a previous crashed run) don't share tmux
// state.
const TMUX_SOCKET = `kc-tests-${process.pid}`;
const DATA_DIR = path.join(os.tmpdir(), `kc-tests-${process.pid}`);

export default defineConfig({
  testDir: "./tests",
  // Notebook tests run serially: they share a single katulong-client
  // serve instance + one underlying katulong. Running them in parallel
  // would clobber each other's `current` session state.
  workers: 1,
  retries: 1,
  timeout: 60_000,
  expect: { timeout: 10_000 },

  use: {
    baseURL: `http://127.0.0.1:${NOTEBOOK_PORT}`,
    trace: "on-first-retry",
    screenshot: "only-on-failure",
  },

  projects: [
    { name: "chromium", use: { ...devices["Desktop Chrome"] } },
  ],

  // Two webServers: katulong first, then `katulong-client serve`
  // pointing at it. Playwright starts them in order and waits for
  // each `url` to become reachable before running tests.
  webServer: [
    {
      command: katulongRepo ? `node "${katulongRepo}/server.js"` : "false",
      cwd: katulongRepo ?? __dirname,
      url: `${KATULONG_URL}/sessions`,
      timeout: 30_000,
      reuseExistingServer: !process.env.CI,
      stdout: "pipe",
      stderr: "pipe",
      env: {
        PORT: String(KATULONG_PORT),
        KATULONG_BIND_HOST: "127.0.0.1",
        KATULONG_DATA_DIR: DATA_DIR,
        KATULONG_TMUX_SOCKET: TMUX_SOCKET,
        LOG_LEVEL: "warn",
        NODE_ENV: "production",
      },
    },
    {
      command:
        `cargo run -p katulong-client -- ` +
        `--url ${KATULONG_URL} --api-key ${KATULONG_API_KEY} ` +
        `serve --port ${NOTEBOOK_PORT}`,
      cwd: repoRoot,
      url: `http://127.0.0.1:${NOTEBOOK_PORT}/api/state`,
      timeout: 120_000,
      reuseExistingServer: !process.env.CI,
      stdout: "pipe",
      stderr: "pipe",
    },
  ],
});
