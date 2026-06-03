# How to Vet a Solana RPC Endpoint for Your Workload

Most "is the RPC up?" checks answer the wrong question. An endpoint can return
`200 OK` and still be a bad fit for a bot, indexer, or wallet backend. This is a
short, practical guide to checking whether a Solana RPC endpoint is actually
ready for *your* workload, using [`sol-doctor`](https://crates.io/crates/solana-infra-doctor)
— a local-first Rust CLI.

```bash
cargo install solana-infra-doctor
```

## 1. Is it usable at all?

```bash
sol-doctor check --rpc https://api.mainnet-beta.solana.com
```

This runs the core JSON-RPC methods (`getHealth`, `getVersion`, `getGenesisHash`,
`getSlot`, `getLatestBlockhash`, `isBlockhashValid`,
`getRecentPerformanceSamples`), measures per-call latency, and returns a verdict
(`GOOD` / `WARNING` / `BAD` / `UNKNOWN`) with exit codes `0/1/2/3`. It tells you
whether blockhashes are usable and slot data is fresh — not just whether the
socket opened.

## 2. Which of two endpoints is better — for this workload?

```bash
sol-doctor compare \
  --rpc https://api.mainnet-beta.solana.com \
  --rpc https://solana-rpc.publicnode.com \
  --profile bot
```

The key insight: **lower latency is not automatically better.** An endpoint can
answer faster but serve *staler* slots. For bot and indexer workloads, slot
freshness often matters more than raw HTTP latency, and the recommendation says
so when the faster endpoint is the staler one.

Pick the profile that matches your use case:

| Profile | For |
| --- | --- |
| `wallet` | wallets/dApps — reliability and blockhash readiness |
| `bot` | bots/automation — slot freshness and latency tradeoffs |
| `indexer` | indexers/pipelines — freshness and data availability |
| `ci` | CI gates — deterministic pass/fail readiness |
| `general` | balanced default |

Comparisons are only valid within one network — mixed mainnet/devnet inputs are
rejected by genesis hash, because slot lag across networks is meaningless.

## 3. Is realtime (WebSocket) ready?

```bash
sol-doctor ws --rpc https://api.mainnet-beta.solana.com
```

This derives the `wss` URL, connects, subscribes with `slotSubscribe`, measures
the time-to-first-slot-notification, then unsubscribes and closes. If your bot or
indexer relies on subscriptions, this is the check that matters. (It is a
single-shot readiness check, not continuous monitoring.)

## 4. Produce a shareable report

```bash
sol-doctor compare \
  --rpc https://api.mainnet-beta.solana.com \
  --rpc https://solana-rpc.publicnode.com \
  --profile bot \
  --report rpc-report.md
```

Credentials and likely API keys are redacted from the terminal, JSON, Markdown,
and error output (basic-auth, common secret query parameters, and provider
tokens in URL paths), so the report is safe to share. Always re-read a report
before sharing to confirm no secret slipped through.

For a full report workflow, see
[`docs/rpc-readiness-report.md`](rpc-readiness-report.md).

## What this does not do

It is a local diagnostic CLI, not hosted monitoring, not an SLA, and not a
replacement for provider observability. Scores are deterministic heuristics, not
provider guarantees.

---

Solana Infra Doctor is an independent open-source tool and is not affiliated with
or endorsed by Solana Foundation.
