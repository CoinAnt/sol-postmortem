use anyhow::Result;
use owo_colors::OwoColorize;
use solana_pubkey::Pubkey;
use solana_instruction::error::InstructionError;
use solana_transaction_error::TransactionError;
use solana_transaction_status::option_serializer::OptionSerializer;
use solana_transaction_status::{
    EncodedConfirmedTransactionWithStatusMeta, EncodedTransaction, UiMessage,
};
use std::collections::HashMap;
use std::str::FromStr;

use crate::decode;
use crate::idl::{self, Idl};
use crate::logs::{self, InvocationStatus, ProgramInvocation};
use crate::programs;

pub fn print_postmortem(rpc_url: &str, tx: &EncodedConfirmedTransactionWithStatusMeta) -> Result<()> {
    let meta = tx
        .transaction
        .meta
        .as_ref()
        .expect("rpc::fetch_transaction guarantees meta is present");

    let log_messages: Vec<String> = match &meta.log_messages {
        OptionSerializer::Some(v) => v.clone(),
        _ => Vec::new(),
    };

    print_header(tx, meta.fee, meta.err.is_some());

    let (account_keys, top_level): (Vec<String>, Vec<TopLevelIx>) =
        extract_top_level(&tx.transaction.transaction);

    // Fetch IDLs for every unique program ID in the top-level instructions.
    // Cache silently — every miss is fine, we fall back to the program registry.
    let mut idl_cache: HashMap<String, Option<Idl>> = HashMap::new();
    for ix in &top_level {
        idl_cache.entry(ix.program_id.clone()).or_insert_with(|| {
            match Pubkey::from_str(&ix.program_id) {
                Ok(pid) => match idl::fetch(rpc_url, &pid) {
                    Ok(idl) => idl,
                    Err(_) => None, // Don't fail the whole tx render on one IDL fetch.
                },
                Err(_) => None,
            }
        });
    }

    print_top_level(&top_level, &idl_cache);

    let invocations = logs::parse(&log_messages);

    // For failed txs, also pull IDLs for any failing program in the CPI chain
    // so we can decode an error that originated below the top level.
    if meta.err.is_some() {
        for inv in &invocations {
            if !matches!(inv.status, InvocationStatus::Failed(_)) {
                continue;
            }
            let key = inv.program_id.to_string();
            idl_cache.entry(key.clone()).or_insert_with(|| {
                idl::fetch(rpc_url, &inv.program_id).unwrap_or(None)
            });
        }
    }

    print_invocations(&invocations);

    print_status(
        meta.err.as_ref(),
        &top_level,
        &invocations,
        &idl_cache,
        &account_keys,
    );

    Ok(())
}

// ---------------------------------------------------------------------------

struct TopLevelIx {
    program_id: String,
    /// Raw instruction data bytes (already base58-decoded).
    data: Vec<u8>,
}

fn extract_top_level(tx: &EncodedTransaction) -> (Vec<String>, Vec<TopLevelIx>) {
    let EncodedTransaction::Json(ui_tx) = tx else {
        return (Vec::new(), Vec::new());
    };
    let UiMessage::Raw(msg) = &ui_tx.message else {
        return (Vec::new(), Vec::new());
    };

    let mut out = Vec::with_capacity(msg.instructions.len());
    for ix in &msg.instructions {
        let pid_idx = ix.program_id_index as usize;
        let program_id = msg
            .account_keys
            .get(pid_idx)
            .cloned()
            .unwrap_or_else(|| "<unknown>".to_string());
        let data = bs58::decode(&ix.data).into_vec().unwrap_or_default();
        out.push(TopLevelIx { program_id, data });
    }
    (msg.account_keys.clone(), out)
}

// ---------------------------------------------------------------------------

fn print_header(
    tx: &EncodedConfirmedTransactionWithStatusMeta,
    fee_lamports: u64,
    failed: bool,
) {
    println!();
    let badge = if failed {
        "FAIL".red().bold().to_string()
    } else {
        " OK ".green().bold().to_string()
    };
    println!("  [{badge}] slot {}  fee {} lamports", tx.slot, fee_lamports);
    if let Some(blocktime) = tx.block_time {
        println!("        blocktime {blocktime}");
    }
}

fn print_top_level(top_level: &[TopLevelIx], idl_cache: &HashMap<String, Option<Idl>>) {
    println!();
    println!("  {}", "Top-level instructions:".bold());
    for (i, ix) in top_level.iter().enumerate() {
        let label = label_for(&ix.program_id, idl_cache);
        let decoded = decode_for(&ix.program_id, &ix.data, idl_cache);

        match decoded {
            Some(d) => println!(
                "    {} {}  {}",
                format!("#{i}").dimmed(),
                label.cyan(),
                d
            ),
            None => println!(
                "    {} {}  {} {} {}",
                format!("#{i}").dimmed(),
                label.cyan(),
                "data".dimmed(),
                ix.data.len(),
                "bytes".dimmed()
            ),
        }
    }
}

