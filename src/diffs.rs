// Compute account state changes (lamport + SPL token balance deltas) from a
// transaction's pre/post snapshots, and render them as a readable section.
//
// Notes on the underlying RPC data:
// - meta.pre_balances / post_balances are u64 lamports indexed by account_keys.
// - meta.pre_token_balances / post_token_balances are sparse — only accounts
//   that hold an SPL token (before or after) appear, indexed by account_index.
// - For v0 (versioned) transactions, additional accounts are loaded via
//   address-table lookups and appear in meta.loaded_addresses.{writable,readonly}.
//   pre/post_balances are indexed against the combined list:
//     [static keys from message, then writable loaded, then readonly loaded].

use serde::Serialize;
use solana_transaction_status::option_serializer::OptionSerializer;
use solana_transaction_status::{
    EncodedTransaction, UiMessage, UiTransactionStatusMeta, UiTransactionTokenBalance,
};

use crate::style;
use crate::tokens;

#[derive(Debug, Serialize)]
pub struct LamportDiff {
    pub pubkey: String,
    pub is_signer: bool,
    pub is_writable: bool,
    pub before: u64,
    pub after: u64,
    /// Signed delta in lamports, serialised as a string to dodge JS precision
    /// loss for large negative values.
    #[serde(serialize_with = "i128_as_string")]
    pub delta: i128,
}

#[derive(Debug, Serialize)]
pub struct TokenDiff {
    pub pubkey: String,
    pub mint: String,
    /// Friendly mint symbol when known (e.g. "WSOL", "USDC"), else None.
    pub mint_symbol: Option<String>,
    pub decimals: u8,
    /// Raw token amounts as u128 — serialised as strings since they routinely
    /// exceed JS safe integer range for tokens with many decimals.
    #[serde(serialize_with = "u128_as_string")]
    pub before_raw: u128,
    #[serde(serialize_with = "u128_as_string")]
    pub after_raw: u128,
    #[serde(serialize_with = "i128_as_string")]
    pub delta_raw: i128,
    /// Decimal-scaled human-readable strings (e.g. "1.234567").
    pub before_ui: String,
    pub after_ui: String,
    pub delta_ui: String,
}

#[derive(Debug, Serialize)]
pub struct DiffSummary {
    pub lamports: Vec<LamportDiff>,
    pub tokens: Vec<TokenDiff>,
}

fn u128_as_string<S: serde::Serializer>(v: &u128, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&v.to_string())
}

fn i128_as_string<S: serde::Serializer>(v: &i128, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&v.to_string())
}

