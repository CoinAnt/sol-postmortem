use crate::diffs;
use crate::postmortem::{
    DecodedInstruction, NodeStatus, Postmortem, PostmortemStatus, TraceNode,
};
use crate::style;

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
        style::red_bold("FAIL")
    } else {
        style::green_bold(" OK ")
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
    println!("  {}", style::bold("Execution trace:"));
    for node in nodes {
        let indent = "  ".repeat(node.depth);
        let badge = match node.status {
            NodeStatus::Ok => style::green(" ok "),
            NodeStatus::Fail => style::red_bold("FAIL"),
            NodeStatus::Unknown => style::yellow(" ?? "),
        };
        let cu = match node.compute_units {
            Some(n) => style::dim(&format!("{n} CU")),
            None => style::dim("— CU"),
        };
        match &node.instruction {
            Some(ix) => println!(
                "  {indent}[{badge}] {}  {}  {cu}",
                style::cyan(&node.program_label),
                format_call(ix),
            ),
            None => println!(
                "  {indent}[{badge}] {}  {cu}",
                style::cyan(&node.program_label),
            ),
        }
        if let Some(reason) = &node.fail_reason {
            println!(
                "  {indent}      {} {}",
                style::dim("└─ reason:"),
                style::red(reason),
            );
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
            println!("  {} {}", style::bold("status:"), style::green_bold("SUCCESS"));
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
                        "  {} {} — instruction #{idx} ({}) failed: {}",
                        style::bold("status:"),
                        style::red_bold("FAILED"),
                        style::cyan(&label),
                        style::red_bold(n),
                    );
                    if let Some(origin) = originating_program_label {
                        if origin != &label {
                            println!("           originated in {}", style::cyan(origin));
                        }
                    }
                }
                (Some(idx), None) => {
                    println!(
                        "  {} {} — instruction #{idx} ({}) failed: {}",
                        style::bold("status:"),
                        style::red_bold("FAILED"),
                        style::cyan(&label),
                        style::red_bold(code),
                    );
                }
                (None, _) => {
                    println!(
                        "  {} {} — {}",
                        style::bold("status:"),
                        style::red_bold("FAILED"),
                        style::red_bold(code),
                    );
                }
            }
        }
    }
}
