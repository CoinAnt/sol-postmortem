use anyhow::Result;
use owo_colors::OwoColorize;
use solana_instruction::error::InstructionError;
use solana_pubkey::Pubkey;
use solana_transaction_error::TransactionError;
use solana_transaction_status::option_serializer::OptionSerializer;
use solana_transaction_status::{
    EncodedConfirmedTransactionWithStatusMeta, EncodedTransaction, UiInnerInstructions,
    UiInstruction, UiMessage,
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

    let inner_instructions: &[UiInnerInstructions] = match &meta.inner_instructions {
        OptionSerializer::Some(v) => v.as_slice(),
        _ => &[],
    };

    let executed = build_executed(&tx.transaction.transaction, inner_instructions);

    // Pre-fetch IDLs for every unique program in the executed flat list.
    // For failed txs, also include any program in the failed CPI chain
    // (it might have been invoked but errored before completing — but the
    // executed list already contains it, so this loop covers both).
    let mut idl_cache: HashMap<String, Option<Idl>> = HashMap::new();
    for ix in &executed {
        idl_cache
            .entry(ix.program_id.clone())
            .or_insert_with(|| match Pubkey::from_str(&ix.program_id) {
                Ok(pid) => idl::fetch(rpc_url, &pid).unwrap_or(None),
                Err(_) => None,
            });
    }

    let invocations = logs::parse(&log_messages);
    let decoded = decode_each(&invocations, &executed, &idl_cache);

    print_invocations(&invocations, &decoded, &idl_cache);

    print_status(meta.err.as_ref(), &invocations, &idl_cache);

    Ok(())
}

// ---------------------------------------------------------------------------
// Executed-instruction flattening
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct ExecutedIx {
    program_id: String,
    /// Raw instruction data bytes (already base58-decoded).
    data: Vec<u8>,
}

fn build_executed(
    tx: &EncodedTransaction,
    inner_instructions: &[UiInnerInstructions],
) -> Vec<ExecutedIx> {
    let EncodedTransaction::Json(ui_tx) = tx else {
        return Vec::new();
    };
    let UiMessage::Raw(msg) = &ui_tx.message else {
        return Vec::new();
    };

    let mut out: Vec<ExecutedIx> = Vec::new();
    for (top_idx, top_ix) in msg.instructions.iter().enumerate() {
        let pid = msg
            .account_keys
            .get(top_ix.program_id_index as usize)
            .cloned()
            .unwrap_or_else(|| "<unknown>".to_string());
        let data = bs58::decode(&top_ix.data).into_vec().unwrap_or_default();
        out.push(ExecutedIx { program_id: pid, data });

        // Inner instructions are stored under the top-level index they belong to.
        if let Some(set) = inner_instructions
            .iter()
            .find(|s| s.index as usize == top_idx)
        {
            for ui_ix in &set.instructions {
                if let UiInstruction::Compiled(ci) = ui_ix {
                    let pid = msg
                        .account_keys
                        .get(ci.program_id_index as usize)
                        .cloned()
                        .unwrap_or_else(|| "<unknown>".to_string());
                    let data = bs58::decode(&ci.data).into_vec().unwrap_or_default();
                    out.push(ExecutedIx { program_id: pid, data });
                }
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Decode each executed ix in lockstep with the log-derived invocation trace
// ---------------------------------------------------------------------------

fn decode_each(
    invocations: &[ProgramInvocation],
    executed: &[ExecutedIx],
    idl_cache: &HashMap<String, Option<Idl>>,
) -> Vec<Option<String>> {
    let mut out = Vec::with_capacity(invocations.len());
    let mut exec_iter = executed.iter();

    for inv in invocations {
        let exec = exec_iter.next();
        let decoded = match exec {
            Some(ix) if ix.program_id == inv.program_id.to_string() => {
                decode_for(&ix.program_id, &ix.data, idl_cache)
            }
            // If the streams desync (unexpected), don't mis-attribute — render bare.
            _ => None,
        };
        out.push(decoded);
    }
    out
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
// Header
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

// ---------------------------------------------------------------------------
// CPI tree
// ---------------------------------------------------------------------------

fn print_invocations(
    invocations: &[ProgramInvocation],
    decoded: &[Option<String>],
    idl_cache: &HashMap<String, Option<Idl>>,
) {
    if invocations.is_empty() {
        return;
    }
    println!();
    println!("  {}", "Execution trace:".bold());
    for (i, inv) in invocations.iter().enumerate() {
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
        let label = label_for(&inv.program_id.to_string(), idl_cache);

        match decoded.get(i).and_then(|d| d.as_ref()) {
            Some(call) => println!(
                "  {indent}[{badge}] {}  {}  {cu}",
                label.cyan(),
                call,
            ),
            None => println!("  {indent}[{badge}] {}  {cu}", label.cyan()),
        }

        if let InvocationStatus::Failed(reason) = &inv.status {
            println!("  {indent}      {} {}", "└─ reason:".dimmed(), reason.red());
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

// ---------------------------------------------------------------------------
// Status / failure enrichment
// ---------------------------------------------------------------------------

fn print_status(
    err: Option<&TransactionError>,
    invocations: &[ProgramInvocation],
    idl_cache: &HashMap<String, Option<Idl>>,
) {
    println!();
    let Some(err) = err else {
        println!("{} {}", "  status:".bold(), "SUCCESS".green().bold());
        return;
    };

    if let TransactionError::InstructionError(idx, ref ix_err) = err {
        let idx = *idx as usize;
        // The Nth top-level program is the Nth depth-1 entry in the trace.
        let top_pid = invocations
            .iter()
            .filter(|inv| inv.depth == 1)
            .nth(idx)
            .map(|inv| inv.program_id.to_string());

        let enriched = if let InstructionError::Custom(code) = ix_err {
            // 1. Walk the failed CPI chain from deepest up.
            let from_idl = failed_chain_from_deepest(invocations)
                .into_iter()
                .find_map(|pid| {
                    let idl = idl_cache.get(&pid.to_string())?.as_ref()?;
                    decode::lookup_error(idl, *code).map(|name| (Some(pid), name))
                });
            // 2. Fall back to Anchor's framework error table.
            from_idl.or_else(|| {
                decode::anchor_framework_error(*code)
                    .map(|name| (None, format!("{name} (Anchor framework)")))
            })
        } else {
            None
        };

        let prog_label = top_pid
            .as_deref()
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

fn failed_chain_from_deepest(invocations: &[ProgramInvocation]) -> Vec<Pubkey> {
    let mut failed: Vec<&ProgramInvocation> = invocations
        .iter()
        .filter(|inv| matches!(inv.status, InvocationStatus::Failed(_)))
        .collect();
    failed.sort_by(|a, b| b.depth.cmp(&a.depth));
    failed.into_iter().map(|inv| inv.program_id).collect()
}
