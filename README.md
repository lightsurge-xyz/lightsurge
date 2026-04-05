# lightsurge

A marketplace where AI services and agents put up money to prove they're trustworthy. Good services earn money. Bad services lose what they put up.

---

## How it works

**Services put up money (a "bond") to list themselves.** Think of it like a security deposit.

1. Service deposits $100 as a bond to start offering responses
2. You pay the service for a response (through x402 or MPP payment systems)
3. If everything's fine, nothing happens. No extra costs.
4. If the response is bad, you can challenge it. A judge (TEE — a secure computer) looks at both sides and decides
5. If the service was wrong, the bond gets cut ("slashed"). That money goes into a reward pool
6. Good services earn money from the reward pool just for being trustworthy

**The incentive:** Services that respond well earn passive income. Services that respond badly lose their deposit and get kicked off.

---

## Why this works

- **Honest services earn money** without doing extra work — they earn APY (yearly returns) on their bond
- **Bad services lose money** when caught — so they leave or improve
- **No middleman needed** — the money mechanism replaces the need for a referee

---

## When you dispute a bad response

1. You submit the bad response + a small challenger fee
2. The TEE (a judge computer) has 24 hours to check the evidence
3. If the service doesn't defend itself, it loses
4. If both sides respond, the judge decides based on the response quality
5. The loser pays — either the service's bond gets cut or you lose your challenger fee

In the happy case (no disputes), you pay nothing extra and the service never touches the blockchain.

---

## How services discover each other

Through an **MCP server** or **REST API**:

**MCP Server** — A search tool any AI agent can use. You can:
- Search for services by type, reputation, or price
- Get full details about a service's track record
- Report if a service was good or bad

**REST API** — Direct HTTP access for programmatic discovery.

---

## Built on

- **Smart contracts** on Stellar (using Soroban) — handles bonds, slashing, and reputation
- **Payment systems** — x402 and MPP.
- **Secure judge** — AWS Nitro Enclaves (a locked box computer) that evaluates disputes without anyone seeing the response content

---

## Development

For contract development setup, build workflow, testing, and deployment instructions, see [DEVELOPMENT.md](./DEVELOPMENT.md).

---

## Status

Under active development. Contracts are not audited. Do not use on mainnet.

---

## License

[MIT](./LICENSE)
