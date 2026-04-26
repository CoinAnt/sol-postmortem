# solpm

Solana transaction postmortem. Paste a signature, get a developer-grade debug view: every program invocation as a CPI tree, compute units per hop, success/fail per hop, and the actual failure reason from program logs.

Block explorers are built for browsing. `solpm` is built for the moment a transaction failed in production and you need to understand it, fast.

## Install

Requires a recent Rust toolchain (tested on 1.93).

```bash
git clone <this-repo>
cd solpm
cargo install --path .
```

Or run directly without installing:

```bash
cargo run -- <tx-signature>
```

## Usage

```bash
solpm <tx-signature>
solpm <tx-signature> --rpc https://your-rpc-endpoint
```

The RPC URL resolves in this order: `--rpc` flag, `SOLPM_RPC_URL` env var, then `https://api.mainnet-beta.solana.com`. The public endpoint is rate-limited and only retains recent history — point at your own RPC (Helius, Triton, QuickNode, or a private node) for anything serious.

## Example

```text
[ OK ] slot 415823696  fee 15000 lamports
        blocktime 1777222430

  Top-level instructions:
    #0 program ComputeBudget111111111111111111111111111111  data 12 bytes
    #1 program ComputeBudget111111111111111111111111111111  data 6 bytes
    #6 program pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA  data 33 bytes
    ...

  Execution trace:
    [ ok ] ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL  20600 CU
      [ ok ] TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA   1569 CU
      [ ok ] 11111111111111111111111111111111              — CU
    [ ok ] pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA   97118 CU
      [ ok ] pfeeUxB6jkeY1Hxd7CsFCAjcbHA9rWtchMGdZ6VojVZ   4658 CU
      ...

  status: SUCCESS
```

For a failed transaction:

```text
[FAIL] slot 415823696  fee 7555 lamports
    [FAIL] DiabLoFN9hCNkEc2HhCtgo1VqeQvyRXQiGh2B14mnwJs  64315 CU
          └─ reason: custom program error: 0x1
  status: FAILED — InstructionError(3, Custom(1))
```

## Status

**v0 — early.** What's wired up:

- Tx fetch by signature against any RPC.
- Top-level instruction list with program IDs and data length.
- Full CPI tree reconstructed from program logs, with depth-correct indentation.
- Compute units consumed per program invocation.
- Per-hop success/fail badges and the raw fail reason.

What's not yet:

- Decoded instruction names and arguments. Today we show the program ID and data length; we don't yet fetch the program's Anchor IDL and Borsh-decode the args. That's next.
- Translating `Custom(N)` error codes into the IDL's named error (`InsufficientLiquidity` instead of `0x1`).
- Account state diffs (lamports, parsed token balances) per instruction.
- Versioned-tx address-table-lookup expansion.

## How it's built

Pure Rust, single static binary on release.

- `ureq` (rustls) for the JSON-RPC POST. Avoids the openssl-sys dependency that `solana-client` pulls transitively, which doesn't build cleanly on Windows MSVC without OpenSSL installed.
- `solana-transaction-status` for the response types only.
- `solana-pubkey` for `Pubkey` parsing in the log folder.
- `clap` for the CLI, `owo-colors` for the terminal output, `anyhow` for error plumbing.

## License

MIT OR Apache-2.0