pub fn compute(tx: &EncodedTransaction, meta: &UiTransactionStatusMeta) -> DiffSummary {
    let (account_keys, signer_count, writable_signer_count, writable_unsigned_count) =
        message_meta(tx);

    let loaded_writable: Vec<String> = match &meta.loaded_addresses {
        OptionSerializer::Some(la) => la.writable.clone(),
        _ => Vec::new(),
    };
    let loaded_readonly: Vec<String> = match &meta.loaded_addresses {
        OptionSerializer::Some(la) => la.readonly.clone(),
        _ => Vec::new(),
    };

    // Combined index space: [static, then loaded_writable, then loaded_readonly].
    let mut combined: Vec<String> = account_keys.clone();
    combined.extend(loaded_writable.iter().cloned());
    combined.extend(loaded_readonly.iter().cloned());

    let static_count = account_keys.len();
    let loaded_writable_count = loaded_writable.len();

    // Lamports
    let mut lamports: Vec<LamportDiff> = Vec::new();
    let n = meta.pre_balances.len().min(meta.post_balances.len());
    for i in 0..n {
        let before = meta.pre_balances[i];
        let after = meta.post_balances[i];
        if before == after {
            continue;
        }
        let pubkey = combined.get(i).cloned().unwrap_or_else(|| format!("<idx {i}>"));
        let (is_signer, is_writable) = classify(
            i,
            static_count,
            signer_count,
            writable_signer_count,
            writable_unsigned_count,
            loaded_writable_count,
        );
        let delta = after as i128 - before as i128;
        lamports.push(LamportDiff {
            pubkey,
            is_signer,
            is_writable,
            before,
            after,
            delta,
        });
    }

    // Tokens
    let pre_token: &[UiTransactionTokenBalance] = match &meta.pre_token_balances {
        OptionSerializer::Some(v) => v.as_slice(),
        _ => &[],
    };
    let post_token: &[UiTransactionTokenBalance] = match &meta.post_token_balances {
        OptionSerializer::Some(v) => v.as_slice(),
        _ => &[],
    };

    let mut tokens: Vec<TokenDiff> = Vec::new();
    // Walk every account_index that appears in either snapshot.
    let mut indices: Vec<u8> = pre_token
        .iter()
        .chain(post_token.iter())
        .map(|t| t.account_index)
        .collect();
    indices.sort_unstable();
    indices.dedup();

    for idx in indices {
        let pre = pre_token.iter().find(|t| t.account_index == idx);
        let post = post_token.iter().find(|t| t.account_index == idx);

        let mint = post
            .map(|t| t.mint.clone())
            .or_else(|| pre.map(|t| t.mint.clone()))
            .unwrap_or_default();
        let decimals = post
            .map(|t| t.ui_token_amount.decimals)
            .or_else(|| pre.map(|t| t.ui_token_amount.decimals))
            .unwrap_or(0);

        let before_raw: u128 = pre
            .and_then(|t| t.ui_token_amount.amount.parse().ok())
            .unwrap_or(0);
        let after_raw: u128 = post
            .and_then(|t| t.ui_token_amount.amount.parse().ok())
            .unwrap_or(0);
        if before_raw == after_raw {
            continue;
        }

        let pubkey = combined
            .get(idx as usize)
            .cloned()
            .unwrap_or_else(|| format!("<idx {idx}>"));

        let delta_raw = after_raw as i128 - before_raw as i128;
        let mint_symbol = tokens::symbol(&mint).map(String::from);
        let before_ui = ui_amount(before_raw as f64, decimals);
        let after_ui = ui_amount(after_raw as f64, decimals);
        let delta_ui = ui_amount(delta_raw as f64, decimals);
        tokens.push(TokenDiff {
            pubkey,
            mint,
            mint_symbol,
            decimals,
            before_raw,
            after_raw,
            delta_raw,
            before_ui,
            after_ui,
            delta_ui,
        });
    }

    DiffSummary { lamports, tokens }
}

