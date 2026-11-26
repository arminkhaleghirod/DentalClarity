// Captures the README screenshots in docs/screenshots/.
//
// Drives a headless Chromium browser (Microsoft Edge or Chrome) over the
// Chrome DevTools Protocol using only Node's built-in fetch and WebSocket
// (Node 22 or newer, no npm packages).
//
// 1. Start the server in demo mode with a fresh data folder:
//      DC_DATA_DIR=<scratch folder> cargo run --release -p dc-server
// 2. Run from the repository root:
//      node scripts/screenshots/capture.mjs
//
// The run generates one presentation (a few cents of model spend through
// OpenRouter). Options, all optional, as environment variables:
//   DC_URL     server address (default http://127.0.0.1:8080)
//   BROWSER    path to msedge.exe or chrome
//   SCALE      device scale factor (default 2)
//   OUT_DIR    output folder (default docs/screenshots)
//   PROFILE    browser profile folder; demo presentations belong to the
//              visitor session stored there (default: a new temp folder)
//   SKIP_GENERATE=1  reuse the newest presentation of that visitor instead
//              of generating a new one (needs the PROFILE of an earlier run)

import { spawn } from "node:child_process";
import { mkdtempSync, writeFileSync, mkdirSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const BASE = process.env.DC_URL ?? "http://127.0.0.1:8080";
const BROWSER =
  process.env.BROWSER ??
  "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe";
const SCALE = Number(process.env.SCALE ?? 2);
const OUT_DIR = resolve(process.env.OUT_DIR ?? "docs/screenshots");
const PROFILE = process.env.PROFILE;
const SKIP_GENERATE = process.env.SKIP_GENERATE === "1";
const PORT = 9333;

const STAFF = { width: 1280, height: 800 };
const PATIENT = { width: 1180, height: 820 };
// At a glance is taller so the tooth chart and the estimated share both fit.
const GLANCE = { width: 1180, height: 920 };

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// ---------------------------------------------------------------- browser

async function launchBrowser() {
  const profile = PROFILE ?? mkdtempSync(join(tmpdir(), "dc-shots-"));
  console.log(`browser profile: ${profile}`);
  const proc = spawn(
    BROWSER,
    [
      "--headless=new",
      `--remote-debugging-port=${PORT}`,
      `--user-data-dir=${profile}`,
      "--no-first-run",
      "--no-default-browser-check",
      "--hide-scrollbars",
      "--force-color-profile=srgb",
      "about:blank",
    ],
    { stdio: "ignore" },
  );
  for (let i = 0; i < 50; i++) {
    try {
      const res = await fetch(`http://127.0.0.1:${PORT}/json/list`);
      const page = (await res.json()).find((t) => t.type === "page");
      if (page) return { proc, wsUrl: page.webSocketDebuggerUrl };
    } catch {
      // not listening yet
    }
    await sleep(200);
  }
  proc.kill();
  throw new Error("browser did not open a debugging port");
}

class Cdp {
  constructor(ws) {
    this.ws = ws;
    this.nextId = 1;
    this.pending = new Map();
    this.listeners = new Set();
    ws.addEventListener("message", (event) => {
      const msg = JSON.parse(event.data);
      if (msg.id && this.pending.has(msg.id)) {
        const { resolve, reject } = this.pending.get(msg.id);
        this.pending.delete(msg.id);
        msg.error ? reject(new Error(msg.error.message)) : resolve(msg.result);
      } else if (msg.method) {
        for (const fn of this.listeners) fn(msg);
      }
    });
  }

  static async connect(url) {
    const ws = new WebSocket(url);
    await new Promise((ok, fail) => {
      ws.addEventListener("open", ok, { once: true });
      ws.addEventListener("error", fail, { once: true });
    });
    return new Cdp(ws);
  }

  send(method, params = {}) {
    const id = this.nextId++;
    this.ws.send(JSON.stringify({ id, method, params }));
    return new Promise((resolve, reject) => this.pending.set(id, { resolve, reject }));
  }

  once(method) {
    return new Promise((resolve) => {
      const fn = (msg) => {
        if (msg.method === method) {
          this.listeners.delete(fn);
          resolve(msg.params);
        }
      };
      this.listeners.add(fn);
    });
  }
}

// ---------------------------------------------------------------- page helpers

let cdp;
let inflight = 0;

async function evaluate(expression) {
  const { result, exceptionDetails } = await cdp.send("Runtime.evaluate", {
    expression,
    awaitPromise: true,
    returnByValue: true,
  });
  if (exceptionDetails) throw new Error(exceptionDetails.exception?.description ?? expression);
  return result.value;
}

async function waitFor(label, expression, timeoutMs = 20000) {
  const end = Date.now() + timeoutMs;
  while (Date.now() < end) {
    if (await evaluate(expression).catch(() => false)) return;
    await sleep(250);
  }
  throw new Error(`timed out waiting for ${label}`);
}

const hasText = (text) =>
  `document.body && document.body.innerText.includes(${JSON.stringify(text)})`;

// True once no loading skeleton is on screen, no view transition is running
// and every finite CSS animation or transition has finished. Endless ones,
// such as a progress spinner, are ignored.
const STILL = `(() => {
  if (document.querySelector('.skeleton')) return false;
  if (document.documentElement.matches('[class*="vt-"]')) return false;
  return document.getAnimations().every((a) => {
    const end = a.effect?.getComputedTiming().endTime;
    return a.playState !== 'running' || end === Infinity;
  });
})()`;

// Waits until no request has been in flight for `quietMs`, images have
// loaded, fonts are ready and the page has stopped moving.
async function settle(quietMs = 700) {
  let quietSince = Date.now();
  const end = Date.now() + 20000;
  while (Date.now() < end) {
    if (inflight > 0) quietSince = Date.now();
    if (Date.now() - quietSince >= quietMs) break;
    await sleep(100);
  }
  await evaluate(
    `document.fonts.ready.then(() => Promise.all([...document.images].map((img) =>
       img.complete ? null : new Promise((r) => { img.onload = img.onerror = r; }))))
     .then(() => true)`,
  );
  await waitFor("page to stop moving", STILL).catch(async (err) => {
    const moving = await evaluate(`[
      document.documentElement.className,
      ...[...document.querySelectorAll('.skeleton')].map((e) => 'skeleton ' + e.className),
      ...document.getAnimations().filter((a) => a.playState === 'running')
        .map((a) => (a.animationName ?? a.transitionProperty) + ' on ' + a.effect?.target?.className),
    ].join('; ')`);
    throw new Error(`${err.message}: ${moving}`);
  });
  await sleep(300);
}

async function setViewport({ width, height }) {
  await cdp.send("Emulation.setDeviceMetricsOverride", {
    width,
    height,
    deviceScaleFactor: SCALE,
    mobile: false,
  });
}

async function navigate(path) {
  const loaded = cdp.once("Page.loadEventFired");
  await cdp.send("Page.navigate", { url: BASE + path });
  await loaded;
}

// Clicks the first visible button or link whose text matches exactly.
async function click(text, scope = "button, a, [role=tab], [role=radio]") {
  const ok = await evaluate(`(() => {
    const want = ${JSON.stringify(text)};
    const el = [...document.querySelectorAll(${JSON.stringify(scope)})]
      .find((e) => e.offsetParent !== null && e.innerText.trim() === want);
    if (!el) return false;
    el.scrollIntoView({ block: "center" });
    el.click();
    return true;
  })()`);
  if (!ok) throw new Error(`no clickable element with text "${text}"`);
}

// Clicks the first visible element matching `selector` whose text contains `text`.
async function clickContaining(selector, text) {
  const ok = await evaluate(`(() => {
    const el = [...document.querySelectorAll(${JSON.stringify(selector)})]
      .find((e) => e.offsetParent !== null && e.innerText.includes(${JSON.stringify(text)}));
    if (!el) return false;
    el.click();
    return true;
  })()`);
  if (!ok) throw new Error(`no ${selector} containing "${text}"`);
}

async function shot(name) {
  await evaluate("window.scrollTo(0, 0), true");
  await sleep(200);
  const { data } = await cdp.send("Page.captureScreenshot", { format: "png" });
  const file = join(OUT_DIR, `${name}.png`);
  writeFileSync(file, Buffer.from(data, "base64"));
  console.log(`${name}.png  ${Math.round(statSync(file).size / 1024)} KB`);
}

// ---------------------------------------------------------------- flow

// Generates a presentation for the root canal plan and captures the progress
// card twice: while sections are still being written and when it is ready.
async function generate() {
  await navigate("/patients/10482/plans/tp-10482-1/new");
  await waitFor("create page", hasText("Create presentation"));
  await settle();
  await click("Create presentation");
  await waitFor(
    "generation midway",
    `document.querySelectorAll('.stage-row.passed').length >= 1 &&
     document.querySelectorAll('.stage-row.active').length >= 1`,
    300000,
  );
  await shot("create-progress");
  await waitFor("generation finished", hasText("Open presentation"), 300000);
  await settle();
  await shot("create-ready");
  await click("Open presentation");
}

// Clicks Done on the patient screen and waits for the staff workspace.
async function leavePresent() {
  const loaded = cdp.once("Page.loadEventFired");
  await click("Done");
  await loaded;
  await waitFor("staff workspace", `location.pathname !== "/present" && !location.pathname.startsWith("/present/")`);
}

async function main() {
  mkdirSync(OUT_DIR, { recursive: true });
  const { proc, wsUrl } = await launchBrowser();
  try {
    cdp = await Cdp.connect(wsUrl);
    cdp.listeners.add((msg) => {
      if (msg.method === "Network.requestWillBeSent") inflight++;
      if (msg.method === "Network.loadingFinished" || msg.method === "Network.loadingFailed") {
        inflight = Math.max(0, inflight - 1);
      }
    });
    await cdp.send("Page.enable");
    await cdp.send("Network.enable");
    await cdp.send("Runtime.enable");
    await cdp.send("Emulation.setEmulatedMedia", {
      features: [{ name: "prefers-color-scheme", value: "light" }],
    });

    // Staff screens. Present mode locks the staff workspace until the
    // patient screen is left, so an interrupted earlier run is ended first.
    await setViewport(STAFF);
    await navigate("/");
    await waitFor("dashboard or lock screen",
      `${hasText("Ready to present")} || ${hasText("Return to presentation")}`);
    if (await evaluate(hasText("Return to presentation"))) {
      await navigate("/present");
      await waitFor(
        "present view (a crashed run can leave the visit locked; start over with a new PROFILE)",
        "document.querySelector('.p-tabs') !== null",
      );
      await leavePresent();
      await navigate("/");
    }
    await waitFor("dashboard", hasText("Ready to present"));
    await settle();
    await shot("dashboard");

    await navigate("/patients");
    await waitFor("patient list", "document.querySelectorAll('table tbody tr').length > 0");
    await settle();
    await shot("patients");

    if (SKIP_GENERATE) {
      await navigate("/presentations");
      await waitFor("presentation list", hasText("Daniel Okafor"));
      await settle();
      await evaluate(`[...document.querySelectorAll('a')]
        .find((a) => (a.getAttribute('aria-label') ?? '').startsWith('Open presentation')).click(), true`);
    } else {
      await generate();
    }

    // Editor: confirm the note-derived explanation on the root canal.
    await waitFor("editor", "document.querySelector('.section-rail') !== null");
    await settle();
    await clickContaining(".section-rail .rail-item", "Root canal");
    await settle();
    // Each note-derived block has its own "Looks right" button. A rerun with
    // SKIP_GENERATE finds them already confirmed.
    while (await evaluate(`[...document.querySelectorAll('button')].some((b) => b.innerText.trim() === 'Looks right')`)) {
      await click("Looks right");
      await waitFor("note confirmed", `!(${hasText("Confirming…")})`);
      await settle();
    }
    await shot("editor");

    // Present mode is a full page load to /present.
    await setViewport(GLANCE);
    const loaded = cdp.once("Page.loadEventFired");
    await click("Present");
    await loaded;
    await waitFor("present view", "document.querySelector('.p-tabs') !== null");
    await settle();
    await shot("present-at-a-glance");
    await setViewport(PATIENT);

    await click("Procedures", ".p-tabs button");
    await settle();
    await clickContaining(".p-step", "Root canal");
    await waitFor("illustration", "document.querySelector('.p-illustration-img') !== null");
    await settle();
    await shot("present-procedure");

    await click("Journey", ".p-tabs button");
    await settle();
    await shot("present-journey");

    await click("Costs", ".p-tabs button");
    await settle();
    await shot("present-costs");

    // Leave present mode so the staff workspace is unlocked for the next run.
    await leavePresent();
  } finally {
    proc.kill();
  }
}

main().catch((err) => {
  console.error(err.message);
  process.exit(1);
});
