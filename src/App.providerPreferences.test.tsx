// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import App from './App';
import type { ProviderUsage } from './types';

// Real useSettings (localStorage-backed, inert outside Tauri) so visibility,
// ordering, selection fallback, and persistence are tested end to end.
vi.mock('./hooks/useProviderUsage', () => ({
  useProviderUsage: () => ({
    usages: standardUsages(),
    loading: false,
    lastUpdatedAt: new Date('2026-09-28T21:58:00'),
    refresh: vi.fn(),
    refreshOverdue: false,
    stale: false,
    staleMinutes: 0,
    historyRevision: 1,
  }),
}));

vi.mock('./hooks/useNow', () => ({
  useNow: () => new Date('2026-09-28T22:00:00').getTime(),
}));

vi.mock('./hooks/useQuotaPredictions', () => ({
  useQuotaPredictions: () => ({
    predictionFor: () => undefined,
    historyUnavailable: false,
    clearLocalHistory: vi.fn(),
  }),
}));

function usage(overrides: Partial<ProviderUsage>): ProviderUsage {
  return {
    id: 'zai',
    name: 'Z.ai',
    status: 'ok',
    health: 'live',
    checkedAt: '2026-09-28T21:58:00Z',
    limits: [],
    ...overrides,
  };
}

function standardUsages(): ProviderUsage[] {
  return [
    usage({
      id: 'openai-codex',
      name: 'OpenAI / Codex',
      limits: [{ label: 'Weekly credits', usedPercent: 78 }],
    }),
    usage({
      id: 'zai',
      name: 'Z.ai',
      limits: [{ label: '30-day credits', usedPercent: 42 }],
    }),
    usage({
      id: 'opencode-go',
      name: 'OpenCode Go',
      limits: [{ label: 'Weekly requests', usedPercent: 12 }],
    }),
    usage({
      id: 'antigravity',
      name: 'Google Antigravity',
      limits: [{ label: 'Weekly requests', usedPercent: 20 }],
    }),
    usage({
      id: 'grok',
      name: 'Grok (xAI)',
      limits: [{ label: 'Weekly', usedPercent: 96 }],
    }),
  ];
}

function expandDrawer() {
  fireEvent.click(screen.getByRole('button', { name: 'Settings' }));
}

function providerSwitches() {
  return within(screen.getByRole('group', { name: 'Providers' })).getAllByRole('switch');
}

function railNames() {
  return screen.getAllByRole('tab').map((tab) => tab.getAttribute('aria-label') ?? '');
}

function storedPrefs() {
  return JSON.parse(window.localStorage.getItem('rate-limits.settings.v1')!).providerPreferences;
}

describe('provider presentation preferences', () => {
  beforeEach(() => {
    window.localStorage.clear();
  });
  afterEach(() => {
    cleanup();
  });

  it('lists providers in canonical registry order with visibility switches', () => {
    render(<App />);
    expandDrawer();
    expect(providerSwitches().map((el) => el.getAttribute('aria-label'))).toEqual([
      'Show OpenAI / Codex in the dashboard',
      'Show Z.ai in the dashboard',
      'Show OpenCode Go in the dashboard',
      'Show Google Antigravity in the dashboard',
      'Show Grok (xAI) in the dashboard',
    ]);
  });

  it('hides a provider from the main navigation and persists the choice', () => {
    render(<App />);
    expandDrawer();
    fireEvent.click(screen.getByRole('switch', { name: 'Show Grok (xAI) in the dashboard' }));
    expect(screen.queryByRole('tab', { name: /Grok/ })).toBeNull();
    expect(railNames()).toHaveLength(4);
    expect(storedPrefs().hidden).toEqual(['grok']);
  });

  it('refuses to hide the final visible provider with feedback', () => {
    render(<App />);
    expandDrawer();
    const names = ['OpenAI / Codex', 'Z.ai', 'OpenCode Go', 'Google Antigravity'];
    for (const name of names) {
      fireEvent.click(screen.getByRole('switch', { name: 'Show ' + name + ' in the dashboard' }));
    }
    expect(railNames()).toHaveLength(1);
    fireEvent.click(screen.getByRole('switch', { name: 'Show Grok (xAI) in the dashboard' }));
    expect(screen.getByText('At least one provider stays visible.')).toBeTruthy();
    expect(railNames()).toHaveLength(1);
    expect(railNames()[0]).toMatch(/Grok/);
  });

  it('reorders providers with move buttons', () => {
    render(<App />);
    expandDrawer();
    fireEvent.click(screen.getByRole('button', { name: 'Move OpenCode up' }));
    fireEvent.click(screen.getByRole('button', { name: 'Move OpenCode up' }));
    const names = railNames();
    expect(names[0]).toMatch(/OpenCode Go/);
    expect(storedPrefs().order.slice(0, 2)).toEqual(['opencode-go', 'openai-codex']);
  });

  it('disables move up for the first provider and move down for the last', () => {
    render(<App />);
    expandDrawer();
    const moveFirstUp = screen.getByRole('button', { name: 'Move Codex up' }) as HTMLButtonElement;
    const moveLastDown = screen.getByRole('button', { name: 'Move Grok down' }) as HTMLButtonElement;
    expect(moveFirstUp.disabled).toBe(true);
    expect(moveLastDown.disabled).toBe(true);
  });

  it('falls back when the selected provider is hidden', () => {
    render(<App />);
    fireEvent.click(screen.getByRole('tab', { name: /OpenCode Go/ }));
    expect(within(screen.getByRole('tabpanel')).getByText('OpenCode Go')).toBeTruthy();
    expandDrawer();
    fireEvent.click(screen.getByRole('switch', { name: 'Show OpenCode Go in the dashboard' }));
    expect(within(screen.getByRole('tabpanel')).getByText('Grok (xAI)')).toBeTruthy();
  });

  it('anchors the default selection on the highest visible provider', () => {
    render(<App />);
    expect(within(screen.getByRole('tabpanel')).getByText('Grok (xAI)')).toBeTruthy();
    expandDrawer();
    fireEvent.click(screen.getByRole('switch', { name: 'Show Grok (xAI) in the dashboard' }));
    expect(within(screen.getByRole('tabpanel')).getByText('OpenAI / Codex')).toBeTruthy();
  });

  it('supports keyboard operation of the reorder controls', async () => {
    const user = userEvent.setup();
    render(<App />);
    expandDrawer();
    const moveDown = screen.getByRole('button', { name: 'Move Z.ai down' });
    moveDown.focus();
    expect(document.activeElement).toBe(moveDown);
    await user.keyboard('{Enter}');
    const names = railNames();
    expect(names[1]).toMatch(/OpenCode Go/);
    expect(names[2]).toMatch(/Z\.ai/);
  });
});

