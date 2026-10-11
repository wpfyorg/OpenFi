// Ego Lite browser regression against a running fixture-mode server.
// Usage: ego-browser nodejs < qa/rc_browser.mjs

const assert = (await import("node:assert/strict")).default;
const base = "http://127.0.0.1:18088";
const taskName = "OpenWrtFi RC Ego Lite QA";
const existing = (await listTaskSpaces())
  .filter((space) => space.name === taskName)
  .sort((a, b) => b.id - a.id)[0];
const task = existing ? await taskSpace(existing.id) : await taskSpace(taskName);
const page = task.page("p1");
const errors = [];
const failedRequests = [];
const badResponses = [];

// Make reruns deterministic without clearing browser profile state.
await page.goto(`${base}/api/v1/health`);
await page.fetch("/api/v1/auth/logout", {
  method: "POST",
  headers: { "X-OpenWrtFi": "1" },
});

const collectPageErrors = async (label) => {
  const q = await page.evaluate(() => window.__openwrtfiQa ?? {
    errors: [], failedRequests: [], badResponses: [],
  });
  errors.push(...q.errors.map((value) => `${label}: ${value}`));
  failedRequests.push(...q.failedRequests.map((value) => `${label}: ${value}`));
  badResponses.push(...q.badResponses.map((value) => `${label}: ${value}`));
};

await page.goto(`${base}/#/devices`);
await page.waitForURL(`${base}/#/login`, { timeout: 10_000 });

let snapshot = await page.snapshot({ scope: "full_page" });
assert.match(snapshot, /Sign in/);
await page.fill("loc=css:input[type=password]", "wrong");
await page.click('loc=role:button[name="Sign in"]', { label: "submit wrong password" });
await page.waitForSelector("[role=alert]", { state: "visible", timeout: 10_000 });
const alertText = await page.evaluate(
  () => document.querySelector("[role=alert]")?.textContent?.trim() ?? "",
);
assert.match(alertText, /Wrong username or password/);

await page.fill("loc=css:input[type=password]", "mock");
await page.click('loc=role:button[name="Sign in"]', { label: "sign in to OpenWrtFi" });
await page.waitForURL(`${base}/#/devices`, { timeout: 10_000 });
await page.waitForFunction(
  (expected) => [...document.querySelectorAll("h1,h2,h3")]
    .some((el) => el.textContent?.trim() === expected),
  "Devices",
  { timeout: 10_000 },
);

await page.cdp("Page.addScriptToEvaluateOnNewDocument", {
  source: `(() => {
    const q = window.__openwrtfiQa = { errors: [], failedRequests: [], badResponses: [] };
    const originalConsoleError = console.error.bind(console);
    console.error = (...args) => {
      q.errors.push(args.map((value) => String(value)).join(" "));
      originalConsoleError(...args);
    };
    window.addEventListener("error", (event) => {
      if (event.target && event.target !== window) {
        q.failedRequests.push(String(event.target.src || event.target.href || "resource error"));
      } else {
        q.errors.push(String(event.message || event.error || "window error"));
      }
    }, true);
    window.addEventListener("unhandledrejection", (event) => {
      q.errors.push(String(event.reason || "unhandled rejection"));
    });
    const originalFetch = window.fetch.bind(window);
    window.fetch = async (...args) => {
      try {
        const response = await originalFetch(...args);
        if (response.status >= 400) q.badResponses.push(response.status + " " + response.url);
        return response;
      } catch (error) {
        q.failedRequests.push("fetch " + String(args[0]) + ": " + String(error));
        throw error;
      }
    };
    const originalOpen = XMLHttpRequest.prototype.open;
    XMLHttpRequest.prototype.open = function(method, url, ...rest) {
      this.__qaMethod = method;
      this.__qaUrl = url;
      this.addEventListener("load", () => {
        if (this.status >= 400) q.badResponses.push(this.status + " " + this.responseURL);
      });
      this.addEventListener("error", () => {
        q.failedRequests.push(String(this.__qaMethod) + " " + String(this.__qaUrl));
      });
      return originalOpen.call(this, method, url, ...rest);
    };
  })();`,
});

const routes = [
  ["#/", "Dashboard"],
  ["#/topology", "Topology"],
  ["#/traffic", "Traffic"],
  ["#/usage", "Usage"],
  ["#/devices", "Devices"],
  ["#/wifi", "WiFi"],
  ["#/dns", "DNS filter"],
  ["#/qos", "QoS"],
  ["#/settings/wifi", "WiFi networks"],
  ["#/settings/nodes", "Nodes"],
  ["#/logs", "Logs"],
];

for (const [hash, heading] of routes) {
  await page.goto(`${base}/${hash}`);
  await page.waitForFunction(
    (expected) => [...document.querySelectorAll("h1,h2,h3")]
      .some((el) => el.textContent?.trim() === expected),
    heading,
    { timeout: 10_000 },
  );
  await page.waitForTimeout(150);
  await collectPageErrors(hash);
}

await page.reload();
await page.waitForFunction(
  () => [...document.querySelectorAll("h1,h2,h3")]
    .some((el) => el.textContent?.trim() === "Logs"),
  undefined,
  { timeout: 10_000 },
);
await collectPageErrors("reload");

const streamOk = await page.evaluate(() => new Promise((resolve) => {
  const stream = new EventSource("/api/v1/stream");
  const timer = setTimeout(() => {
    stream.close();
    resolve(false);
  }, 8_000);
  stream.addEventListener("snapshot", () => {
    clearTimeout(timer);
    stream.close();
    resolve(true);
  }, { once: true });
  stream.onerror = () => {};
}));

assert.equal(streamOk, true, "authenticated browser did not receive an SSE snapshot");
assert.deepEqual(errors, [], "browser console or uncaught errors");
assert.deepEqual(failedRequests, [], "failed browser requests");
assert.deepEqual(badResponses, [], "unexpected HTTP error responses after login");

console.log(`PASS: Ego Lite auth, invalid login, ${routes.length} routes, reload, and SSE (${base})`);
await task.finish({ keep: [] });
