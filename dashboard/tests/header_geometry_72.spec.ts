import { expect, test, type Page } from '@playwright/test';

async function open(page: Page, route: string, width: number) {
  const base = process.env.SYNAPSE_DASHBOARD_TEST_URL;
  if (!base) throw new Error('Manager must supply isolated SYNAPSE_DASHBOARD_TEST_URL');
  await page.setViewportSize({ width, height: 988 });
  await page.goto(`${base.replace(/#.*$/, '')}#/${route}`);
  await expect(page.locator('h1')).toBeVisible();
}

async function geometry(page: Page) {
  return page.locator('header:has(h1)').evaluate(header => {
    const title = header.querySelector('h1')!;
    const children = [...header.children];
    const heading = children[0].getBoundingClientRect();
    const actions = children[1].getBoundingClientRect();
    const document = header.ownerDocument.documentElement;
    return {
      title: title.textContent, headingBottom: heading.bottom, actionsTop: actions.top,
      client: document.clientWidth, scroll: document.scrollWidth,
      titleWidth: title.getBoundingClientRect().width
    };
  });
}

for (const width of [1297, 1920, 600, 360, 320]) {
  test(`must_pass_header_stacks_without_overlap_at_${width}`, async ({ page }) => {
    for (const [route, title] of [['tasks', 'Task Board'], ['system', 'System, Storage & Activity'], ['context', 'Context Inspector'], ['audit', 'Audit Explorer']]) {
      await open(page, route, width);
      await expect(page.locator('h1')).toHaveText(title);
      const box = await geometry(page);
      expect(box.headingBottom).toBeLessThanOrEqual(box.actionsTop);
      expect(box.scroll).toBe(box.client);
      expect(box.titleWidth).toBeGreaterThan(100);
    }
  });
}

test('must_fail_unbroken_saved_view_name_cannot_push_actions_over_heading', async ({ page }) => {
  await open(page, 'system', 360);
  const name = page.locator('#saved-view-name');
  await expect(name).toBeVisible();
  const original = await name.inputValue();
  await name.fill('unbroken'.repeat(100));
  await expect(name).toHaveValue('unbroken'.repeat(100));
  const box = await geometry(page);
  expect(box.headingBottom).toBeLessThanOrEqual(box.actionsTop);
  expect(box.scroll).toBe(box.client);
  await name.fill(original);
  await expect(name).toHaveValue(original);
});
