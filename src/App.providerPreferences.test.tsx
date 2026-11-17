// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import App from './App';
import type { ProviderUsage } from './types';

// Real useSettings (localStorage-backed, inert outside Tauri) so visibility,
// ordering, and persistence are tested end to end through the Settings
// destination. The main window no longer carries the provider rail — the
// floating bar (a separate surface) and the run panel consume the same
// preferences.
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
    usageIntelligenceRevision: 0,
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
    usage({
      id: 'minimax-token-plan',
      name: 'MiniMax Token Plan',
      limits: [
        { label: '5-hour', usedPercent: 62 },
        { label: 'Weekly', usedPercent: 18 },
      ],
    }),
  ];
}

function openSettings() {
  fireEvent.click(screen.getByRole('button', { name: 'Settings' }));
  fireEvent.click(screen.getByRole('button', { name: 'Providers & Wallets' }));
}

function providerSwitchLabels() {
  return within(screen.getByRole('group', { name: 'Providers' }))
    .getAllByRole('switch')
    .map((el) => el.getAttribute('aria-label') ?? '');
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
    openSettings();
    expect(providerSwitchLabels()).toEqual([
      'Show OpenAI / Codex in the floating bar',
      'Show Z.ai in the floating bar',
      'Show OpenCode Go in the floating bar',
      'Show Google Antigravity in the floating bar',
      'Show Grok (xAI) in the floating bar',
      'Show MiniMax Token Plan in the floating bar',
    ]);
  });

  it('hides the MiniMax Token Plan after the subscription expires', () => {
    // The one-month subscription is a temporary arrangement: hiding the
    // provider must be a single switch, and hiding it must not stop the
    // runtime from refreshing it (history and notifications continue).
    render(<App />);
    openSettings();
    fireEvent.click(
      screen.getByRole('switch', { name: 'Show MiniMax Token Plan in the floating bar' }),
    );
    expect(storedPrefs().hidden).toEqual(['minimax-token-plan']);
  });

  it('hides a provider and persists the choice', () => {
    render(<App />);
    openSettings();
    fireEvent.click(screen.getByRole('switch', { name: 'Show Grok (xAI) in the floating bar' }));
    expect(storedPrefs().hidden).toEqual(['grok']);
  });

  it('refuses to hide the final visible provider with feedback', () => {
    render(<App />);
    openSettings();
    const names = [
      'OpenAI / Codex',
      'Z.ai',
      'OpenCode Go',
      'Google Antigravity',
      'MiniMax Token Plan',
    ];
    for (const name of names) {
      fireEvent.click(screen.getByRole('switch', { name: 'Show ' + name + ' in the floating bar' }));
    }
    fireEvent.click(screen.getByRole('switch', { name: 'Show Grok (xAI) in the floating bar' }));
    expect(screen.getByText('At least one provider stays visible.')).toBeTruthy();
    // Grok — the last visible provider — was not hidden.
    expect(storedPrefs().hidden).toHaveLength(5);
    expect(storedPrefs().hidden).not.toContain('grok');
  });

  it('reorders providers with move buttons', () => {
    render(<App />);
    openSettings();
    fireEvent.click(screen.getByRole('button', { name: 'Move OpenCode up' }));
    fireEvent.click(screen.getByRole('button', { name: 'Move OpenCode up' }));
    expect(providerSwitchLabels()[0]).toBe('Show OpenCode Go in the floating bar');
    expect(storedPrefs().order.slice(0, 2)).toEqual(['opencode-go', 'openai-codex']);
  });

  it('disables move up for the first provider and move down for the last', () => {
    render(<App />);
    openSettings();
    const moveFirstUp = screen.getByRole('button', { name: 'Move Codex up' }) as HTMLButtonElement;
    const moveLastDown = screen.getByRole('button', {
      name: 'Move MiniMax down',
    }) as HTMLButtonElement;
    expect(moveFirstUp.disabled).toBe(true);
    expect(moveLastDown.disabled).toBe(true);
  });

  it('supports keyboard operation of the reorder controls', async () => {
    const user = userEvent.setup();
    render(<App />);
    openSettings();
    const moveDown = screen.getByRole('button', { name: 'Move Z.ai down' });
    moveDown.focus();
    expect(document.activeElement).toBe(moveDown);
    await user.keyboard('{Enter}');
    expect(providerSwitchLabels()[1]).toBe('Show OpenCode Go in the floating bar');
    expect(providerSwitchLabels()[2]).toBe('Show Z.ai in the floating bar');
  });
});
