# Repository Guidelines

## Project Structure & Module Organization
- `rs/` — Rust workspace for the CLI binaries and shared library (`rs/src/`, `rs/src/bin/<name>/`).
- `browser/` — SvelteKit 2 + Svelte 5 SPA (routes in `browser/src/routes/`, components in `browser/src/lib/`).
- `python/` — Utility scripts and helper tooling (`generate.py`, `resolve.py`, `statwalker/`).
- `script/` — Repo automation helpers (`vendor.sh`, `bench.py`).
- `doc/` — Documentation such as `doc/white_paper.md`.

## Build, Test, and Development Commands
- `cd rs && cargo build --release` — Build all Rust binaries (e.g., `duscan`, `dusum`, `duapi`).
- `cd rs && cargo test` — Run Rust tests.
- `cd rs && cargo check` — Fast type check without full build.
- `cd rs && cargo clippy` — Lint Rust code.
- `cd browser && npm install` — Install frontend dependencies.
- `cd browser && npm run dev` — Start the dev server (port 5173).
- `cd browser && npm run build` — Production build (outputs to `browser/build/`).
- `cd browser && npm run check` — TypeScript/Svelte type checking.

## Coding Style & Naming Conventions
- Rust: follow standard Rust style; format with `cargo fmt` when needed and keep modules focused.
- Svelte/TypeScript: prefer PascalCase for components (`Login.svelte`) and lowercase for TS modules (`api.svelte.ts`).
- Keep binaries under `rs/src/bin/<name>/` with clear, single-purpose modules.

## Testing Guidelines
- Primary tests are run via `cargo test` in `rs/`.
- When adding tests, use Rust’s built-in test framework and name tests clearly by behavior (e.g., `test_parse_scan_row`).
- Frontend checks run via `npm run check`.

## Commit & Pull Request Guidelines
- Recent history favors short, imperative messages (e.g., “Refactor …”, “Add …”). Version bumps use `chore: bump version to X.Y.Z`.
- Include a clear PR description, the affected pipeline stage(s) (`duscan`, `dusum`, `duapi`, UI), and any relevant commands run.
- For frontend changes, include before/after screenshots or a brief screen recording.

## Security & Configuration Tips
- API auth is JWT-based. Set `JWT_SECRET` for `duapi` and optionally `ADMIN_GROUP` for admin gating.
- TLS support exists in `rs/` (see `cert.pem`/`key.pem` for local testing).

## Architecture Overview
Pipeline: `duscan` → `dusum` → `duapi` → Svelte SPA. Optional tools: `duhuman` (humanize output) and `duzip` (CSV ↔ Zstandard).
