import { defineConfig, devices } from "@playwright/test";
import * as path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));

// The web tests drive `katulong-client serve`, which itself spawns
// a katulong subprocess. The test harness has to know where
// katulong's `server.js` lives — same env var the Rust harness
// uses. If unset, all webServer-dependent tests will fail; print a
// clear hint instead of mysteriously timing out.
const katulongRepo = process.env.KATULONG_REPO;
if (!katulongRepo) {
  console.warn(
    "[katulong-client/tests-web] KATULONG_REPO is unset. Set it to the path of a katulong checkout (e.g. ~/Projects/dorky_robot/katulong) or webServer-dependent tests will time out."
  );
}

const repoRoot = path.resolve(__dirname, "../..");
const PORT = parseInt(process.env.NOTEBOOK_PORT ?? "8765", 10);

export default defineConfig({
  testDir: "./tests",
  // Notebook tests run serially: they share a single
  // `katulong-client serve` instance and inside it a single
  // katulong subprocess. Running them in parallel would mean
  // hitting the same /api/* endpoints concurrently and clobbering
  // each other's `current` session state.
  workers: 1,
  // Single retry catches the occasional Chromium startup flake on
  // a busy machine without masking real bugs.
  retries: 1,
  timeout: 60_000,
  expect: { timeout: 10_000 },

  use: {
    baseURL: `http://127.0.0.1:${PORT}`,
    trace: "on-first-retry",
    screenshot: "only-on-failure",
  },

  projects: [
    { name: "chromium", use: { ...devices["Desktop Chrome"] } },
  ],

  webServer: {
    // Reuse the dev-built binary so the test loop is fast.
    // `cargo build -p katulong-client` produces it at
    // ../../target/debug/katulong-client.
    command: `cargo run -p katulong-client -- serve --port ${PORT}`,
    cwd: repoRoot,
    url: `http://127.0.0.1:${PORT}/api/state`,
    timeout: 120_000,
    reuseExistingServer: !process.env.CI,
    stdout: "pipe",
    stderr: "pipe",
    env: katulongRepo ? { KATULONG_REPO: katulongRepo } : {},
  },
});
