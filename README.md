<!-- Project README following requested template style -->
<p align="center">
  <img alt="logo" src="/web/public/icons/favicon.png" width="300" height="300" />

  <h1 align="center">SolvEx</h1>
  <p align="center"><strong>High-performance Verifiable CEX &amp; Orderbook Matching Engine — Rust</strong></p>

  <p align="center">
    <a href="#"><img alt="build" src="https://img.shields.io/badge/build-passing-brightgreen.svg"></a>
    <a href="#"><img alt="coverage" src="https://img.shields.io/badge/coverage-~%25-yellowgreen.svg"></a>
    <a href="#"><img alt="license" src="https://img.shields.io/badge/license-MIT-blue.svg"></a>
    <a href="#"><img alt="docker" src="https://img.shields.io/badge/docker-compose-blue.svg"></a>
  </p>
</p>

---

All-in-one monorepo containing a Rust in-memory matching engine, async Postgres persistence, and ZK proof tooling.

Development/test only: identity is declared through `x-user-id` and remains impersonable. The default mode is read-only `maintenance`; opt in to `CEX_MODE=demo` only with disposable data and configure `CEX_ADMIN_USER_IDS` on the server. Financial durability/recovery gates are still pending. ZKP responses are prototypes: the UI does not claim verified inclusion, aggregate liabilities, reserve evidence or solvency. Admin snapshot records are simulated/unverified. See `docs/IMPLEMENTATION_PROGRESS.md` and `docs/PROOF_STATUS.md`.

## Quick Links

- `core/` — Rust backend (engine, API, db, observability)
- `web/` — Svelte + Vite SPA (client)
- `zkp/` — Merkle Sum Tree and ZK circuit primitives
- `docs/techstack.md` — architecture & library choices
- `docs/wbs.csv` — current roadmap / status
- `docker-compose.yml` — local runtime (Postgres, backend, web, prometheus, grafana)

## Getting Started

Start the full local runtime using Docker Compose (recommended):

```powershell
# First time only
Copy-Item .env.example .env

# Migrations apply automatically; preserve existing database volumes.

# Build and start all services
docker compose up -d --build
docker compose ps
```

Quick verification:

```powershell
Invoke-RestMethod http://localhost:8080/health
Invoke-RestMethod "http://localhost:8080/api/orderbook?symbol=BTC_USDT"
```

Stop the runtime:

```powershell
docker compose down
```

## Features

- Deterministic in-memory matching engine using `BTreeMap` price levels
- Accurate financial math using `rust_decimal` (no `f32`/`f64` for money)
- REST API (Axum) + WebSocket real-time feed (`/ws`)
- Async persistence worker (non-blocking writes to PostgreSQL via `sqlx`)
- ZKP Solvency tooling (Merkle Sum Tree, prover, wasm verifier)

## API & WebSocket (summary)

Key endpoints (high-level):

- `GET /health` — liveness probe
- `GET /metrics` — Prometheus exposition
- `GET /api/orderbook` — depth snapshot
- `POST /api/orders` — place limit order
- `DELETE /api/orders/:id` — cancel order
- `GET /api/balances` — user balances (requires `x-user-id` header)
- `GET /ws` — WebSocket upgrade for real-time events
- `GET /api/zkp/proof` — user solvency proof
- `GET /api/zkp/solvency` — exchange-wide solvency summary

For the full list, see the API handlers in `core/src/api/`.

## Development

Backend (run from repo root):

```powershell
cargo run -p core --bin core
```

Frontend (Svelte):

```powershell
cd web
npm install
npm run dev
```

ZKP crate tests:

```powershell
cargo test -p zkp
```

## Testing & Benchmark

```powershell
# Run all tests
cargo test

# Core-specific tests
cargo test -p core

# Criterion benchmarks
cargo bench -p core --bench engine_benchmark
```

## Observability

- Prometheus: `http://localhost:9090`
- Grafana: `http://localhost:3001` (default `admin` / `admin`)
- Backend metrics: `GET /metrics`


## Contributing

- Follow Conventional Commits (`feat`, `fix`, `docs`, `chore`, `test`, `perf`, `refactor`).
- Keep core engine deterministic and sync-only; avoid I/O inside matching code.
- Never use `f32`/`f64` for financial values — use `rust_decimal`.
- Add tests (unit/proptest) for any change touching engine logic.

See [docs/wbs.csv](docs/wbs.csv) and [docs/techstack.md](docs/techstack.md) for design constraints.

## License

This project is licensed under the MIT License — see the [LICENSE](LICENSE) file.

