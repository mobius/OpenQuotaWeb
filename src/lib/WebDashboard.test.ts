import { cleanup, fireEvent, render, screen, within } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import WebDashboard from './WebDashboard.svelte';
import { ProviderCatalogIndex } from './metrics';
import type { AppSettings, ProviderViewState, UsageViewState } from './types';
import {
  claudeState,
  codexState,
  liveState,
  providerCatalog,
  settingsState,
} from '../test/appFixtures';

const catalog = new ProviderCatalogIndex(providerCatalog);
const now = Date.parse('2026-07-10T10:00:00Z');

function renderDashboard(
  overrides: Partial<{
    viewState: UsageViewState;
    settings: AppSettings;
    catalog: ProviderCatalogIndex;
    anyRefreshing: boolean;
    onRefreshAll: () => void;
    onRefreshProvider: (id: string) => void;
    onCustomize: () => void;
    onSettings: () => void;
  }> = {},
) {
  const props = {
    viewState: liveState,
    catalog,
    settings: structuredClone(settingsState.settings),
    now,
    anyRefreshing: false,
    onRefreshAll: vi.fn(),
    onRefreshProvider: vi.fn(),
    onCustomize: vi.fn(),
    onSettings: vi.fn(),
    ...overrides,
  };
  render(WebDashboard, props);
  return props;
}

afterEach(cleanup);

