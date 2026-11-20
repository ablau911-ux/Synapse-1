// Run against an owned daemon/dashboard via SYNAPSE_DASHBOARD_TEST_URL.
import { expect, test, type Page } from '@playwright/test';

function url(route: string) {
  const base = process.env.SYNAPSE_DASHBOARD_TEST_URL;
  if (!base) throw new Error('Manager must supply isolated SYNAPSE_DASHBOARD_TEST_URL');
  return `${base.replace(/#.*$/, '')}#/${route}`;
}

async function watchErrors(page: Page) {
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.addInitScript(() => {
    (window as unknown as { g8Errors: string[] }).g8Errors = [];
    window.addEventListener('unhandledrejection', event => {
      (window as unknown as { g8Errors: string[] }).g8Errors.push(String(event.reason));
    });
  });
  return errors;
}

test('must_pass_tasks_to_system_renders_while_replacement_subscription_pending', async ({ page }) => {
  const errors = await watchErrors(page);
  await page.goto(url('tasks'));
  await expect(page.locator('h1')).toHaveText('Task Board');
  let release!: () => void;
  const pending = new Promise<void>(resolve => { release = resolve; });
  let requests = 0;
  await page.route('**/dashboard/events/subscribe', async route => {
    if (route.request().postDataJSON().scope !== 'system') { await route.continue(); return; }
    requests += 1;
    await pending;
    await route.continue();
  });
  try {
    await page.getByRole('button', { name: 'System', exact: true }).click();
    await expect.poll(() => requests).toBe(1);
    await expect(page.locator('h1')).toHaveText('System, Storage & Activity');
    await expect(page.locator('header:has(h1)')).toContainText('SSE connecting');
    expect(errors).toEqual([]);
    expect(await page.evaluate(() => (window as unknown as { g8Errors: string[] }).g8Errors)).toEqual([]);
  } finally { release(); }
});

test('must_fail_rejected_subscription_stays_rendered_and_does_not_invent_live_data', async ({ page }) => {
  const errors = await watchErrors(page);
  await page.route('**/dashboard/events/subscribe', route => route.fulfill({
    status: 503, contentType: 'application/json',
    body: JSON.stringify({ ok: false, code: 'G8_SUBSCRIPTION_REFUSED', message: 'synthetic obstacle' })
  }));
  await page.goto(url('tasks'));
  await expect(page.locator('h1')).toHaveText('Task Board');
  await page.getByRole('button', { name: 'System', exact: true }).click();
  await expect(page.locator('h1')).toHaveText('System, Storage & Activity');
  await expect(page.locator('header:has(h1)')).toContainText('SSE disconnected');
  await expect(page.locator('header:has(h1)')).not.toContainText('SSE live');
  await expect(page.locator('header:has(h1)')).not.toContainText('SSE connected');
  expect(errors).toEqual([]);
  expect(await page.evaluate(() => (window as unknown as { g8Errors: string[] }).g8Errors)).toEqual([]);
});
