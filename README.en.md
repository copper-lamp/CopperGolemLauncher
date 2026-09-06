# CopperGolem Launcher · CopperCore

**CopperGolem** is a pluggable, modular launcher for Minecraft Bedrock Edition (MCBE).
**CopperCore** is its kernel — the skeleton and host of the launcher, responsible for module loading, capability services, the Shell UI, and inter-module coordination.

> The project is in early development. This repository hosts the core (`CopperCore`). The launcher ships with three built-in modules (**Home**, **Game Download**, **Content Download**); additional modules (Agent / MCP) are loaded dynamically.

---

## Features

- **Modular kernel**: built-in and additional modules are unified as "homogeneous frontend + backend plugins"; they are pluggable, isolated from each other, and coordinate only through the kernel.
- **Capability services**: i18n, theme tokens, SQLite persistence, download queue, MCBE account, software updater, filesystem abstraction.
- **Coordination mediator**: event bus (broadcast) + intent registry (request/response) for decoupled inter-module collaboration.
- **Shell UI**: custom frameless title bar, left navigation, content area, settings page, floating download dashboard.
- **Platform abstraction**: filesystem / download / storage capabilities are not tied to Windows, keeping a boundary for future Android support.

## Tech Stack

| Layer | Choice |
|---|---|
| Desktop shell | Tauri v2 (Rust backend + system WebView) |
| Backend | Rust (tokio async runtime) |
| Frontend | Vue 3 + TypeScript + Vite |
| Persistence | SQLite (rusqlite wrapper, versioned migrations) |
| Download engine | standalone Rust crate `copper-downloader` |

## Architecture

The launcher runs as a single desktop process, split into four layers:

1. **CopperCore** — the single Tauri app, infrastructure and host.
2. **Capability services** — globally unique facilities: i18n, theme tokens, database, download queue, account, updater, filesystem abstraction.
3. **Plugin registry** (with event bus / intent registry) — module loading and coordination.
4. **Module layer** — built-in modules compiled statically into the kernel; additional modules loaded dynamically.

Detailed design lives under the project `docs/` directory (architecture overview, CopperCore design, and per-module design docs).

## Directory Layout

```
CopperCore/
├─ frontend/        # Vue 3 Shell + frontend packages of built-in modules
├─ src-tauri/       # Rust backend (services / registry / modules mount points)
│   ├─ src/
│   ├─ capabilities/
│   ├─ icons/
│   ├─ Cargo.toml
│   └─ tauri.conf.json
├─ .github/workflows/   # build / release workflows
├─ LICENSE               # GPL-3.0
└─ package.json          # root entrypoint (forwards to frontend, hosts tauri CLI)
```

## Quick Start (Development)

Prerequisites: Node.js ≥ 20, Rust stable toolchain, Windows 10 / 11.

```bash
cd CopperCore
npm install                    # root dependencies (incl. tauri CLI)
npm --prefix frontend install  # frontend dependencies
```

| Command | Description |
|---|---|
| `npm run dev` | Frontend only (Vite HMR, port 1420) |
| `npm run tauri:dev` | Full app development (frontend + Rust) |
| `npm run build` | Build frontend only |
| `npm run tauri:build` | Build the full app |
| `cargo build --release --manifest-path src-tauri/Cargo.toml` | Build the Rust backend (release) only |

Testing:

```bash
cargo test --manifest-path src-tauri/Cargo.toml   # Rust unit / integration tests
```

> The full build & test pipeline is executed by CI (the `build` workflow) — see "Continuous Integration".

## Continuous Integration (GitHub Actions)

- **`build` workflow**: triggered on commits / pull requests — installs frontend deps, builds the frontend, and builds Rust (release) to keep the project buildable.
- **`release` workflow**: triggered when releasing a version — two ways:
  - **Automatic**: pushing a tag matching `v*`;
  - **Manual**: run from the Actions tab and fill in the version tag.
  It bundles Windows installers via `tauri-action` and creates a GitHub Release.

Versioning follows semver conventions; see [CHANGE.md](./CHANGE.md).

## i18n

Multilingual support is provided by the kernel capability service. Modules merge language packs under the `module.<module-name>.*` namespace; a missing key falls back to `en`, then to the key itself. Do not hardcode UI strings when developing modules.

## Theme Tokens

A unified set of CSS design tokens `var(--copper-*)` is provided, supporting dark / light / auto and applying globally in real time. Consume these tokens instead of hardcoding colors and spacing to keep a consistent style.

## License

This program is licensed under the [GPL-3.0](./LICENSE).
copyright © 2026 copper-lamp.

## Documentation

Architecture and per-module design documents are maintained under the project root `docs/` (development plan, architecture overview, CopperCore design, built-in & additional module designs). See the Requirements / Architecture / Notes sections of each document.