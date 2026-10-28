# cista

[![ci](https://github.com/c0v0/cista/actions/workflows/ci.yml/badge.svg)](https://github.com/c0v0/cista/actions/workflows/ci.yml)
[![build](https://github.com/c0v0/cista/actions/workflows/build.yml/badge.svg)](https://github.com/c0v0/cista/actions/workflows/build.yml)
[![release](https://img.shields.io/github/v/release/c0v0/cista?include_prereleases&sort=semver)](https://github.com/c0v0/cista/releases)
[![license: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

English | [简体中文](README.md)

Self-hosted, multi-platform notes with end-to-end encryption: a Rust server (axum + two SQLite
databases, `private.db` / `shared.db`) plus Tauri 2 clients (shared React SPA) for Linux, Windows and
Android, and a web client.

Private notes are E2E encrypted — the server only stores ciphertext. Shared content is readable by the
server (encrypted at rest). Archived workspaces become private again. Single tenant, single instance.

Design, decisions and acceptance criteria: [`docs/design.md`](docs/design.md),
[`docs/specs/`](docs/specs/), [`docs/adr/`](docs/adr/); documentation index in
[`docs/README.md`](docs/README.md) (Chinese).

## Features

| Area | What you get |
|---|---|
| Server | axum + two SQLite databases (`private.db` / `shared.db`), single binary; share routes hold only the shared-store pool |
| Private notes | per-request B+ (Ed25519) signatures, XChaCha20-Poly1305 E2E encryption, `If-Match` concurrency, incremental sync, device enrollment/revocation |
| Sharing | single-file / workspace links, optional passphrase, use limits and expiry, two-step download tickets, read-only enforcement, three-tier quotas, encrypted shared blobs |
| Collaboration | Yjs documents, `/ws` handshake (owner/share), read-only connections drop updates, snapshot + tail persistence |
| Operations | archive/quarantine/restore, `cista-server gc`, rate limits with backoff, structured redacted logs, CORS allowlist |
| Clients | one SPA for Web / Linux / Windows / Android: first-run server address setup, pairing, note editor, attachments, in-memory search index |
| Native | Tauri 2 shell + `plugins/keystore`: Secret Service / Credential Manager on desktop, AndroidKeyStore on Android; only the device seed is stored, the master key never enters OS credentials |

Tests: Rust **118** (plus 2 `#[ignore]` drill cases), Web **96** (plus 1 opt-in real-server e2e).
Gate: `./scripts/verify.sh` — web build → rustfmt → clippy → test → cargo-deny → web tests.

Data boundaries and deletion flow: [`docs/privacy.md`](docs/privacy.md) (Chinese).

## Quick start

```bash
cp .env.example .env            # at minimum CISTA_BOOTSTRAP_TOKEN / CISTA_STATIC_KEY
cargo run -p cista-server       # 127.0.0.1:8787 by default, data in ./data
./scripts/verify.sh             # same gate as CI (fmt/clippy/test/deny/web)
```

All configuration is via environment variables; there is no admin API or dashboard, operations are SSH only.

## Client: connecting to a server

Desktop and Android **must be given a server address on first launch** (a native WebView has no
"current site"), while the web client defaults to same-origin and can be switched to a cross-origin
address:

1. The app opens the "Connect to server" page; enter `https://<your-domain>` (a path prefix such as
   `https://example.com/cista` is allowed);
2. "Test connection" calls `<address>/healthz` and expects `ok`;
3. The address is stored in `localStorage` under `cista.server.base` (not a secret); changing it locks
   the current session;
4. Later changes are available from the footer or the pairing page.

Self-hosted web builds can bake in a default: `VITE_API_BASE=https://cista.example.com pnpm -C web build`.
For same-origin deployments, Caddy should serve `web/dist` and proxy the API — see the `handle` blocks in
[`deploy/Caddyfile`](deploy/Caddyfile).

## Deployment

- Default shape: single binary + systemd ([`deploy/cista.service`](deploy/cista.service)).
  Docker is supported as a wrapper: `docker compose up -d --build` (see `Dockerfile` / `compose.yaml`).
- Reverse proxy: [`deploy/Caddyfile`](deploy/Caddyfile) — TLS/HSTS, 110 MB request body limit, no WS
  buffering, JSON logs, CSP.
- Backup: `scripts/backup.sh` (`VACUUM INTO` both databases + blob manifest + restic), triggered daily by
  `deploy/cista-backup.{service,timer}`; failures land in the journal.
- Restore drill: `scripts/restore-drill.sh <empty-dir>`, record template in
  [`docs/specs/restore-drill-log.md`](docs/specs/restore-drill-log.md); the manual step (decrypt one note
  with the MK) must be filled in.
- Migration drill: `scripts/migration-drill.sh <empty-dir>` runs forward migrations on a copy and checks
  that the pre-migration snapshot still boots.
- Desktop packaging: `scripts/package-desktop.sh [deb|deb,appimage]` (builds `web/dist` first, then calls
  the Tauri CLI; artifacts under `target/release/bundle/`). Packages are not committed.
- CI: `.github/workflows/build.yml` builds the server (Linux/Windows), desktop packages and a signed APK,
  and publishes release assets on `v*` tags.
- Verify release assets (download `cista-release-public.asc`, `SHA256SUMS`, `SHA256SUMS.asc`):
  ```bash
  gpg --import cista-release-public.asc
  gpg --verify SHA256SUMS.asc SHA256SUMS
  sha256sum -c SHA256SUMS --ignore-missing
  # verify a single .deb detached signature
  gpg --verify Cista_0.1.0_debian13_amd64.deb.asc Cista_0.1.0_debian13_amd64.deb
  ```
- Shared blobs at rest: `CISTA_STATIC_KEY` (32 bytes hex) is required and must be stored separately from
  the other keys; without it the server refuses to start (local development only: `CISTA_ALLOW_PLAINTEXT=1`, ADR 0009).
- Upgrade / rollback:
  - Upgrade: stop service → back up → replace the binary (`systemctl stop cista` → install
    `/usr/local/bin/cista-server` → `systemctl start cista`); migrations run forward at startup.
  - Rollback: restore the previous binary **and** the pre-upgrade backup (migrations are forward-only).
  - Versions: [`CHANGELOG.md`](CHANGELOG.md).
- Availability: external uptime check hits `/healthz` every 5 minutes.
- Logs: structured JSON to journald/Docker logs, ~30 days rotation; no note bodies, no plaintext tokens
  (share tokens travel only in the `Authorization: Share` header and are logged as a truncated
  `sha256:` fingerprint).

## Three keys (keep them separate)

| Key | If lost | Where to store |
|---|---|---|
| MK (private master key) | Private data is unrecoverable | Offline recovery code |
| Static encryption key (shared blobs) | Shared blobs cannot be decrypted | Offline custody |
| Backup repository password | All backups are unreadable | Offline custody |

## Repository layout

```text
crates/core       protocol / crypto / B+ auth / sync state machine / client index (no IO, wasm-capable)
crates/server     axum API, private + shared databases, shared blobs
apps/desktop      Tauri 2 shell (reuses web/)
web/              React + Vite + TypeScript SPA (includes the server-address setup page)
plugins/keystore  Tauri mobile plugin: Android Keystore adapter
docs/             design / privacy / ADRs / specs (phase specs and acceptance)
scripts/          verify.sh, backup.sh, restore-drill.sh, packaging and drill scripts
deploy/           systemd units/timer, Caddyfile (static site + API proxy)
.github/          CI, build/release workflows, dependabot, issue/PR templates, CODEOWNERS
Dockerfile        server image (multi-stage); compose.yaml for a single-instance deployment
```

## Contributing and legal

- Contribution flow and gates: [`CONTRIBUTING.md`](CONTRIBUTING.md) (Chinese)
- Code of conduct: [`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md) (Chinese)
- Security reports (do **not** open a public issue): [`SECURITY.md`](SECURITY.md)
- Changes: [`CHANGELOG.md`](CHANGELOG.md)
- License: MIT — see [`LICENSE`](LICENSE)