fn label_for(program_id: &str, idl_cache: &HashMap<String, Option<Idl>>) -> String {
    if let Some(Some(idl)) = idl_cache.get(program_id) {
        if !idl.program_name.is_empty() && idl.program_name != "(unnamed)" {
            return idl.program_name.clone();
        }
    }
    if let Some(known) = programs::label(program_id) {
        return known.to_string();
    }
    program_id.to_string()
}

fn decode_for(
    program_id: &str,
    data: &[u8],
    idl_cache: &HashMap<String, Option<Idl>>,
) -> Option<String> {
    let idl = idl_cache.get(program_id)?.as_ref()?;
    let outcome = decode::decode_instruction(idl, data);
    decode::render_outcome(&outcome)
}

// ---------------------------------------------------------------------------

fn print_invocations(invocations: &[ProgramInvocation]) {
    if invocations.is_empty() {
        return;
    }
    println!();
    println!("  {}", "Execution trace:".bold());
    for inv in invocations {
        let indent = "  ".repeat(inv.depth);
        let badge = match &inv.status {
            InvocationStatus::Success => " ok ".green().to_string(),
            InvocationStatus::Failed(_) => "FAIL".red().bold().to_string(),
            InvocationStatus::Unknown => " ?? ".yellow().to_string(),
        };
        let cu = match inv.compute_units_consumed {
            Some(n) => format!("{n} CU").dimmed().to_string(),
            None => "— CU".dimmed().to_string(),
        };
        let label = programs::label(&inv.program_id.to_string())
            .map(String::from)
            .unwrap_or_else(|| inv.program_id.to_string());
        println!("  {indent}[{badge}] {}  {cu}", label.cyan());
        if let InvocationStatus::Failed(reason) = &inv.status {
            println!("  {indent}      {} {}", "└─ reason:".dimmed(), reason.red());
        }
    }
}

// ---------------------------------------------------------------------------

fn print_status(
    err: Option<&TransactionError>,
    top_level: &[TopLevelIx],
    invocations: &[ProgramInvocation],
    idl_cache: &HashMap<String, Option<Idl>>,
    _account_keys: &[String],
) {
    println!();
    let Some(err) = err else {
        println!("{} {}", "  status:".bold(), "SUCCESS".green().bold());
        return;
    };

    // Enrich InstructionError(idx, Custom(code)) with the IDL error name, or
    // fall back to Anchor's framework error table.
    if let TransactionError::InstructionError(idx, ref ix_err) = err {
        let idx = *idx as usize;
        let top_pid = top_level.get(idx).map(|ix| ix.program_id.as_str());

        let enriched = if let InstructionError::Custom(code) = ix_err {
            // 1. Walk the failed CPI chain from deepest up, return first IDL hit.
            let from_idl = failed_chain_from_deepest(invocations)
                .into_iter()
                .find_map(|pid| {
                    let idl = idl_cache.get(&pid.to_string())?.as_ref()?;
                    decode::lookup_error(idl, *code).map(|name| (Some(pid), name))
                });
            // 2. Fall back to Anchor's built-in framework error codes.
            from_idl.or_else(|| {
                decode::anchor_framework_error(*code)
                    .map(|name| (None, format!("{name} (Anchor framework)")))
            })
        } else {
            None
        };

        let prog_label = top_pid
            .map(|p| label_for(p, idl_cache))
            .unwrap_or_else(|| "?".to_string());

        match enriched {
            Some((origin_pid, name)) => {
                println!(
                    "{} {} — instruction #{idx} ({}) failed: {}",
                    "  status:".bold(),
                    "FAILED".red().bold(),
                    prog_label.cyan(),
                    name.red().bold()
                );
                if let Some(origin_pid) = origin_pid {
                    let origin_label = label_for(&origin_pid.to_string(), idl_cache);
                    if origin_label != prog_label {
                        println!("           originated in {}", origin_label.cyan());
                    }
                }
                return;
            }
            None => {
                println!(
                    "{} {} — instruction #{idx} ({}) failed: {:?}",
                    "  status:".bold(),
                    "FAILED".red().bold(),
                    prog_label.cyan(),
                    ix_err.red().bold()
                );
                return;
            }
        }
    }

    println!(
        "{} {} — {:?}",
        "  status:".bold(),
        "FAILED".red().bold(),
        err.red().bold()
    );
}

/// Programs from the CPI chain that ended in Failed, ordered deepest-first.
fn failed_chain_from_deepest(invocations: &[ProgramInvocation]) -> Vec<Pubkey> {
    let mut failed: Vec<&ProgramInvocation> = invocations
        .iter()
        .filter(|inv| matches!(inv.status, InvocationStatus::Failed(_)))
        .collect();
    failed.sort_by(|a, b| b.depth.cmp(&a.depth));
    failed.into_iter().map(|inv| inv.program_id).collect()
}
