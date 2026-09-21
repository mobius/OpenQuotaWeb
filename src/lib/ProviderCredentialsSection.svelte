<script lang="ts">
  import { saveProviderCredentials } from './backend';
  import { providerFamily } from './providerIconPaths';

  interface Props {
    providerId: string;
    providerName: string;
  }
  let { providerId, providerName }: Props = $props();
  const providerKey = $derived(providerFamily(providerId));

  const targets: Record<string, string> = {
    codex: '~/.codex/auth.json',
    claude: '~/.claude/.credentials.json',
    opencode: '~/.local/share/opencode/auth.json',
    antigravity: 'Antigravity auth.json',
  };

  const supported = $derived(providerKey in targets);
  let content = $state('');
  let status = $state<'idle' | 'saving' | 'saved' | 'error'>('idle');
  let error = $state('');

  async function save() {
    if (!content.trim() || status === 'saving') return;
    status = 'saving';
    error = '';
    try {
      await saveProviderCredentials(providerKey, content);
      content = '';
      status = 'saved';
    } catch (cause) {
      status = 'error';
      error = cause instanceof Error ? cause.message : String(cause);
    }
  }
</script>

{#if supported}
  <div class="metric-section credentials-section">
    <h2>Sign in</h2>
    <p class="credentials-help">
      Paste the contents of your <code>{targets[providerKey]}</code> file from the computer where
      {providerName} is already signed in.
    </p>
    <textarea
      class="credentials-input"
      bind:value={content}
      rows="4"
      spellcheck="false"
      placeholder={'{ "access_token": "..." }'}
      aria-label={`${providerName} credentials`}></textarea>
    <button
      class="credentials-save"
      type="button"
      onclick={save}
      disabled={status === 'saving' || !content.trim()}
    >
      {status === 'saving' ? 'Saving…' : 'Save credentials'}
    </button>
    {#if status === 'saved'}
      <p class="credentials-ok" role="status">Saved. Refreshing usage…</p>
    {/if}
    {#if status === 'error'}
      <p class="credentials-error" role="alert">{error}</p>
    {/if}
  </div>
{/if}

<style>
  :global {
    .credentials-help {
      margin: 0 0 8px;
      color: var(--secondary);
      font-size: 11px;
      line-height: 15px;
    }

    .credentials-help code {
      color: var(--text);
      font-size: 10px;
    }

    .credentials-input {
      width: 100%;
      margin-bottom: 8px;
      padding: 8px;
      border: 1px solid var(--separator);
      border-radius: 8px;
      color: var(--text);
      background: color-mix(in srgb, var(--card) 72%, transparent);
      font-family: ui-monospace, monospace;
      font-size: 11px;
      resize: vertical;
    }

    .credentials-save {
      padding: 6px 10px;
      border: 1px solid var(--separator);
      border-radius: 8px;
      color: var(--text);
      background: color-mix(in srgb, var(--card) 72%, transparent);
      font-size: 11px;
      cursor: pointer;
    }

    .credentials-save:disabled {
      opacity: 0.5;
      cursor: default;
    }

    .credentials-ok {
      margin: 8px 0 0;
      color: #34c759;
      font-size: 11px;
    }

    .credentials-error {
      margin: 8px 0 0;
      color: var(--error);
      font-size: 11px;
    }
  }
</style>
