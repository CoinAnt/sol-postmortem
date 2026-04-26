use anyhow::Result;
use owo_colors::OwoColorize;
use solana_transaction_status::option_serializer::OptionSerializer;
use solana_transaction_status::{
    EncodedConfirmedTransactionWithStatusMeta, EncodedTransaction, UiMessage,
};

use crate::logs::{self, InvocationStatus, ProgramInvocation};

pub fn print_postmortem(tx: &EncodedConfirmedTransactionWithStatusMeta) -> Result<()> {
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
    print_program_ids(&tx.transaction.transaction);

    let invocations = logs::parse(&log_messages);
    print_invocations(&invocations);

    if let Some(err) = &meta.err {
        println!();
        println!(
            "{} {}",
            "  status:".bold(),
            format!("FAILED — {err:?}").red().bold()
        );
    } else {
        println!();
        println!("{} {}", "  status:".bold(), "SUCCESS".green().bold());
    }

    Ok(())
}

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

fn print_program_ids(tx: &EncodedTransaction) {
    let EncodedTransaction::Json(ui_tx) = tx else {
        return;
    };
    let UiMessage::Raw(msg) = &ui_tx.message else {
        return;
    };

    println!();
    println!("  {}", "Top-level instructions:".bold());
    for (i, ix) in msg.instructions.iter().enumerate() {
        let pid_idx = ix.program_id_index as usize;
        let pid = msg
            .account_keys
            .get(pid_idx)
            .cloned()
            .unwrap_or_else(|| "<unknown>".to_string());
        println!(
            "    {} program {}  data {} bytes",
            format!("#{i}").dimmed(),
            pid.cyan(),
            ix.data.len()
        );
    }
}

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
        println!(
            "  {indent}[{badge}] {pid}  {cu}",
            pid = inv.program_id.to_string().cyan()
        );
        if let InvocationStatus::Failed(reason) = &inv.status {
            println!("  {indent}      {} {}", "└─ reason:".dimmed(), reason.red());
        }
    }
}
