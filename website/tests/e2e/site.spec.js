import { expect, test } from '@playwright/test';

const releaseApi = 'https://api.github.com/repos/Rayjine/sound-multiplexer/releases/latest';
const release = {
  tag_name: 'v9.8.7-test',
  assets: [
    { name: 'sound-multiplexer.AppImage', browser_download_url: 'https://downloads.example.test/sound.AppImage' },
    { name: 'sound-multiplexer.deb', browser_download_url: 'https://downloads.example.test/sound.deb' },
    { name: 'sound-multiplexer.rpm', browser_download_url: 'https://downloads.example.test/sound.rpm' },
    { name: 'sound-multiplexer-setup.exe', browser_download_url: 'https://downloads.example.test/sound.exe' },
  ],
};

test.beforeEach(async ({ page }) => {
  // The release request is the only API call; keep it deterministic and local.
  await page.route(releaseApi, (route) => route.fulfill({ json: release }));
});

test('release metadata updates all four downloads while keeping fallback labels', async ({ page }) => {
  await page.goto('./');

  const cards = page.locator('.dl-card');
  await expect(cards).toHaveCount(4);
  for (const [kind, suffix] of [['appimage', '.AppImage'], ['deb', '.deb'], ['rpm', '.rpm'], ['windows', '.exe']]) {
    await expect(page.locator(`[data-asset="${kind}"]`)).toHaveAttribute('href', `https://downloads.example.test/sound${suffix}`);
  }
  await expect(page.locator('[data-version]')).toHaveCount(2);
  await expect(page.locator('[data-version]').first()).toHaveText('v9.8.7-test');
  await expect(page.locator('[data-version]').first()).toBeVisible();
});

test('device toggles update pressed state, active count, and master-row visibility', async ({ page }) => {
  await page.goto('./');

  const tv = page.getByRole('button', { name: 'Play audio on Living-room TV' });
  await tv.click();
  await expect(tv).toHaveAttribute('aria-pressed', 'true');
  await expect(page.locator('.aw-status-text')).toHaveText('3 devices active');

  await page.getByRole('button', { name: 'Play audio on Built-in Speakers' }).click();
  await page.getByRole('button', { name: 'Play audio on HD 660S Headphones' }).click();
  await expect(page.locator('.aw-status-text')).toHaveText('1 device active');
  await expect(page.locator('.aw-master')).toBeHidden();

  await tv.click();
  await expect(page.locator('.aw-status-text')).toHaveText('Silent mode — no devices selected');
});

test('mute and volume controls expose keyboard state and bounds', async ({ page }) => {
  await page.goto('./');

  const volume = page.getByRole('slider', { name: 'Volume for Built-in Speakers' });
  await volume.focus();
  await expect(volume).toBeFocused();
  expect(await volume.evaluate((el) => getComputedStyle(el).outlineStyle)).not.toBe('none');
  await page.keyboard.press('End');
  await expect(volume).toHaveAttribute('aria-valuenow', '100');
  await page.keyboard.press('ArrowLeft');
  await expect(volume).toHaveAttribute('aria-valuenow', '95');
  await page.keyboard.press('Home');
  await expect(volume).toHaveAttribute('aria-valuenow', '0');

  const mute = page.getByRole('button', { name: 'Mute Built-in Speakers' });
  await mute.click();
  await expect(mute).toHaveAttribute('aria-pressed', 'true');
  await expect(page.getByRole('button', { name: 'Unmute Built-in Speakers' })).toBeVisible();
});

test('release API 404 preserves the working releases-page fallback', async ({ page }) => {
  await page.route(releaseApi, (route) => route.fulfill({ status: 404, contentType: 'application/json', body: '{"message":"Not Found"}' }));
  const pageErrors = [];
  page.on('pageerror', (error) => pageErrors.push(error.message));
  await page.goto('./');

  for (const card of await page.locator('.dl-card').all()) {
    await expect(card).toHaveAttribute('href', 'https://github.com/Rayjine/sound-multiplexer/releases/latest');
  }
  for (const chip of await page.locator('[data-version]').all()) await expect(chip).toBeHidden();
  expect(pageErrors).toEqual([]);
});

test('mobile layout at 360px keeps metadata, focus and download actions reachable', async ({ page }) => {
  await page.setViewportSize({ width: 360, height: 800 });
  await page.goto('./');

  await expect(page).toHaveTitle('Sound Multiplexer — play audio through several output devices at once');
  await expect(page.locator('html')).toHaveAttribute('lang', 'en');
  await expect(page.locator('meta[name="description"]')).toHaveAttribute('content', /Linux and Windows/);
  await expect(page.getByRole('main')).toBeVisible();
  const widths = await page.evaluate(() => ({ viewport: document.documentElement.clientWidth, content: document.documentElement.scrollWidth }));
  expect(widths.content).toBeLessThanOrEqual(widths.viewport);

  const sourceLink = page.getByRole('link', { name: 'GitHub', exact: true });
  await page.keyboard.press('Tab');
  await expect(sourceLink).toBeFocused();
  await sourceLink.press('Tab');
  const download = page.getByRole('link', { name: /Download/ }).first();
  await expect(download).toBeFocused();
});

test('@published smoke: deployed page serves CSS and interactive demo', async ({ page }) => {
  await page.setViewportSize({ width: 360, height: 800 });
  const response = await page.goto('./');
  expect(response?.status()).toBe(200);
  await expect(page.getByRole('heading', { level: 1, name: /Play audio through several output devices/ })).toBeVisible();

  const stylesheet = await page.locator('link[rel="stylesheet"]').getAttribute('href');
  expect(stylesheet).toBeTruthy();
  const stylesheetResponse = await page.request.get(new URL(stylesheet, page.url()).toString());
  expect(stylesheetResponse.status()).toBe(200);

  const tv = page.getByRole('button', { name: 'Play audio on Living-room TV' });
  await tv.click();
  await expect(page.locator('.aw-status-text')).toHaveText('3 devices active');
  const widths = await page.evaluate(() => ({ viewport: document.documentElement.clientWidth, content: document.documentElement.scrollWidth }));
  expect(widths.content).toBeLessThanOrEqual(widths.viewport);
});
