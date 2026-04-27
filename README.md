# sol-postmortem

Solana transaction postmortem. Paste a signature, get a developer-grade debug view: every program invocation as a CPI tree, compute units per hop, success/fail per hop, and the actual failure reason from program logs.

Block explorers are built for browsing. `sol-postmortem` is built for the moment a transaction failed in production and you need to understand it, fast.

The crate is `sol-postmortem`; the binary it installs is `solpmortem`.

## Install

```bash
cargo install sol-postmortem
```

That gives you `solpmortem` on your PATH. Requires a recent Rust toolchain (tested on 1.93).

To run from a checkout instead:

```bash
git clone https://github.com/<owner>/sol-postmortem
cd sol-postmortem
cargo run -- <tx-signature>
```

## Usage

```bash
solpmortem <tx-signature>
solpmortem <tx-signature> --rpc https://your-rpc-endpoint
solpmortem <tx-signature> --json | jq '.status'
solpmortem <tx-signature> --color never > postmortem.txt
```

The RPC URL resolves in this order: `--rpc` flag, `SOLPM_RPC_URL` env var, then `https://api.mainnet-beta.solana.com`. The public endpoint is rate-limited and only retains recent history — point at your own RPC (Helius, Triton, QuickNode, or a private node) for anything serious.

### `--color`

`auto` (default), `always`, or `never`. Auto enables colours when stdout is a terminal and the standard `NO_COLOR` env var isn't set; `--json` always disables them. Force on or off with the explicit values when piping to a logger or redirecting to a file.

### `--json`

Emits a single pretty-printed JSON object to stdout instead of the terminal-formatted view. Schema highlights:

- `status.result`: `"success"` or `"failed"`. When `failed`, includes `instruction_index`, `top_program_label`, `code` (raw, e.g. `"Custom(101)"`), `name` (resolved when possible), `source` (`"idl"` or `"anchor_framework"`), and `originating_program_*` for failures that propagated from a deeper CPI.
- `trace`: array of `{ depth, program_id, program_label, instruction, compute_units, status, fail_reason }`. `instruction` is `null` when the program has no IDL or its discriminator didn't match.
- `diffs.lamports`: `{ pubkey, is_signer, is_writable, before, after, delta }`. Lamport fields are JSON numbers (typical values fit in a JS safe int); `delta` is a string since it can be negative i128 in pathological cases.
- `diffs.tokens`: `{ pubkey, mint, mint_symbol, decimals, before_raw, after_raw, delta_raw, before_ui, after_ui, delta_ui }`. Raw amounts are strings (u128 routinely overflows JS safe int for tokens with many decimals); UI amounts are decimal-scaled strings.

## Example

A successful Pump.fun AMM swap. Each tree node shows the program label, the decoded instruction call (when the program has an on-chain IDL), and compute units. The diff sections below show what actually moved on-chain. The trailing `…` truncates account pubkeys for display.

```text
[ OK ] slot 415823696  fee 15000 lamports

  Execution trace:
    [ ok ] spl-associated-token-account      20600 CU
      [ ok ] spl-token                        1569 CU
      [ ok ] system                           — CU
    [ ok ] pump_amm  buy_exact_quote_in { spendable_quote_in: 1581045, min_base_amount_out: 1551612806, ... }  97118 CU
      [ ok ] pump_fees  get_fees { is_pump_pool: true, market_cap_lamports: 517779801768, trade_size_lamports: 1581045 }  4658 CU
      [ ok ] spl-token-2022                   2475 CU
      [ ok ] spl-token                        6238 CU

  Lamport changes:
    [sw] 65ZTb9…XY9V  -0.003670125 SOL  0.013770325 SOL → 0.010100200 SOL
    [-w] 49snKg…tcBp  +0.002074080 SOL  0.000000000 SOL → 0.002074080 SOL
    [-w] 6hx1Px…tNM8  +0.001565421 SOL  95.954265310 SOL → 95.955830731 SOL

  Token changes:
    49snKg…tcBp  +3017.247018  0.000000 → 3017.247018  (mint 98Q9Va…pump)
    6hx1Px…tNM8  +0.001565421  95.952226 → 95.953792  (WSOL)
    EiGR6M…9fd8  -3017.247018  185314733.6036 → 185311716.3566  (mint 98Q9Va…pump)

  status: SUCCESS
```

The `[sw]` / `[-w]` flags mean signer-writable / non-signer-writable. The token diff includes the new total post-trade so you can verify both sides of the trade balance.

A failed Voltr → Drift cross-program call. The error originated three levels deep, and `Custom(101)` is an Anchor framework code, not a program-defined one — `sol-postmortem` translates it to its actual name. Note that `drift` shows no decoded call: that's exactly the diagnostic, since `InstructionFallbackNotFound` means drift didn't recognise the discriminator the adapter sent it.

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

**Early but useful.** What's wired up:

- Tx fetch by signature against any RPC.
- Single CPI tree showing every executed instruction (top-level + inner) with depth-correct indentation, compute units per hop, success/fail badges, and the decoded `program.instruction { args }` inline at each node — when the program has an on-chain Anchor IDL we can fetch. Both new-format (Anchor >= 0.30, explicit discriminators) and old-format (Anchor < 0.30, discriminator computed at runtime as `sha256("global:" + snake_case(name))[..8]`) IDLs are supported.
- Programs without a published IDL fall back to a known-program label registry (`spl-token`, `system`, `compute-budget`, `jupiter v6`, `drift v2`, etc.), then raw program ID.
- Lamport balance changes per account, with signer/writable flags and before/after balances.
- SPL token balance changes per account, decimal-aware, with mint label (well-known mints like USDC/USDT/WSOL/BONK get symbols; others show the raw mint).
- For failed transactions: the failing instruction is identified by index and program. The error code is resolved against (1) the failing program's IDL `errors` table, (2) any IDL in the failed CPI chain (the error may have originated below the top level), then (3) Anchor's built-in framework error table (`InstructionFallbackNotFound`, `ConstraintSeeds`, etc.).
- Versioned (v0) transactions: account indices for diffs are resolved against the static keys plus `meta.loaded_addresses` (writable + readonly) so diffs work for accounts loaded via address-table lookups.

What's not yet:

- Per-instruction state diffs (today's diffs are transaction-level totals; per-instruction would need SVM simulation).
- Address-table-lookup expansion in the *tree* (the diffs already see ALT-loaded accounts, but the executed-ix list doesn't yet show their pubkeys).

## How it's built

Pure Rust, single static binary on release.

- `ureq` (rustls) for the JSON-RPC POST. Avoids the openssl-sys dependency that `solana-client` pulls transitively, which doesn't build cleanly on Windows MSVC without OpenSSL installed.
- `solana-transaction-status` for the response types, `solana-pubkey` for the `Pubkey` type, `solana-transaction-error` + `solana-instruction` for structurally pattern-matching the failure type.
- `flate2` for zlib-decompressing the on-chain Anchor IDL payload, `serde_json` to parse it. The IDL → Borsh decoder is hand-rolled in `src/decode.rs` rather than using `@coral-xyz/anchor`'s JS-only equivalent.
- `clap` for the CLI, `anyhow` for error plumbing. Terminal styling is hand-rolled in `src/style.rs` so `--color never` (and stdout-isn't-a-tty) actually emit clean plain text — most ANSI crates always emit escape codes regardless of any global override.

## License

MIT OR Apache-2.0
