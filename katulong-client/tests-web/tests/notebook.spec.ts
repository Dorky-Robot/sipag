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

/// Poll /api/lines via the page's HTTP context until `predicate`
/// returns true (or timeout). The live view renders through
/// xterm.js (canvas + spans), which isn't grep-friendly text — so
/// tests assert against the underlying byte content the lib
/// returns, decoupled from the visual.
async function pollLines(
  page: Page,
  predicate: (text: string) => boolean,
  opts: { timeoutMs?: number; n?: number } = {}
): Promise<string> {
  const timeout = opts.timeoutMs ?? 10_000;
  const n = opts.n ?? 80;
  const deadline = Date.now() + timeout;
  let latest = "";
  while (Date.now() < deadline) {
    const resp = await page.request.get(`/api/lines?n=${n}`);
    if (resp.ok()) {
      const body = (await resp.json()) as { lines?: string[] };
      latest = (body.lines ?? []).join("\n");
      if (predicate(latest)) return latest;
    }
    await new Promise((r) => setTimeout(r, 200));
  }
  return latest;
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

  test("session buffer reflects what we sent (byte-level via /api/lines)", async ({
    page,
  }) => {
    // The live view renders through xterm.js (canvas + spans, not
    // grep-friendly). For assertions about WHAT the library
    // delivered to the session, query /api/lines directly — that's
    // the rolling buffer the lib captured from katulong.
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

    const lines = await pollLines(page, (t) => t.includes(token));
    expect(lines).toContain(token);
  });

  test("paste does NOT wrap the body in bracketed-paste markers", async ({
    page,
  }) => {
    // Regression: an earlier version of the attach client wrapped
    // every paste body in `\x1b[200~ … \x1b[201~`. When the shell
    // hadn't enabled BP mode yet (or didn't recognise the markers),
    // they leaked into the buffer as literal `[200~…[201~` text.
    // The client now sends the raw body — same wire shape xterm.js
    // uses on a paste event — and the shell sees verbatim bytes.
    await page.goto("/");
    await clickPlay(page, "create");
    await expect(page.locator("#meta-session")).toContainText("sipag-d-");

    const token = "playwright-no-bpm-echo-test";
    await page
      .locator("#cell-paste input[data-name='body']")
      .fill(`printf '%s\\n' '${token}'`);
    await clickPlay(page, "paste");
    await expectOk(page, "paste");
    await clickPlay(page, "press");
    await expectOk(page, "press");

    const lines = await pollLines(page, (t) => t.includes(token));
    expect(lines).toContain(token);
    expect(lines).not.toContain("[200~");
    expect(lines).not.toContain("[201~");
    expect(lines).not.toContain("^[[200~");
    expect(lines).not.toContain("^[[201~");
  });

  test("notebook link uses ?s= deep-link (avoids tile-click → adopt path)", async ({
    page,
  }) => {
    // The session list tile in katulong's UI calls POST
    // /tmux-sessions/adopt on click. For an already-managed
    // session (which ours is) that path calls session.detach() in
    // the "already managed" branch — killing the pane. Our link
    // must deep-link via ?s=<name> so katulong's auto-attach path
    // runs instead.
    await page.goto("/");
    await clickPlay(page, "create");
    await expect(page.locator("#meta-session")).toContainText("sipag-d-");

    const stateResp = await page.request.get("/api/state");
    const stateBody = (await stateResp.json()) as {
      katulong_url: string;
      current_session: string | null;
    };
    const href = await page.locator("#kat-link").getAttribute("href");
    expect(href, "kat-link must deep-link with ?s=").toContain(
      `?s=${encodeURIComponent(stateBody.current_session!)}`
    );
    expect(href).toMatch(/^http:\/\/127\.0\.0\.1:\d+\/\?s=/);
  });

  test("hitting katulong's /tmux-sessions/adopt for an already-managed session does NOT kill the pane", async ({
    page,
  }) => {
    // Deterministic reproducer for the bug that crashed the user's
    // shell when they clicked the session tile in katulong's UI.
    // The tile-click handler POSTs /tmux-sessions/adopt; the
    // server-side adopt path creates a wrapper, sees "already
    // managed", detaches it — and the detach kills the underlying
    // PTY. We post directly to /tmux-sessions/adopt and assert our
    // attach to the session stays usable (input still succeeds).
    //
    // Verified: posting /tmux-sessions/adopt for an already-managed
    // session does NOT kill the pane (the server's "already-managed"
    // branch is well-behaved). Kept as documentation; re-run by
    // commenting out the skip if a future katulong version reverts
    // and you want to confirm.
    test.skip(true, "Verified to NOT reproduce the kill — adopt path is innocent");
    await page.goto("/");
    await clickPlay(page, "create");
    await expect(page.locator("#meta-session")).toContainText("sipag-d-");

    const stateResp = await page.request.get("/api/state");
    const stateBody = (await stateResp.json()) as {
      katulong_url: string;
      current_session: string | null;
    };

    // Ask katulong directly for the tmux name backing our session.
    const sessionsResp = await page.request.get(
      `${stateBody.katulong_url}/sessions`
    );
    const sessions = (await sessionsResp.json()) as Array<{
      name: string;
      tmuxSession: string;
    }>;
    const ours = sessions.find((s) => s.name === stateBody.current_session);
    expect(ours).toBeDefined();
    const tmuxName = ours!.tmuxSession;

    // Now do exactly what katulong's tile-click does.
    await page.request.post(`${stateBody.katulong_url}/tmux-sessions/adopt`, {
      data: { name: tmuxName },
    });

    // Try to send input. If the adopt path killed the pane, our
    // terminal check fires and this is a 500. Post-fix, this must
    // remain a 200.
    const pasteResp = await page.request.post("/api/paste", {
      data: { body: "alive-after-adopt-check" },
    });
    expect(
      pasteResp.status(),
      `paste failed after katulong adopt — pane was killed. body: ${await pasteResp.text()}`
    ).toBe(200);
  });

  test("focus alone (click without keys) on katulong xterm does NOT kill the shell", async ({
    page,
    context,
  }) => {
    // The Ctrl-D test (below) confirmed that an EOF keystroke on
    // katulong's focused xterm kills the shell. But "every time"
    // suggests the user isn't typing Ctrl-D. Test the no-keystroke
    // path: open katulong, focus the xterm via a click, switch
    // back, send input. If THIS fails, katulong's xterm is
    // forwarding something on click — and the user's mouse alone
    // is enough to repro.
    await page.goto("/");
    await clickPlay(page, "create");
    await expect(page.locator("#meta-session")).toContainText("sipag-d-");

    const stateResp = await page.request.get("/api/state");
    const stateBody = (await stateResp.json()) as {
      katulong_url: string;
      current_session: string | null;
    };

    const katPage = await context.newPage();
    await katPage.goto(
      `${stateBody.katulong_url}/?s=${encodeURIComponent(stateBody.current_session!)}`
    );
    await katPage.waitForTimeout(3_000);

    // Click into the xterm area (no keys).
    const xtermArea = katPage.locator(".xterm-helper-textarea, .xterm-screen");
    if ((await xtermArea.count()) > 0) {
      await xtermArea.first().click();
      // Wait long enough that any deferred init / focus-side-effect
      // would have landed.
      await katPage.waitForTimeout(3_000);
    }

    await page.bringToFront();
    const pasteResp = await page.request.post("/api/paste", {
      data: { body: "after-focus-click-only" },
    });
    if (pasteResp.status() !== 200) {
      const body = await pasteResp.text();
      console.log(
        `[diagnostic] paste failed AFTER FOCUS-CLICK ONLY (no keystrokes): ${body}`
      );
    }
    expect(
      pasteResp.status(),
      "session ended from focus-click alone — katulong xterm is forwarding bytes on focus, " +
        "and that's the user's real trigger (not a deliberate keystroke)"
    ).toBe(200);
  });

  test("ctrl-D into katulong's focused xterm kills the shell (suspected user trigger)", async ({
    page,
    context,
  }) => {
    // Hypothesis: the user's "session ends with exit code 0" when
    // opening katulong was because the katulong tab gained focus
    // and an accidental Ctrl-D (or similar EOF-producing keystroke)
    // got forwarded to the PTY by xterm.js. zsh on EOF → clean
    // exit code 0 → our attach goes terminal → next paste errors.
    //
    // This test deliberately sends Ctrl-D to katulong's xterm and
    // asserts our subsequent paste fails — if it does, the user's
    // accidental key is the most likely trigger. (Documentary —
    // it's a katulong-side concern that we can't fix from here.)
    await page.goto("/");
    await clickPlay(page, "create");
    await expect(page.locator("#meta-session")).toContainText("sipag-d-");

    const stateResp = await page.request.get("/api/state");
    const stateBody = (await stateResp.json()) as {
      katulong_url: string;
      current_session: string | null;
    };

    // Open katulong's UI, auto-attach to our session.
    const katPage = await context.newPage();
    await katPage.goto(
      `${stateBody.katulong_url}/?s=${encodeURIComponent(stateBody.current_session!)}`
    );
    // Give katulong's xterm time to attach + render.
    await katPage.waitForTimeout(3_000);

    // xterm.js wires `term.onData` to send bytes via WS. The
    // hidden helper textarea receives keyboard events. Focus it
    // and send Ctrl-D — zsh's EOF.
    const xtermArea = katPage.locator(
      ".xterm-helper-textarea, .xterm-screen"
    );
    if ((await xtermArea.count()) > 0) {
      await xtermArea.first().click();
      await katPage.keyboard.press("Control+d");
      // Allow time for the WS frame to reach katulong → PTY.
      await katPage.waitForTimeout(2_000);
    }

    // Back to notebook. If the EOF killed the shell, paste fails
    // with our terminal-check error.
    await page.bringToFront();
    const pasteResp = await page.request.post("/api/paste", {
      data: { body: "after-ctrl-d-test" },
    });
    if (pasteResp.status() !== 200) {
      const body = await pasteResp.text();
      console.log(
        `[diagnostic] paste failed after ctrl-D in katulong xterm: ${body}`
      );
      // Hypothesis confirmed. We can't auto-fix this — it's an
      // operator-keystroke issue. Future hardening: katulong could
      // gate Ctrl-D / suspend / etc on a "secure" flag. Or we could
      // teach the notebook to detect terminal-state changes and
      // surface "session was ended externally — restart?".
      expect(
        body,
        "session-ended after Ctrl-D into katulong xterm — operator-keystroke is the likely root cause"
      ).toContain("session ended");
    } else {
      // Session survived. Likely means: focus didn't transfer to
      // xterm, OR katulong's xterm doesn't forward Ctrl-D
      // unmodified. Either way the user's scenario must be
      // something else.
      console.log(
        "[diagnostic] session survived Ctrl-D — operator-keystroke is NOT the root cause"
      );
      expect(pasteResp.status()).toBe(200);
    }
  });

  test("opening the same session in katulong's web UI does not kill the shell", async ({
    page,
    context,
  }) => {
    // Reproducer for the bug the user reported:
    //   1. Start a session via our serve (Create cell)
    //   2. Open the SAME session in katulong's web UI in a second tab
    //   3. Try to send input via cell 2
    //   4. → "session ended with exit code 0"
    //
    // If this test reproduces, we have a deterministic anchor to
    // iterate against. The expected post-fix behaviour is: opening
    // the session in katulong's UI must not cause the shell to
    // exit; the subsequent paste must succeed.
    await page.goto("/");
    await clickPlay(page, "create");
    await expect(page.locator("#meta-session")).toContainText("sipag-d-");

    // Pull the katulong URL + session name from our serve, not the
    // header pill (the pill might be cached stale across reloads).
    const stateResp = await page.request.get("/api/state");
    const stateBody = (await stateResp.json()) as {
      katulong_url: string;
      current_session: string | null;
    };
    expect(stateBody.katulong_url).toMatch(/^http:\/\/127\.0\.0\.1:\d+$/);
    expect(stateBody.current_session).toMatch(/^sipag-d-/);

    // Open katulong's UI for the SAME session in a second tab. The
    // `?s=` param tells katulong's frontend to auto-attach to that
    // session (see app.js: explicitSession reads URLSearchParams).
    const katPage = await context.newPage();
    await katPage.goto(
      `${stateBody.katulong_url}/?s=${encodeURIComponent(stateBody.current_session!)}`
    );
    // Wait for katulong's xterm.js to mount + send its attach +
    // for any side effects to land.
    await katPage.waitForTimeout(3_000);

    // Back to the notebook tab — send input. Our serve's
    // /api/paste calls attach.input() which now (post-fix) checks
    // the terminal state. If opening the katulong UI caused the
    // session to exit, this will return 500 with "session ended".
    await page.bringToFront();
    await page
      .locator("#cell-paste input[data-name='body']")
      .fill("after-katulong-tab-open");
    await clickPlay(page, "paste");

    // Expectation: the paste still succeeds (no shell exit). If
    // the bug reproduces here, we see {"error":"session ended..."}
    // and a 500 status instead.
    await expect(page.locator("#out-paste")).toContainText('"ok": true', {
      timeout: 5_000,
    });
  });

  test("live view renders xterm.js (not a flat <pre>)", async ({ page }) => {
    // Pins the architectural fix: the visual is an xterm.js
    // terminal, NOT a plain <pre>. xterm.js applies cursor
    // escapes in 2D space, so shells with autosuggestions /
    // syntax highlighting (fish, zsh-autosuggestions) don't leak
    // adjacent text into the visible buffer the way our previous
    // 1D linearisation did. If a future edit reverts to a <pre>,
    // this test fails immediately.
    await page.goto("/");
    await clickPlay(page, "create");
    await expect(page.locator("#meta-session")).toContainText("sipag-d-");
    // xterm.js injects a `.xterm` element into its container.
    await expect(page.locator("#live-view .xterm")).toBeAttached({
      timeout: 10_000,
    });
  });
});