pub fn print(summary: &DiffSummary) {
    if summary.lamports.is_empty() && summary.tokens.is_empty() {
        return;
    }

    if !summary.lamports.is_empty() {
        println!();
        println!("  {}", style::bold("Lamport changes:"));

        // Pre-format raw strings so we can compute max widths and right-align
        // the numeric columns regardless of magnitude.
        let rows: Vec<(String, String, String, String, String, bool)> = summary
            .lamports
            .iter()
            .map(|d| {
                let flags = format!(
                    "[{}{}]",
                    if d.is_signer { "s" } else { "-" },
                    if d.is_writable { "w" } else { "-" }
                );
                let delta_str = format!("{:+.9} SOL", d.delta as f64 / 1_000_000_000.0);
                (
                    flags,
                    short(&d.pubkey),
                    delta_str,
                    lamports_to_sol(d.before),
                    lamports_to_sol(d.after),
                    d.delta > 0,
                )
            })
            .collect();
        let w_delta = rows.iter().map(|r| r.2.len()).max().unwrap_or(0);
        let w_before = rows.iter().map(|r| r.3.len()).max().unwrap_or(0);
        let w_after = rows.iter().map(|r| r.4.len()).max().unwrap_or(0);

        for (flags, pubkey, delta_str, before_str, after_str, positive) in rows {
            let padded_delta = format!("{:>w$}", delta_str, w = w_delta);
            let coloured_delta = if positive {
                style::green(&padded_delta)
            } else {
                style::red(&padded_delta)
            };
            println!(
                "    {} {}  {}  {} → {}",
                style::dim(&flags),
                style::cyan(&pubkey),
                coloured_delta,
                style::dim(&format!("{:>w$}", before_str, w = w_before)),
                style::dim(&format!("{:>w$}", after_str, w = w_after)),
            );
        }
    }

    if !summary.tokens.is_empty() {
        println!();
        println!("  {}", style::bold("Token changes:"));

        let rows: Vec<(String, String, bool, String, String, String)> = summary
            .tokens
            .iter()
            .map(|d| {
                let delta_disp = if d.delta_raw > 0 {
                    format!("+{}", d.delta_ui)
                } else {
                    d.delta_ui.clone()
                };
                let mint_label = match &d.mint_symbol {
                    Some(sym) => format!("{sym} ({})", short(&d.mint)),
                    None => format!("mint {}", short(&d.mint)),
                };
                (
                    short(&d.pubkey),
                    delta_disp,
                    d.delta_raw > 0,
                    d.before_ui.clone(),
                    d.after_ui.clone(),
                    mint_label,
                )
            })
            .collect();
        let w_delta = rows.iter().map(|r| r.1.len()).max().unwrap_or(0);
        let w_before = rows.iter().map(|r| r.3.len()).max().unwrap_or(0);
        let w_after = rows.iter().map(|r| r.4.len()).max().unwrap_or(0);

        for (pubkey, delta_str, positive, before_str, after_str, mint_label) in rows {
            let padded_delta = format!("{:>w$}", delta_str, w = w_delta);
            let coloured_delta = if positive {
                style::green(&padded_delta)
            } else {
                style::red(&padded_delta)
            };
            println!(
                "    {}  {}  {} → {}  ({})",
                style::cyan(&pubkey),
                coloured_delta,
                style::dim(&format!("{:>w$}", before_str, w = w_before)),
                style::dim(&format!("{:>w$}", after_str, w = w_after)),
                style::dim(&mint_label),
            );
        }
    }
}

// ---------------------------------------------------------------------------

fn message_meta(tx: &EncodedTransaction) -> (Vec<String>, usize, usize, usize) {
    // (account_keys, signer_count, writable_signer_count, writable_unsigned_count)
    let EncodedTransaction::Json(ui_tx) = tx else {
        return (Vec::new(), 0, 0, 0);
    };
    let UiMessage::Raw(msg) = &ui_tx.message else {
        return (Vec::new(), 0, 0, 0);
    };
    let h = &msg.header;
    let signer_count = h.num_required_signatures as usize;
    let writable_signer = signer_count.saturating_sub(h.num_readonly_signed_accounts as usize);
    let total = msg.account_keys.len();
    let writable_unsigned = total
        .saturating_sub(signer_count)
        .saturating_sub(h.num_readonly_unsigned_accounts as usize);
    (
        msg.account_keys.clone(),
        signer_count,
        writable_signer,
        writable_unsigned,
    )
}

fn classify(
    idx: usize,
    static_count: usize,
    signer_count: usize,
    writable_signer_count: usize,
    writable_unsigned_count: usize,
    loaded_writable_count: usize,
) -> (bool, bool) {
    if idx < static_count {
        // Static keys: header rules apply.
        let is_signer = idx < signer_count;
        let is_writable = if is_signer {
            idx < writable_signer_count
        } else {
            idx < signer_count + writable_unsigned_count
        };
        (is_signer, is_writable)
    } else {
        // Loaded via ALT: writable block first, then readonly. Never signers.
        let in_writable = idx < static_count + loaded_writable_count;
        (false, in_writable)
    }
}

fn short(s: &str) -> String {
    if s.len() <= 12 {
        s.to_string()
    } else {
        format!("{}…{}", &s[..6], &s[s.len() - 4..])
    }
}

fn lamports_to_sol(l: u64) -> String {
    format!("{:.9} SOL", l as f64 / 1_000_000_000.0)
}

fn ui_amount(raw: f64, decimals: u8) -> String {
    let div = 10f64.powi(decimals as i32);
    format!("{:.*}", decimals as usize, raw / div)
}
