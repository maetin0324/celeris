import { readFileSync } from "node:fs";
import { expect, test } from "@playwright/test";

// Explicit opt-in: all other parity specs use their own fake daemon.
const base = process.env.WEB_E2E_REAL_BASE_URL;
test.skip(!base, "requires WEB_E2E_REAL_BASE_URL for the staging gateway");

test.beforeEach(async ({ page }) => {
  const passwordFile = process.env.WEB_E2E_PASSWORD_FILE;
  if (!passwordFile) throw new Error("WEB_E2E_PASSWORD_FILE is required");
  const login = await page.request.post("/login", {
    form: { password: readFileSync(passwordFile, "utf8").trim(), next: "/" },
  });
  expect(login.ok()).toBe(true);
  await page.route("**/api/**", (route) => {
    if (route.request().method() !== "GET") return route.abort("blockedbyclient");
    return route.continue();
  });
});

test("parity staging: release and gui are healthy together", async ({ page }) => {
  const health = await page.request.get("/healthz");
  expect(health.ok()).toBe(true);
  const body = await health.json();
  expect(body).toMatchObject({ ok: true, name: "celeris-web", release: process.env.WEB_E2E_SHA12 });
  const gui = await page.request.get("http://127.0.0.1:7701/healthz");
  expect(gui.ok()).toBe(true);
  expect(await gui.json()).toMatchObject({ release: process.env.WEB_E2E_SHA12 });
});

test("parity staging: real celeris serves read-only collections", async ({ page }) => {
  for (const path of ["health", "tasks", "projects", "org", "reports", "clusters"]) {
    const response = await page.request.get(`/api/${path}`);
    expect(response.status(), path).toBe(200);
    expect(response.headers()["content-type"], path).toContain("application/json");
    await response.json();
  }
});

test("parity staging: SPA renders read-only screens", async ({ page }) => {
  for (const path of ["/", "/tasks", "/projects", "/org", "/reports", "/clusters"]) {
    const response = await page.goto(path);
    expect(response?.status(), path).toBe(200);
    await expect(page.locator("[data-shell]"), path).toBeVisible();
    await expect(page.getByRole("heading", { level: 1 }).first(), path).toBeVisible();
    await expect(page.locator("[data-celeris-down]"), path).toHaveCount(0);
  }
});