describe('WebDashboard', () => {
  it('renders the summary KPIs from the enabled providers', () => {
    renderDashboard();

    const summary = within(screen.getByRole('region', { name: 'Summary' }));
    expect(summary.getByText('Spend · today')).toBeInTheDocument();
    expect(summary.getByText('$3.84')).toBeInTheDocument();
    expect(summary.getByText('2.1M tokens')).toBeInTheDocument();
    expect(summary.getByText('Providers')).toBeInTheDocument();
    expect(summary.getByText('reporting data')).toBeInTheDocument();
    expect(summary.getByText('Alerts')).toBeInTheDocument();
    expect(summary.getByText('all healthy')).toBeInTheDocument();
  });

  it('renders a provider card with its plan, quotas and value metrics', () => {
    renderDashboard();

    const providers = within(screen.getByRole('region', { name: 'Providers' }));
    expect(providers.getByRole('heading', { name: 'Codex' })).toBeInTheDocument();
    expect(providers.getByText('Plus')).toBeInTheDocument();
    expect(providers.getByText('Session')).toBeInTheDocument();
    expect(providers.getByText('68% left')).toBeInTheDocument();
    expect(providers.getByText('41% left')).toBeInTheDocument();
    expect(providers.getByText('Extra Usage')).toBeInTheDocument();
    expect(providers.getByText('$32.84')).toBeInTheDocument();
    expect(providers.getByText('2 available')).toBeInTheDocument();
  });

  it('renders the daily usage breakdown with token and cost totals', () => {
    renderDashboard();

    const providers = within(screen.getByRole('region', { name: 'Providers' }));
    expect(providers.getByText('2.1M')).toBeInTheDocument();
    expect(providers.getByText('$3.84')).toBeInTheDocument();
    expect(providers.getByText('684K')).toBeInTheDocument();
    expect(providers.getByText('$1.27')).toBeInTheDocument();
    expect(providers.getByText('3M')).toBeInTheDocument();
    expect(providers.getByText('$5.11')).toBeInTheDocument();
  });

  it('switches quota rows to used percentages when configured', () => {
    const settings = structuredClone(settingsState.settings);
    settings.usageDisplay = 'used';
    renderDashboard({ settings });

    expect(screen.getByText('32% used')).toBeInTheDocument();
    expect(screen.getByText('59% used')).toBeInTheDocument();
  });

  it('flags providers near a limit in the alerts KPI', () => {
    const viewState: UsageViewState = {
      providers: {
        codex: {
          ...liveState.providers.codex,
          snapshot: {
            ...liveState.providers.codex.snapshot!,
            quotas: [{ ...liveState.providers.codex.snapshot!.quotas[0], usedPercent: 92 }],
          },
        },
      },
    };
    renderDashboard({ viewState });

    const summary = within(screen.getByRole('region', { name: 'Summary' }));
    expect(summary.getByText('near a limit')).toBeInTheDocument();
  });

  it('shows an empty state when no providers are enabled', () => {
    const settings = structuredClone(settingsState.settings);
    settings.providers = [];
    renderDashboard({ settings });

    expect(screen.getByRole('heading', { name: 'No providers enabled' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Open Customize' })).toBeInTheDocument();
  });

  it('offers a retry when a provider reports an error without a snapshot', () => {
    const state: ProviderViewState = {
      source: 'live',
      refreshing: false,
      stale: false,
      error: 'Codex is unreachable.',
      errorKind: null,
      lastAttemptAt: null,
      snapshot: null,
    };
    const onRefreshProvider = vi.fn();
    renderDashboard({ viewState: { providers: { codex: state } }, onRefreshProvider });

    expect(screen.getByText('Codex is unreachable.')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Retry' }));
    expect(onRefreshProvider).toHaveBeenCalledWith('codex');
  });

  it('wires the refresh, customize and settings controls', async () => {
    const props = renderDashboard();

    await fireEvent.click(screen.getByRole('button', { name: 'Refresh' }));
    expect(props.onRefreshAll).toHaveBeenCalledTimes(1);

    await fireEvent.click(screen.getByRole('button', { name: 'Refresh Codex' }));
    expect(props.onRefreshProvider).toHaveBeenCalledWith('codex');

    await fireEvent.click(screen.getByRole('button', { name: 'Customize' }));
    expect(props.onCustomize).toHaveBeenCalledTimes(1);

    await fireEvent.click(screen.getByRole('button', { name: 'Settings' }));
    expect(props.onSettings).toHaveBeenCalledTimes(1);
  });

  it('renders every enabled provider', () => {
    renderDashboard({
      viewState: { providers: { codex: liveState.providers.codex, claude: claudeState } },
      settings: {
        ...structuredClone(settingsState.settings),
        knownProviderIds: ['claude', 'codex'],
        providers: [
          ...structuredClone(settingsState.settings.providers),
          { id: 'claude', enabled: true, detected: true, expanded: false, metrics: [] },
        ],
      },
    });

    const providers = within(screen.getByRole('region', { name: 'Providers' }));
    expect(providers.getByRole('heading', { name: 'Codex' })).toBeInTheDocument();
    expect(providers.getByRole('heading', { name: 'Claude' })).toBeInTheDocument();
    expect(providers.getByText('Max')).toBeInTheDocument();
  });

  it('renders multiple discovered Cursor accounts independently', () => {
    const cursorCatalog = new ProviderCatalogIndex({
      apiKeyProviderIds: [],
      providers: [
        {
          id: 'cursor@1111aaaa',
          displayName: 'Cursor — 1111aaaa',
          shortName: 'Cu',
          fallbackEnabled: true,
          localUsageSourceNote: null,
          links: [],
          metrics: [],
        },
        {
          id: 'cursor@2222bbbb',
          displayName: 'Cursor — 2222bbbb',
          shortName: 'Cu',
          fallbackEnabled: false,
          localUsageSourceNote: null,
          links: [],
          metrics: [],
        },
      ],
    });
    const settings = {
      ...structuredClone(settingsState.settings),
      knownProviderIds: ['cursor@1111aaaa', 'cursor@2222bbbb'],
      providerNames: {
        'cursor@1111aaaa': 'Cursor Work',
        'cursor@2222bbbb': 'Cursor Personal',
      },
      providers: [
        { id: 'cursor@1111aaaa', enabled: true, detected: true, expanded: false, metrics: [] },
        { id: 'cursor@2222bbbb', enabled: true, detected: true, expanded: false, metrics: [] },
      ],
    };
    const viewState: UsageViewState = {
      providers: {
        'cursor@1111aaaa': {
          ...codexState,
          snapshot: { ...codexState.snapshot!, providerId: 'cursor@1111aaaa' },
        },
        'cursor@2222bbbb': {
          ...claudeState,
          snapshot: { ...claudeState.snapshot!, providerId: 'cursor@2222bbbb' },
        },
      },
    };
    renderDashboard({ catalog: cursorCatalog, settings, viewState });

    const providers = within(screen.getByRole('region', { name: 'Providers' }));
    expect(providers.getByRole('heading', { name: 'Cursor Work' })).toBeInTheDocument();
    expect(providers.getByRole('heading', { name: 'Cursor Personal' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Refresh Cursor Work' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Refresh Cursor Personal' })).toBeInTheDocument();
  });
});
