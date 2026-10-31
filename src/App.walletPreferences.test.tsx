// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import App from './App';
import type { ApiWallet, ProviderUsage } from './types';

// Real useSettings (localStorage-backed, inert outside Tauri) so the
// monitor and floating-bar switches persist end to end. The Rust push
// effect is skipped outside Tauri, exactly like every other settings lane.
vi.mock('./hooks/useProviderUsage', () => ({
  useProviderUsage: () => ({
    usages: usages(),
    wallets: wallets(),
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

function usages(): ProviderUsage[] {
  return [
    {
      id: 'openai-codex',
      name: 'OpenAI / Codex',
      status: 'ok',
      health: 'live',
      checkedAt: '2026-09-28T21:58:00Z',
      limits: [{ label: 'Weekly credits', usedPercent: 78 }],
    },
  ];
}

function wallet(overrides: Partial<ApiWallet>): ApiWallet {
  return {
    id: 'openrouter',
    name: 'OpenRouter',
    health: 'live',
    checkedAt: '2026-09-28T21:55:00Z',
    balances: [{ currency: 'USD', amount: 12.5 }],
    ...overrides,
  };
}

function wallets(): ApiWallet[] {
  return [
    wallet({}),
    wallet({
      id: 'deepseek',
      name: 'DeepSeek',
      balances: [{ currency: 'CNY', amount: 110 }],
    }),
    wallet({ id: 'openai', name: 'OpenAI', health: 'unknown', checkedAt: undefined, balances: undefined }),
  ];
}

function expandDrawer() {
  fireEvent.click(screen.getByRole('button', { name: 'Settings' }));
  fireEvent.click(screen.getByRole('button', { name: 'Providers & Wallets' }));
}

function walletGroup() {
  return screen.getByRole('group', { name: 'API wallets' });
}

function storedWalletPrefs() {
  return JSON.parse(window.localStorage.getItem('rate-limits.settings.v1')!)
    .walletPreferences;
}

describe('wallet preferences', () => {
  beforeEach(() => {
    window.localStorage.clear();
  });
  afterEach(() => {
    cleanup();
  });

  it('lists every registered wallet with monitor and floating-bar switches', () => {
    render(<App />);
    expandDrawer();
    const group = walletGroup();
    expect(
      within(group)
        .getAllByRole('switch')
        .map((el) => el.getAttribute('aria-label')),
    ).toEqual([
      'Monitor OpenRouter',
      'Show OpenRouter in the floating bar',
      'Monitor DeepSeek',
      'Show DeepSeek in the floating bar',
      'Monitor OpenAI',
      'Show OpenAI in the floating bar',
    ]);
  });

  it('keeps both switches on for the default wallets', () => {
    render(<App />);
    expandDrawer();
    const group = walletGroup();
    for (const name of [
      'Monitor OpenRouter',
      'Show OpenRouter in the floating bar',
      'Monitor DeepSeek',
      'Show DeepSeek in the floating bar',
    ]) {
      const el = within(group).getByRole('switch', {
        name,
      }) as HTMLInputElement;
      expect(el.disabled).toBe(false);
      expect(el.checked).toBe(true);
    }
  });

  it('disables the OpenAI placeholder and states why', () => {
    render(<App />);
    expandDrawer();
    const group = walletGroup();
    for (const name of ['Monitor OpenAI', 'Show OpenAI in the floating bar']) {
      const el = within(group).getByRole('switch', {
        name,
      }) as HTMLInputElement;
      expect(el.disabled).toBe(true);
    }
    expect(group.textContent).toContain(
      'The public OpenAI API exposes usage and cost reporting',
    );
  });

  it('turning monitoring off persists without touching floating-bar visibility', () => {
    render(<App />);
    expandDrawer();
    fireEvent.click(
      within(walletGroup()).getByRole('switch', { name: 'Monitor OpenRouter' }),
    );
    expect(storedWalletPrefs()).toEqual({
      monitored: { openrouter: false },
      floatingBar: {},
    });
    // The other wallet's switches are untouched.
    const deepseek = within(walletGroup()).getByRole('switch', {
      name: 'Monitor DeepSeek',
    }) as HTMLInputElement;
    expect(deepseek.checked).toBe(true);
  });

  it('hiding a wallet from the floating bar persists on its own', () => {
    render(<App />);
    expandDrawer();
    fireEvent.click(
      within(walletGroup()).getByRole('switch', {
        name: 'Show DeepSeek in the floating bar',
      }),
    );
    expect(storedWalletPrefs()).toEqual({
      monitored: {},
      floatingBar: { deepseek: false },
    });
  });

  it('a wallet hidden from the floating bar keeps being monitored', () => {
    render(<App />);
    expandDrawer();
    fireEvent.click(
      within(walletGroup()).getByRole('switch', {
        name: 'Show OpenRouter in the floating bar',
      }),
    );
    fireEvent.click(
      within(walletGroup()).getByRole('switch', { name: 'Monitor DeepSeek' }),
    );
    const stored = storedWalletPrefs();
    expect(stored.floatingBar).toEqual({ openrouter: false });
    expect(stored.monitored).toEqual({ deepseek: false });
  });
});
