# Getting Started — Contract Development

This guide covers the local setup, build workflow, testing, and deployment for lightsurge Soroban contracts.

---

## Prerequisites

### Rust

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustup target add wasm32-unknown-unknown
```

### Stellar CLI

```bash
cargo install --locked stellar-cli
```

Verify:

```bash
stellar --version
```

### Docker (optional, for local network)

Required only if you want a fully local Stellar node. [Install Docker](https://docs.docker.com/get-docker/).

---

## Repository Layout

```
lightsurge/
├── contracts/
│   ├── registry/       # Service registration & bond lifecycle
│   └── escalation/     # Dispute filing & resolution
├── packages/           # Off-chain TypeScript packages (MCP server, SDK)
├── Cargo.toml          # Workspace root
└── docs/
```

Each contract is an independent Cargo crate inside the workspace. The workspace `Cargo.toml` at the root manages shared dependencies and release profile settings.

---

## Build

From the workspace root, build all contracts:

```bash
stellar contract build
```

This compiles every crate in `contracts/*` to optimized WASM under:

```
target/wasm32v1-none/release/<contract_name>.wasm
```

To build a single contract, `cd` into its directory first:

```bash
cd contracts/registry
stellar contract build
```

> **Why `stellar contract build` instead of `cargo build`?**
> It sets the correct target (`wasm32-unknown-unknown`), applies the release profile optimizations defined in `Cargo.toml`, and strips debug symbols — all in one command.

---

## Test

Unit tests compile to native Rust (not WASM), so they run fast without any network.

```bash
# From workspace root — runs all contracts
cargo test

# Single contract
cd contracts/registry && cargo test

# With stdout (useful for debugging)
cargo test -- --nocapture
```

Tests live inside each contract's `src/lib.rs` under `#[cfg(test)]`, and use `soroban-sdk`'s `testutils` feature to simulate the Soroban environment, ledger state, and authorization without spinning up a node.

---

## Networks

| Network  | RPC Endpoint                              | Passphrase                              | Friendbot                            |
|----------|-------------------------------------------|-----------------------------------------|--------------------------------------|
| Local    | `http://localhost:8000/soroban/rpc`       | `Standalone Network ; February 2017`   | `http://localhost:8000/friendbot`    |
| Testnet  | `https://soroban-testnet.stellar.org`     | `Test SDF Network ; September 2015`    | `https://friendbot.stellar.org`      |
| Mainnet  | `https://mainnet.stellar.validationcloud.io/v1/<key>` | `Public Global Stellar Network ; September 2015` | N/A |

### Start a local network

```bash
stellar container start local
```

Or with Docker directly:

```bash
docker run --rm -it -p 8000:8000 \
  --name stellar \
  stellar/quickstart:latest \
  --local \
  --enable-soroban-rpc
```

---

## Identities (Signing Keys)

Before deploying you need a funded identity. The Stellar CLI manages identities globally across projects.

```bash
# Create and fund a testnet identity
stellar keys generate --global alice --network testnet --fund

# List identities
stellar keys ls

# Check balance
stellar keys show alice
```

For local network:

```bash
stellar keys generate --global alice --network local --fund
```

---

## Deploy

### Testnet

```bash
stellar contract deploy \
  --wasm target/wasm32v1-none/release/lightsurge_registry.wasm \
  --source alice \
  --network testnet
```

This prints a contract ID starting with `C`. Save it — you'll need it for invocations.

### With Constructor Arguments (Protocol 22+)

lightsurge contracts use `__constructor` for atomic initialization at deploy time:

```bash
stellar contract deploy \
  --wasm target/wasm32v1-none/release/lightsurge_registry.wasm \
  --source alice \
  --network testnet \
  -- \
  --admin alice \
  --token STELLAR_ASSET_CONTRACT_ID
```

Arguments after `--` are passed to `__constructor`. If the constructor fails, the whole deployment rolls back.

### Local network

Same command, swap `--network testnet` for `--network local`.

---

## Invoke

```bash
stellar contract invoke \
  --id CONTRACT_ID \
  --source alice \
  --network testnet \
  -- \
  function_name \
  --arg_name value
```

### Simulate only (no broadcast)

```bash
stellar contract invoke \
  --id CONTRACT_ID \
  --source alice \
  --network testnet \
  --sim-only \
  -- \
  function_name
```

Use `--sim-only` to preview resource costs and return values without submitting a transaction.

---

## Upgrade

To upgrade a contract's WASM after deploy:

```bash
# 1. Install new WASM (get the hash)
stellar contract install \
  --wasm target/wasm32v1-none/release/lightsurge_registry.wasm \
  --source alice \
  --network testnet

# 2. Upgrade the deployed instance to the new hash
stellar contract invoke \
  --id CONTRACT_ID \
  --source alice \
  --network testnet \
  -- \
  upgrade \
  --new_wasm_hash WASM_HASH
```

> The `upgrade` entrypoint must be implemented in the contract and protected by admin auth. Constructors do **not** re-run on upgrade — any migration logic must be handled inside the upgrade function.

---

## Storage & TTL

Soroban storage expires. Each contract manages its own TTL extension or entries get archived.

| Type        | Scope              | Survives archival | Best for                        |
|-------------|--------------------|-------------------|---------------------------------|
| Instance    | Whole contract     | Yes               | Admin address, global config    |
| Persistent  | Per key            | Yes (restorable)  | Bond amounts, reputation scores |
| Temporary   | Per key            | No                | Caches, short-lived flags       |

TTL is measured in ledgers (~5 seconds each). To keep a persistent entry alive for ~30 days:

```rust
env.storage().persistent().extend_ttl(&key, 17280, 518400);
//                                          ^min   ^extend_to
```

---

## Contract Addresses (Testnet)

> Fill these in after initial deployment.

| Contract    | Testnet ID |
|-------------|------------|
| Registry    | —          |
| Escalation  | —          |

---

## Common Commands Reference

```bash
# Build all contracts
stellar contract build

# Run all tests
cargo test

# Start local Stellar node
stellar container start local

# Generate & fund a testnet key
stellar keys generate --global alice --network testnet --fund

# Deploy to testnet
stellar contract deploy --wasm <path>.wasm --source alice --network testnet

# Invoke a function
stellar contract invoke --id <CONTRACT_ID> --source alice --network testnet -- <fn> [--args]

# Simulate (no broadcast)
stellar contract invoke --id <CONTRACT_ID> --source alice --network testnet --sim-only -- <fn>

# Fetch deployed WASM for differential testing
stellar contract fetch --id <CONTRACT_ID> --out-file deployed.wasm
```

---

## Testnet Reset

Testnet resets approximately quarterly. All deployed contracts and accounts are wiped. After a reset:

1. Re-fund your identity: `stellar keys fund alice --network testnet`
2. Redeploy both contracts
3. Update contract addresses in `.env` / config files
