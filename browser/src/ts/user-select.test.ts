// browser/src/ts/user-select.test.ts
// Regression: the browser's native autocomplete on the user-select input must
// be disabled. Firefox/Chrome ignore autocomplete="off" on bare text inputs;
// when their own popup opens, Enter confirms the browser suggestion instead
// of selecting the typed Svelecte option (typing + Enter appeared broken).
import { describe, it, expect, beforeAll, vi } from 'vitest';

beforeAll(() => {
  window.matchMedia = window.matchMedia || ((query: string) => ({ matches: false, media: query }) as MediaQueryList);
  // jsdom gaps hit by Svelecte's dropdown and Svelte transitions; without
  // them the test passes but vitest reports unhandled errors and exits 1.
  Element.prototype.scroll ??= function () {};
  Element.prototype.getAnimations ??= () => [];
  // jsdom computes no CSS, so Svelecte's pixelGetter (maxHeight, paddingTop,
  // paddingBottom) finds no "<n>px" value. Give those a pixel value.
  const pixelProps = new Set(['maxHeight', 'paddingTop', 'paddingBottom']);
  const realStyle = window.getComputedStyle.bind(window);
  window.getComputedStyle = ((el: Element, pseudo?: string | null) => {
    const s = realStyle(el, pseudo);
    return new Proxy(s, {
      get(t, p) {
        const v = Reflect.get(t, p);
        if (typeof v === 'function') return v.bind(t);
        if (typeof p === 'string' && pixelProps.has(p) && !/\d+[a-zA-Z]+/.test(v)) return '300px';
        return v;
      },
    });
  }) as typeof window.getComputedStyle;
});

vi.mock('$api', () => ({
  api: {
    getUsers: async () => ['user01', 'user02'],
    getFolders: async () => [],
    getFiles: async () => ({ files: [], totals: {} }),
    probeHealth: async () => {},
  },
}));

import { mount, unmount } from 'svelte';
import Page from '../routes/+page.svelte';
import { State } from './store.svelte';

describe('user select native autocomplete', () => {
  it('replaces autocomplete="off" with a random token after mount', async () => {
    State.isAdmin = true;
    const target = document.createElement('div');
    document.body.appendChild(target);
    const instance = mount(Page, { target });
    await new Promise((r) => setTimeout(r, 100));

    const input = document.querySelector('input.sv-input--text') as HTMLInputElement;
    expect(input).toBeTruthy();
    const ac = input.getAttribute('autocomplete');
    // Must NOT be "off" (ignored by browsers) — must be a random token.
    expect(ac).not.toBe('off');
    expect(ac).toMatch(/^svelecte-user-[a-z0-9]+$/);

    // And typing + Enter still selects (Svelecte path unaffected).
    const control = document.querySelector('.sv-control') as HTMLElement;
    control.dispatchEvent(new MouseEvent('mousedown', { bubbles: true, cancelable: true }));
    control.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }));
    await new Promise((r) => setTimeout(r, 30));
    input.value = 'user02';
    input.dispatchEvent(new Event('input', { bubbles: true }));
    await new Promise((r) => setTimeout(r, 30));
    input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true, cancelable: true }));
    input.dispatchEvent(new KeyboardEvent('keyup', { key: 'Enter', bubbles: true, cancelable: true }));
    await new Promise((r) => setTimeout(r, 80));

    const sel = (document.querySelector('.sv-item--wrap.in-selection .sv-item--content') as HTMLElement)?.textContent?.trim();
    expect(sel).toBe('user02');

    unmount(instance);
    target.remove();
  });
});
