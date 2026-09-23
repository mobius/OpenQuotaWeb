<p align="center">
  <img src="assets/openquota-icon.png" alt="OpenQuota logo" width="88">
</p>

<h1 align="center">OpenQuota Web</h1>

<p align="center">
  Track usage and limits across your AI coding tools, from your browser.
</p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="MIT license"></a>
</p>

OpenQuota Web runs the OpenQuota provider logic as a small HTTP server and serves
the dashboard over HTTP, so there is no desktop app, VNC, X11, or WebKit
required. Run it with Docker and open it in any browser.

```
Browser  ──HTTP/SSE──►  openquota-server (Rust/Axum)  ──►  provider APIs
                               │                         (reqwest)
                               ├── SQLite (history and settings)
                               ├── CLI credential homes (Codex, Claude, OpenCode)
                               └── API-key store (0600)
```

No provider was rewritten: the shared logic from the Tauri binary is exposed
through an Axum server that offers the same commands as the desktop app.

## Quick start

Requirements: Docker with the Compose plugin.

```sh
cp .env.example .env          # optional: set auth and API keys
docker compose up -d --build
```

Open <http://localhost:8080/>.

The first build compiles the frontend (Node) and the server (Rust) and can take
a few minutes. After that, startup is fast.

```sh
docker compose ps        # should show "healthy"
docker compose logs -f   # server logs
```

## Configuration

Copy `.env.example` to `.env` and edit it. **Never commit the populated `.env`.**

| Variable                      | Default   | Description                                 |
| ----------------------------- | --------- | ------------------------------------------- |
| `OPENQUOTA_AUTH_USER`         | _(empty)_ | HTTP Basic username (optional).             |
| `OPENQUOTA_AUTH_PASSWORD`     | _(empty)_ | HTTP Basic password (optional).             |
| `OPENROUTER_API_KEY`          | _(empty)_ | OpenRouter API key.                         |
| `DEEPSEEK_API_KEY`            | _(empty)_ | DeepSeek API key.                           |
| `ZAI_API_KEY` / `GLM_API_KEY` | _(empty)_ | Z.ai API key.                               |
| `KIMI_API_KEY`                | _(empty)_ | Kimi API key.                               |
| `MINIMAX_API_KEY`             | _(empty)_ | MiniMax API key.                            |
| `OPENQUOTA_CURSOR_STATE_DBS`  | _(auto)_  | Comma-separated Cursor `state.vscdb` paths. |
| `OPENQUOTA_CURSOR_STATE_DB`   | _(auto)_  | Single Cursor `state.vscdb` path (legacy).  |

If you set both `OPENQUOTA_AUTH_USER` and `OPENQUOTA_AUTH_PASSWORD`, the whole
dashboard is protected with HTTP Basic (`/api/health` stays open for the
healthcheck). The browser remembers the credentials after the first prompt.

For Cursor account discovery, OpenQuota auto-detects common `state.vscdb`
locations. Set `OPENQUOTA_CURSOR_STATE_DBS` when you want explicit control over
multiple Cursor accounts (for example:
`/path/a/state.vscdb,/path/b/state.vscdb`).

Server variables (usually no need to change):

| Variable                 | Default                                       | Description        |
| ------------------------ | --------------------------------------------- | ------------------ |
| `OPENQUOTA_HOST`         | `0.0.0.0`                                     | Listen interface.  |
| `OPENQUOTA_PORT`         | `8080`                                        | Port.              |
| `OPENQUOTA_STATIC_DIR`   | `/app/dist`                                   | Compiled frontend. |
| `OPENQUOTA_APP_DATA_DIR` | `$XDG_DATA_HOME/io.github.deviffyy.openquota` | Data directory.    |

## Credentials

**CLI providers (Codex, Claude, OpenCode, Antigravity).** In the dashboard go to
**Customize → (provider) → Sign in** and paste the contents of the credential
file from your machine. The server writes it with `0600` permissions and enables
the provider automatically.

| Provider    | File to paste (on your machine)     |
| ----------- | ----------------------------------- |
| Codex       | `~/.codex/auth.json`                |
| Claude      | `~/.claude/.credentials.json`       |
| OpenCode    | `~/.local/share/opencode/auth.json` |
| Antigravity | Antigravity's `auth.json`           |

Alternatively, mount the host directories. Add a local, gitignored
`docker-compose.override.yml`:

