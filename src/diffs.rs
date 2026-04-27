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

use owo_colors::OwoColorize;
use solana_transaction_status::option_serializer::OptionSerializer;
use solana_transaction_status::{
    EncodedTransaction, UiMessage, UiTransactionStatusMeta, UiTransactionTokenBalance,
};

use crate::tokens;

#[derive(Debug)]
pub struct LamportDiff {
    pub pubkey: String,
    pub is_signer: bool,
    pub is_writable: bool,
    pub before: u64,
    pub after: u64,
}

impl LamportDiff {
    pub fn delta(&self) -> i128 {
        self.after as i128 - self.before as i128
    }
}

#[derive(Debug)]
pub struct TokenDiff {
    pub pubkey: String,
    pub mint: String,
    pub decimals: u8,
    pub before_raw: u128,
    pub after_raw: u128,
}

impl TokenDiff {
    pub fn delta_raw(&self) -> i128 {
        self.after_raw as i128 - self.before_raw as i128
    }
}

pub struct DiffSummary {
    pub lamports: Vec<LamportDiff>,
    pub tokens: Vec<TokenDiff>,
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
        lamports.push(LamportDiff {
            pubkey,
            is_signer,
            is_writable,
            before,
            after,
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

        tokens.push(TokenDiff {
            pubkey,
            mint,
            decimals,
            before_raw,
            after_raw,
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
        println!("  {}", "Lamport changes:".bold());
        for d in &summary.lamports {
            let flags = format!(
                "[{}{}]",
                if d.is_signer { "s" } else { "-" },
                if d.is_writable { "w" } else { "-" }
            );
            let delta = d.delta();
            let delta_sol = delta as f64 / 1_000_000_000.0;
            let arrow = format!("{:+.9} SOL", delta_sol);
            let coloured = if delta > 0 {
                arrow.green().to_string()
            } else {
                arrow.red().to_string()
            };
            println!(
                "    {} {}  {}  {} → {}",
                flags.dimmed(),
                short(&d.pubkey).cyan(),
                coloured,
                lamports_to_sol(d.before).dimmed(),
                lamports_to_sol(d.after).dimmed(),
            );
        }
    }

    if !summary.tokens.is_empty() {
        println!();
        println!("  {}", "Token changes:".bold());
        for d in &summary.tokens {
            let symbol = tokens::symbol(&d.mint).unwrap_or("");
            let mint_label = if symbol.is_empty() {
                format!("mint {}", short(&d.mint))
            } else {
                format!("{symbol} ({})", short(&d.mint))
            };
            let delta = d.delta_raw();
            let delta_ui = ui_amount(delta as f64, d.decimals);
            let arrow = if delta > 0 {
                format!("+{delta_ui}").green().to_string()
            } else {
                format!("{delta_ui}").red().to_string()
            };
            println!(
                "    {}  {}  {} → {}  ({})",
                short(&d.pubkey).cyan(),
                arrow,
                ui_amount(d.before_raw as f64, d.decimals).dimmed(),
                ui_amount(d.after_raw as f64, d.decimals).dimmed(),
                mint_label.dimmed(),
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
