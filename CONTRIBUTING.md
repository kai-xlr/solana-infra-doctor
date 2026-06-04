# Contributing

Thanks for your interest in Solana Infra Doctor.

## Development

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test
```

Keep the tool a lightweight, local-first CLI built on direct Solana JSON-RPC. Avoid adding Solana SDK dependencies unless there is a clear need and the tradeoff is documented.

## Architecture

`solana-infra-doctor` is a single published crate that is both a library and a binary:

- `src/lib.rs` is the reusable diagnostic **engine** — `checks/`, `compare/`
  (with `scoring`/`render`), `ws/` (with `analysis`), `rpc/` (with `models`),
  `redact`, `report`, and shared models. Keep business logic here so future
  frontends (for example a CI wrapper) can reuse it.
- `src/main.rs` is the thin `sol-doctor` **CLI frontend** — argument parsing,
  dispatch, and exit codes only.

Keep tests free of live-network calls (use the mock HTTP/WebSocket servers in
`tests/`) and maintain the 95% coverage gate.

## Project Scope

This project is an independent developer tool. Do not add token, NFT, airdrop, governance, points, marketplace, hosted dashboard, cloud service, or speculative crypto mechanics.

## Pull Requests

- Keep changes small and reviewable.
- Include tests for behavior changes.
- Avoid logging sensitive RPC URLs, credentials, or query strings.
- Keep user-facing error messages clear and concise.

## Releasing

Releases are published to crates.io via Trusted Publishing from a manually
triggered GitHub Actions workflow (no permanent API token in GitHub Secrets).
See [`docs/releasing.md`](docs/releasing.md) for the full maintainer process.
