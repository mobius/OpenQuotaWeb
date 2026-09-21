import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import ProviderCredentialsSection from './ProviderCredentialsSection.svelte';

const mocks = vi.hoisted(() => ({
  saveProviderCredentials: vi.fn().mockResolvedValue(undefined),
}));

vi.mock('./backend', () => ({
  saveProviderCredentials: mocks.saveProviderCredentials,
}));

describe('ProviderCredentialsSection', () => {
  afterEach(() => {
    cleanup();
    mocks.saveProviderCredentials.mockClear();
  });

  it('matches suffixed provider IDs by family for credential import', async () => {
    render(ProviderCredentialsSection, {
      providerId: 'claude@1234abcd',
      providerName: 'Claude — Work',
    });

    expect(screen.getByText('~/.claude/.credentials.json')).toBeInTheDocument();
    await fireEvent.input(screen.getByLabelText('Claude — Work credentials'), {
      target: { value: '{"access_token":"abc"}' },
    });
    await fireEvent.click(screen.getByRole('button', { name: 'Save credentials' }));

    await waitFor(() =>
      expect(mocks.saveProviderCredentials).toHaveBeenCalledWith(
        'claude',
        '{"access_token":"abc"}',
      ),
    );
  });
});