```yaml
services:
  openquota:
    volumes:
      - ${HOME}/.codex:/data/home/.codex
      - ${HOME}/.claude:/data/home/.claude
      - ${HOME}/.local/share/opencode:/data/xdg-data/opencode
```

Use `${USERPROFILE}` instead of `${HOME}` on Windows.

**API-key providers (OpenRouter, DeepSeek, Z.ai, Kimi, MiniMax).** Add the key in
**Customize** (stored in `secrets.json`, `0600`, inside the persistent volume) or
set the matching environment variable in `.env`. A key saved in the UI takes
precedence over the environment one.

## Persistence and backup

| Volume                       | Mount     | Contents                                  |
| ---------------------------- | --------- | ----------------------------------------- |
| `openquota_openquota-data`   | `/data`   | SQLite, prices, file key store, CLI homes |
| `openquota_openquota-config` | `/config` | `$XDG_CONFIG_HOME` (per-provider configs) |

Everything survives `docker compose restart`. The backup contains credentials,
so store it securely.

```sh
# Backup
docker run --rm -v openquota_openquota-data:/data -v openquota_openquota-config:/config \
  -v "$PWD:/backup" alpine tar czf /backup/openquota-backup.tgz -C / data config

# Restore
docker compose down
docker run --rm -v openquota_openquota-data:/data -v openquota_openquota-config:/config \
  -v "$PWD:/backup" alpine sh -c 'rm -rf /data/* /config/* && tar xzf /backup/openquota-backup.tgz -C /'
docker compose up -d
```

## Security

- There is **no authentication by default**. Use it on localhost/LAN, or set
  `OPENQUOTA_AUTH_USER` / `OPENQUOTA_AUTH_PASSWORD`.
- Do not expose port 8080 directly to the internet. Put a TLS reverse proxy in
  front (nginx, Traefik, Caddy) and authenticate there.
- To publish on localhost only, map the port as `"127.0.0.1:8080:8080"`.
- API keys and tokens are never written to the logs.

## Release integrity

The original desktop installers are still published by the release workflow, so
their trust model is unchanged:

- Update payloads are cryptographically signed with the Tauri updater key, and
  OpenQuota refuses unsigned or tampered updates.
- Windows installers are Authenticode-signed when native signing is enabled;
  without it the build is unsigned and SmartScreen may warn.
- macOS builds are Developer ID-signed and notarized when native signing is
  enabled; otherwise they use an ad-hoc signature and Gatekeeper may require
  manual approval.

See [docs/releasing.md](docs/releasing.md) for the signing configuration.

## API

The server exposes the same commands the desktop app used:

| Method         | Route                             | Tauri equivalent                         |
| -------------- | --------------------------------- | ---------------------------------------- |
| GET            | `/api/bootstrap`                  | `get_bootstrap_state`                    |
| GET/PUT        | `/api/settings`                   | `get_app_settings` / `save_app_settings` |
| POST           | `/api/settings/reset-*`           | reset customization / all / per provider |
| POST           | `/api/usage/refresh[/{id}]`       | refresh usage                            |
| POST           | `/api/codex/reset-claim`          | `claim_codex_reset_credit`               |
| GET/PUT/DELETE | `/api/providers/{id}/api-key`     | provider API key                         |
| PUT            | `/api/providers/{id}/credentials` | import CLI credentials                   |
| GET            | `/api/events`                     | `usage-state` / `settings-state` (SSE)   |
| GET            | `/api/health`                     | healthcheck                              |

## Development

Requirements: Node 22 + pnpm 11.11.0, stable Rust.

```sh
corepack pnpm install --frozen-lockfile

cargo run --manifest-path src-tauri/Cargo.toml \
  --no-default-features --features web --bin openquota-server
```

The server serves the compiled frontend from `OPENQUOTA_STATIC_DIR`. For
development, run `corepack pnpm build` and restart the server, or serve `dist`
with your own tool and point `/api` at the server.

The original desktop build still works (`corepack pnpm tauri dev`).

## More documentation

See [README-Docker.md](README-Docker.md) for the full architecture notes,
provider matrix, known limitations, and verification details.

## Acknowledgements and license

OpenQuota Web is based on [deviffyy/OpenQuota](https://github.com/deviffyy/OpenQuota)
(MIT). The provider logic is reused as-is; this repository adds the HTTP server
and web build. Licensed under [MIT](LICENSE).
