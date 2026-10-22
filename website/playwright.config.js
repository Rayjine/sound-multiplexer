import { existsSync } from 'node:fs';
import { env } from 'node:process';
import { defineConfig, devices } from '@playwright/test';

const localChrome = env.CI ? undefined : ['/usr/bin/google-chrome', '/usr/bin/google-chrome-stable'].find(existsSync);
const siteUrl = env.SITE_URL ? `${env.SITE_URL.replace(/\/+$/, '')}/` : undefined;

export default defineConfig({
  testDir: './tests/e2e',
  fullyParallel: true,
  forbidOnly: Boolean(env.CI),
  retries: env.CI ? 1 : 0,
  reporter: env.CI ? 'github' : 'list',
  timeout: 30_000,
  expect: { timeout: 5_000 },
  use: {
    ...devices['Desktop Chrome'],
    baseURL: siteUrl || 'http://127.0.0.1:4180/',
    colorScheme: 'light',
    headless: true,
    launchOptions: localChrome ? { executablePath: localChrome } : {},
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
  },
  webServer: siteUrl ? undefined : {
    command: 'python3 -m http.server 4180 --bind 127.0.0.1 --directory .',
    url: 'http://127.0.0.1:4180/',
    reuseExistingServer: !env.CI,
    timeout: 30_000,
  },
});
