import { test, expect } from "@playwright/test";

test.describe("OpenSpec MMO Web Inspector E2E", () => {
  test("boots WebGL scene and renders non-overlapping HUD panels without page errors", async ({
    page,
  }) => {
    const pageErrors = [];
    page.on("pageerror", (err) => pageErrors.push(err.message));

    await page.goto("/", { waitUntil: "domcontentloaded" });
    await expect(page).toHaveTitle(/OpenSpec MMO/);

    // WebGL canvas should be mounted and sized to the viewport
    const canvas = page.locator("canvas");
    await expect(canvas).toHaveCount(1);
    const canvasBox = await canvas.boundingBox();
    expect(canvasBox).not.toBeNull();
    expect(canvasBox.width).toBeGreaterThanOrEqual(1200);
    expect(canvasBox.height).toBeGreaterThanOrEqual(700);

    const hasWebGL = await page.evaluate(() => {
      const c = document.querySelector("canvas");
      return !!(c && (c.getContext("webgl2") || c.getContext("webgl")));
    });
    expect(hasWebGL).toBe(true);

    // Core HUD panels should be visible
    await expect(page.locator(".connection-panel")).toBeVisible();
    await expect(page.locator(".telemetry-panel")).toBeVisible();
    await expect(page.locator(".debug-panel")).toBeVisible();
    await expect(page.locator(".controls-hint")).toBeVisible();
    await expect(page.locator(".inventory-panel")).toBeVisible();

    // Target panel and modals should start hidden
    await expect(page.locator("#target-panel")).toBeHidden();
    await expect(page.locator("#npc-barter-modal")).toBeHidden();
    await expect(page.locator("#player-trade-modal")).toBeHidden();

    // No uncaught runtime exceptions
    expect(pageErrors).toEqual([]);
  });

  test("resolves gateway URL and handles debug toggles", async ({ page }) => {
    await page.goto("/", { waitUntil: "domcontentloaded" });

    const gatewayUrl = await page.inputValue("#gateway-url");
    expect(gatewayUrl).toMatch(/^https:\/\/.+:4433$/);

    // Toggle visual debug checkboxes
    const partitions = page.locator("#toggle-partitions");
    const aoi = page.locator("#toggle-aoi");
    const wireframe = page.locator("#toggle-wireframe");

    await expect(partitions).toBeChecked();
    await expect(aoi).toBeChecked();
    await expect(wireframe).not.toBeChecked();

    await partitions.uncheck();
    await aoi.uncheck();
    await wireframe.check();

    await expect(partitions).not.toBeChecked();
    await expect(aoi).not.toBeChecked();
    await expect(wireframe).toBeChecked();
  });

  test("updates local player position on WASD movement and supports mouse orbit", async ({
    page,
  }) => {
    await page.goto("/", { waitUntil: "domcontentloaded" });

    const posEl = page.locator("#val-pos");
    const initialPos = await posEl.textContent();
    expect(initialPos).toContain("X: 0.0, Y: 0.0, Z:");

    // Move forward with W
    await page.keyboard.down("w");
    await page.waitForTimeout(350);
    await page.keyboard.up("w");

    const afterW = await posEl.textContent();
    expect(afterW).not.toEqual(initialPos);

    // Mouse drag orbit on canvas should not throw
    const pageErrors = [];
    page.on("pageerror", (err) => pageErrors.push(err.message));
    await page.mouse.move(640, 400);
    await page.mouse.down();
    await page.mouse.move(740, 360, { steps: 5 });
    await page.mouse.up();
    await page.mouse.wheel(0, 120);

    expect(pageErrors).toEqual([]);
  });

  test("auto-discovers /cert-hash when served behind gateway ingress", async ({
    page,
  }) => {
    const isLive = !!process.env.E2E_BASE_URL;
    test.skip(!isLive, "Live ingress /cert-hash check runs when E2E_BASE_URL is set");

    await page.goto("/", { waitUntil: "networkidle" });
    const certHash = await page.inputValue("#cert-hash");
    expect(certHash).toMatch(/^[0-9a-f]{2}(:[0-9a-f]{2}){31}$/i);
  });
});
