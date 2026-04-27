use owo_colors::OwoColorize;

use crate::diffs;
use crate::postmortem::{
    DecodedInstruction, NodeStatus, Postmortem, PostmortemStatus, TraceNode,
};

pub fn print_pretty(pm: &Postmortem) {
    print_header(pm);
    print_trace(&pm.trace);
    diffs::print(&pm.diffs);
    print_status(pm);
}

// ---------------------------------------------------------------------------
// Header
// ---------------------------------------------------------------------------

fn print_header(pm: &Postmortem) {
    println!();
    let failed = matches!(pm.status, PostmortemStatus::Failed { .. });
    let badge = if failed {
        "FAIL".red().bold().to_string()
    } else {
        " OK ".green().bold().to_string()
    };
    println!(
        "  [{badge}] slot {}  fee {} lamports",
        pm.slot, pm.fee_lamports
    );
    if let Some(blocktime) = pm.block_time {
        println!("        blocktime {blocktime}");
    }
}

// ---------------------------------------------------------------------------
// CPI tree
// ---------------------------------------------------------------------------

fn print_trace(nodes: &[TraceNode]) {
    if nodes.is_empty() {
        return;
    }
    println!();
    println!("  {}", "Execution trace:".bold());
    for node in nodes {
        let indent = "  ".repeat(node.depth);
        let badge = match node.status {
            NodeStatus::Ok => " ok ".green().to_string(),
            NodeStatus::Fail => "FAIL".red().bold().to_string(),
            NodeStatus::Unknown => " ?? ".yellow().to_string(),
        };
        let cu = match node.compute_units {
            Some(n) => format!("{n} CU").dimmed().to_string(),
            None => "— CU".dimmed().to_string(),
        };
        match &node.instruction {
            Some(ix) => println!(
                "  {indent}[{badge}] {}  {}  {cu}",
                node.program_label.cyan(),
                format_call(ix),
            ),
            None => println!(
                "  {indent}[{badge}] {}  {cu}",
                node.program_label.cyan()
            ),
        }
        if let Some(reason) = &node.fail_reason {
            println!("  {indent}      {} {}", "└─ reason:".dimmed(), reason.red());
        }
    }
}

fn format_call(ix: &DecodedInstruction) -> String {
    let inner = ix
        .args
        .iter()
        .map(|a| format!("{}: {}", a.name, a.value))
        .collect::<Vec<_>>()
        .join(", ");
    let body = if ix.args.is_empty() {
        ix.name.clone()
    } else {
        format!("{} {{ {inner} }}", ix.name)
    };
    match &ix.partial_decode_error {
        Some(e) => format!("{body}  <decode error: {e}>"),
        None => body,
    }
}

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

fn print_status(pm: &Postmortem) {
    println!();
    match &pm.status {
        PostmortemStatus::Success => {
            println!("{} {}", "  status:".bold(), "SUCCESS".green().bold());
        }
        PostmortemStatus::Failed {
            instruction_index,
            top_program_label,
            code,
            name,
            originating_program_label,
            ..
        } => {
            let label = top_program_label
                .clone()
                .unwrap_or_else(|| "?".to_string());
            match (instruction_index, name) {
                (Some(idx), Some(n)) => {
                    println!(
                        "{} {} — instruction #{idx} ({}) failed: {}",
                        "  status:".bold(),
                        "FAILED".red().bold(),
                        label.cyan(),
                        n.red().bold()
                    );
                    if let Some(origin) = originating_program_label {
                        if origin != &label {
                            println!("           originated in {}", origin.cyan());
                        }
                    }
                }
                (Some(idx), None) => {
                    println!(
                        "{} {} — instruction #{idx} ({}) failed: {}",
                        "  status:".bold(),
                        "FAILED".red().bold(),
                        label.cyan(),
                        code.red().bold()
                    );
                }
                (None, _) => {
                    println!(
                        "{} {} — {}",
                        "  status:".bold(),
                        "FAILED".red().bold(),
                        code.red().bold()
                    );
                }
            }
        }
    }
}
