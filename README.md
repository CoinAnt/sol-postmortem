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

A successful Pump.fun AMM swap. Each tree node shows the program label, the decoded instruction call (when the program has an on-chain IDL), and compute units. CPI children are indented under their caller:

```text
[ OK ] slot 415823696  fee 15000 lamports

  Execution trace:
    [ ok ] compute-budget                    — CU
    [ ok ] compute-budget                    — CU
    [ ok ] spl-associated-token-account      20600 CU
      [ ok ] spl-token                        1569 CU
      [ ok ] system                           — CU
      [ ok ] spl-token                        1405 CU
    [ ok ] pump_amm  buy_exact_quote_in { spendable_quote_in: 1581045, min_base_amount_out: 1551612806, ... }  97118 CU
      [ ok ] pump_fees  get_fees { is_pump_pool: true, market_cap_lamports: 517779801768, trade_size_lamports: 1581045 }  4658 CU
      [ ok ] spl-token-2022                   2475 CU
      [ ok ] spl-token                        6238 CU

  status: SUCCESS
```

A failed Voltr → Drift cross-program call. The error originated three levels deep, and `Custom(101)` is an Anchor framework code, not a program-defined one — `solpm` translates it to its actual name. Note that `drift` shows no decoded call: that's exactly the diagnostic, since `InstructionFallbackNotFound` means drift didn't recognise the discriminator the adapter sent it.

```text
[FAIL] slot 415857495  fee 10000 lamports

  Execution trace:
    [ ok ] compute-budget                                       — CU
    [ ok ] compute-budget                                       — CU
    [FAIL] voltr_vault  withdraw_strategy { amount: 663480523, ... }  50708 CU
          └─ reason: custom program error: 0x65
      [FAIL] aVoLTRCRt3NnnchvLYH6rMYehJHwM5m45RmLBZq7PGz        18929 CU
            └─ reason: custom program error: 0x65
        [FAIL] drift                                            3004 CU
              └─ reason: custom program error: 0x65

  status: FAILED — instruction #2 (voltr_vault) failed: InstructionFallbackNotFound (Anchor framework)
```

## Status

**v0.2 — useful.** What's wired up:

- Tx fetch by signature against any RPC.
- Single CPI tree showing every executed instruction (top-level + inner) with depth-correct indentation, compute units per hop, success/fail badges, and the decoded `program.instruction { args }` inline at each node — when the program has an on-chain Anchor IDL we can fetch.
- Programs without a published IDL fall back to a known-program label registry (`spl-token`, `system`, `compute-budget`, `jupiter v6`, `drift v2`, etc.), then raw program ID.
- For failed transactions: the failing instruction is identified by index and program. The error code is resolved against (1) the failing program's IDL `errors` table, (2) any IDL in the failed CPI chain (the error may have originated below the top level), then (3) Anchor's built-in framework error table (`InstructionFallbackNotFound`, `ConstraintSeeds`, etc.).

What's not yet:

- Account state diffs (lamports, parsed token balances) per instruction.
- Versioned-tx address-table-lookup expansion.
- Old-format Anchor IDLs (Anchor < 0.30, where instruction discriminators were derived implicitly from the snake_case name).

## How it's built

Pure Rust, single static binary on release.

- `ureq` (rustls) for the JSON-RPC POST. Avoids the openssl-sys dependency that `solana-client` pulls transitively, which doesn't build cleanly on Windows MSVC without OpenSSL installed.
- `solana-transaction-status` for the response types, `solana-pubkey` for the `Pubkey` type, `solana-transaction-error` + `solana-instruction` for structurally pattern-matching the failure type.
- `flate2` for zlib-decompressing the on-chain Anchor IDL payload, `serde_json` to parse it. The IDL → Borsh decoder is hand-rolled in `src/decode.rs` rather than using `@coral-xyz/anchor`'s JS-only equivalent.
- `clap` for the CLI, `owo-colors` for the terminal output, `anyhow` for error plumbing.

## License

MIT OR Apache-2.0
