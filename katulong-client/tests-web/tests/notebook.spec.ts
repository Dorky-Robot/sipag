import { test, expect, Page } from "@playwright/test";

// Helpers ────────────────────────────────────────────────────────────

async function clickPlay(page: Page, cellId: string) {
  await page
    .locator(`#cell-${cellId}`)
    .locator("button.play")
    .click();
}

async function outputOf(page: Page, cellId: string): Promise<string> {
  // Wait for the cell's output area to settle. The page sets the
  // text to "⏳ running…" while a Play click is in flight, and to
  // "—" before the first click. Either sentinel means "not yet
  // settled"; locator's auto-waiting handles the negation.
  const out = page.locator(`#out-${cellId}`);
  await expect(out).not.toHaveText("—", { timeout: 10_000 });
  await expect(out).not.toContainText("⏳ running…", { timeout: 10_000 });
  return (await out.textContent()) ?? "";
}

async function expectOk(page: Page, cellId: string) {
  await expect(page.locator(`#out-${cellId}`)).toContainText("200", {
    timeout: 10_000,
  });
  await expect(page.locator(`#out-${cellId}`)).toContainText('"ok": true', {
    timeout: 10_000,
  });
}

// Tests ──────────────────────────────────────────────────────────────

test.describe("notebook page", () => {
  // The serve subprocess is reused across tests for speed. Each
  // test starts by RESETTING server state — closes the persistent
  // attach AND kills every session on the underlying katulong — so
  // we don't accumulate towards katulong's MAX_SESSIONS=20 limit.
  test.beforeEach(async ({ request }) => {
    await request.post("/api/reset").catch(() => {
      // Server might be settling; non-fatal.
    });
  });

  test("loads with title, header meta, and seven cells", async ({ page }) => {
    await page.goto("/");
    await expect(page).toHaveTitle(/katulong-client.*notebook/);

    // Header shows katulong URL + session=none on first load.
    await expect(page.locator("#meta-katulong")).toContainText("http://");
    await expect(page.locator("#meta-session")).toHaveText("none");

    // All seven cells render.
    for (const id of [
      "create",
      "paste",
      "press",
      "wait",
      "lines",
      "sessions",
      "close",
    ]) {
      await expect(page.locator(`#cell-${id}`)).toBeVisible();
    }
  });

  test("cell 1 (create) opens a fresh session and updates header", async ({
    page,
  }) => {
    await page.goto("/");
    await clickPlay(page, "create");

    const out = await outputOf(page, "create");
    // Server returns {"name":"sipag-d-<hex>","id":"<base64-ish>"}.
    expect(out).toMatch(/sipag-d-/);
    expect(out).toContain('"id"');

    // Header session pill now shows the new session name.
    await expect(page.locator("#meta-session")).toContainText("sipag-d-");
  });

  test("paste + press + wait-for round-trip", async ({ page }) => {
    await page.goto("/");
    await clickPlay(page, "create");
    await expect(page.locator("#meta-session")).toContainText("sipag-d-", {
      timeout: 10_000,
    });

    // Set a unique sentinel so a stray session prompt can't satisfy
    // the wait by accident.
    const token = "playwright-notebook-sentinel-bravo";
    await page
      .locator("#cell-paste input[data-name='body']")
      .fill(`printf '%s\\n' '${token}'`);
    await page.locator("#cell-wait input[data-name='pattern']").fill(token);
    await page.locator("#cell-wait select[data-name='from']").selectOption("attach");
    await page.locator("#cell-wait input[data-name='timeout']").fill("8");

    await clickPlay(page, "paste");
    await expectOk(page, "paste");

    await clickPlay(page, "press");
    await expectOk(page, "press");

    await clickPlay(page, "wait");
    const waitOut = await outputOf(page, "wait");
    expect(waitOut).toContain('"matched_text"');
    expect(waitOut).toContain(token);
  });

  test("lines reflect what was pasted", async ({ page }) => {
    await page.goto("/");
    await clickPlay(page, "create");
    await expect(page.locator("#meta-session")).toContainText("sipag-d-");

    const token = "playwright-lines-sentinel-charlie";
    await page
      .locator("#cell-paste input[data-name='body']")
      .fill(`printf '%s\\n' '${token}'`);
    await clickPlay(page, "paste");
    await expectOk(page, "paste");
    await clickPlay(page, "press");
    await expectOk(page, "press");

    // Wait for the prompt to render the command + output.
    await page.locator("#cell-wait input[data-name='pattern']").fill(token);
    await page.locator("#cell-wait select[data-name='from']").selectOption("attach");
    await clickPlay(page, "wait");
    await outputOf(page, "wait");

    // Lines should now contain the sentinel.
    await page.locator("#cell-lines input[data-name='n']").fill("20");
    await clickPlay(page, "lines");
    const lines = await outputOf(page, "lines");
    expect(lines).toContain(token);
  });

  test("create + attach adds exactly one session (no phantom spawn)", async ({
    page,
  }) => {
    // Regression for PR #534 — attaching by NAME (which serve does)
    // must add exactly ONE session. Attaching by id-as-name would
    // make katulong spawn a phantom and the delta would be 2.
    //
    // Tests share a long-lived `serve` instance (workers=1,
    // reuseExistingServer=true), so prior tests' Create cells leave
    // sessions lying around. We assert the DELTA, not the absolute
    // count.
    // beforeEach reset cleared every session on katulong, so the
    // count starts at 0. After Create the server should have
    // exactly one session — the new sipag-d-* we just made. Two or
    // more means katulong silently spawned a phantom (the PR #534
    // regression shape).
    await page.goto("/");
    await clickPlay(page, "create");
    await expect(page.locator("#meta-session")).toContainText("sipag-d-");

    await clickPlay(page, "sessions");
    const out = await outputOf(page, "sessions");
    const idCount = (out.match(/"id"/g) || []).length;
    expect(idCount).toBe(1);
  });

  test("close terminates without hanging the page", async ({ page }) => {
    // Server-side has a 2s timeout around attach.close() to catch
    // the writer-task deadlock regression — but if the page itself
    // hangs on the fetch, we wouldn't see the 504. Use Playwright's
    // action timeout as a second tripwire.
    await page.goto("/");
    await clickPlay(page, "create");
    await expect(page.locator("#meta-session")).toContainText("sipag-d-");

    await clickPlay(page, "close");
    await expectOk(page, "close");
    await expect(page.locator("#meta-session")).toHaveText("none", {
      timeout: 5_000,
    });
  });

  test("live-view pane reflects the session's rolling buffer", async ({
    page,
  }) => {
    // The notebook page replaces the cross-origin iframe (blocked
    // by katulong's X-Frame-Options: SAMEORIGIN) with a live-view
    // pane that polls /api/lines. Asserts: after pasting + pressing
    // Enter, the live-view contains the typed content.
    await page.goto("/");
    await clickPlay(page, "create");
    await expect(page.locator("#meta-session")).toContainText("sipag-d-");

    const token = "playwright-live-view-delta";
    await page
      .locator("#cell-paste input[data-name='body']")
      .fill(`printf '%s\\n' '${token}'`);
    await clickPlay(page, "paste");
    await expectOk(page, "paste");
    await clickPlay(page, "press");
    await expectOk(page, "press");

    // Live-view polls every ~500ms; give it a bit and assert.
    await expect(page.locator("#live-view")).toContainText(token, {
      timeout: 10_000,
    });
  });
});
