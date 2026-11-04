# Tech Stack & Architecture Specification
**Project:** SolvEx — High-Performance Verifiable CEX & Order Book Matching Engine
**Status:** Architecture locked. AI Assistant MUST strictly adhere to these libraries and design decisions.

## 0. Project Directory Structure (Monorepo)
The repository is structured as a monorepo aligning with our Git branches (`core`, `web`, `zkp`, `docs`). The AI must place code strictly in the corresponding directories.

/ (Root)
├── .github/
│   └── copilot-instructions.md  # Strict AI System Instructions
├── docs/                        # Architecture documentation
│   ├── README.md
│   └── techstack.md
├── core/                        # Backend: Rust Matching Engine + API
│   ├── Cargo.toml
│   ├── src/
│   │   ├── main.rs              # Axum server entry point
│   │   ├── engine/              # Orderbook matching logic (BTreeMap)
│   │   ├── api/                 # REST & WebSocket handlers
│   │   └── db/                  # Database operations (Postgres, sqlx)
│   └── benches/                 # Criterion benchmarking
├── web/                         # Frontend: Svelte + Vite GUI Simulator
│   ├── package.json
│   ├── src/
│   │   ├── components/          # Orderbook UI, Trade Form
│   │   ├── stores/              # Svelte stores for real-time WS data
│   │   └── lib/                 # Wasm verifier integration
├── zkp/                         # ZK-Proof: Merkle Sum Tree & Wasm Prover/Verifier
│   ├── Cargo.toml
│   ├── src/
│   │   ├── lib.rs               # Wasm bindings (wasm-pack)
│   │   ├── circuit.rs           # ZK logic (arkworks/halo2)
│   │   └── tree.rs              # Merkle Sum Tree builder
│   └── tests/                   # Proptest for tree generation
└── docker-compose.yml           # Local deployment (Postgres, Core, Web)

## 1. ENGINE (Matching Core & Backend API)
- **Architecture:** Monolith. Single Rust process.
- **Language:** Rust (Stable)
- **Async Runtime:** `tokio` (features: `["full"]`)
- **Web Framework:** `axum` (REST API & WebSocket server)
- **Financial Math (CRITICAL):** `rust_decimal` (features: `["serde-float"]`). **Absolutely NO `f32`/`f64`.**
- **In-Memory State:** `std::collections::BTreeMap` for the Order Book. `std::collections::HashMap` for quick order/user lookups.
- **Concurrency:** `std::sync::Arc` and `tokio::sync::RwLock` (or `parking_lot::RwLock`) to share the engine state across async Axum handlers.
- **Serialization:** `serde`, `serde_json` (features: `["derive"]`)
- **Logging:** `tracing`, `tracing-subscriber`

## 2. DATABASE (The Ledger)
- **Purpose:** Async persistence only (snapshots, trade history, user balances). Does NOT block the matching engine.
- **Database Engine:** PostgreSQL
- **Driver / Query Builder:** `sqlx` (features: `["runtime-tokio", "postgres", "chrono", "decimal", "uuid"]`). 
- **Constraint:** NO ORMs (like Diesel). Write raw, compile-time checked SQL queries using `sqlx::query!`.
- **ID Generation:** `uuid` (v4)

## 3. FRONTEND (GUI Simulator)
- **Purpose:** A lightweight Graphical User Interface to visualize the Order Book and submit trades. Contains NO business logic.
- **Framework:** Svelte (v4 or v5 runes) + **Vite (Single Page Application)**.
- **Constraint:** STRICTLY NO SvelteKit (to avoid SSR and WebAssembly conflicts).
- **Styling:** Tailwind CSS (Dark mode optimized for trading terminals).
- **Real-time Updates:** Native `WebSocket` API for listening to Axum events.
- **Authentication:** Dummy Auth. Pass `x-user-id` (e.g., `1`, `2`) in HTTP headers. No JWT or Web3 wallets.

## 4. ZK-PROOF (Solvency)
- **Purpose:** Generate and verify a Proof of Solvency using a Merkle Sum Tree.
- **ZK Framework:** `arkworks` ecosystem (or `halo2`).
- **Hash Function:** Poseidon Hash (ZK-friendly hash).
- **Wasm Compilation:** `wasm-pack` (Compiles the Rust verification logic to WebAssembly for the client browser).

## 5. TESTING & BENCHMARKING (Quality Assurance)
- **Unit/Integration:** `cargo test`, `tokio-test`.
- **Property Testing:** `proptest` (Fuzzing the matching engine with random orders to guarantee memory safety and exact matching math).
- **Performance:** `criterion` (Measuring order matching latency to maintain micro-second performance).

## 6. INFRASTRUCTURE & DEPLOYMENT
- **Containerization:** Docker & Docker Compose (to run PostgreSQL, the Rust Engine, and the Svelte UI together consistently across environments).
- **Reverse Proxy / Web Server:** Nginx.
  - Serves the built Svelte SPA static files (`.html`, `.js`, `.css`, `.wasm`).
  - Routes and load-balances `/api` and `/ws` requests to the Rust Axum backend.
- **Environment Management:** `dotenvy` (for managing `DATABASE_URL` and other secrets in the Rust environment).