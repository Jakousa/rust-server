import { defineConfig, devices } from "@playwright/test";

const host = process.env.E2E_HOST || "127.0.0.1";
const port = Number(process.env.E2E_PORT || 4173);
const baseURL = process.env.E2E_BASE_URL || `http://${host}:${port}`;
const useDevServer = process.env.E2E_DEV === "1";

export default defineConfig({
  testDir: "./e2e",
  timeout: 30_000,
  expect: {
    timeout: 5_000,
  },
  fullyParallel: false,
  retries: process.env.CI ? 1 : 0,
  reporter: [["list"], ["html", { open: "never" }]],
  use: {
    baseURL,
    viewport: { width: 1280, height: 800 },
    trace: "on-first-retry",
    screenshot: "only-on-failure",
    launchOptions: {
      args: ["--use-gl=angle", "--use-angle=swiftshader", "--enable-webgl"],
    },
  },
  projects: [
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"] },
    },
  ],
  webServer: process.env.E2E_BASE_URL
    ? undefined
    : {
        command: useDevServer
          ? `npm run dev -- --port ${port} --host ${host} --strictPort`
          : `npm run preview -- --port ${port} --host ${host} --strictPort`,
        url: `http://${host}:${port}`,
        reuseExistingServer: process.env.E2E_REUSE_SERVER
          ? process.env.E2E_REUSE_SERVER === "1"
          : !process.env.CI,
        timeout: 15_000,
      },
});
