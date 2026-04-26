// Parse Solana program logs into a structured per-instruction view.
//
// Solana logs follow a stack-based format:
//   "Program <pid> invoke [<depth>]"
//   "Program log: ..." | "Program data: ..." | "Program return: ..."
//   "Program <pid> consumed N of M compute units"
//   "Program <pid> success" | "Program <pid> failed: <reason>"
//
// We fold this into a flat list of program invocations with their CU and
// final status, preserving depth so renderers can build a CPI tree.

use solana_pubkey::Pubkey;
use std::str::FromStr;

#[derive(Debug, Clone)]
pub struct ProgramInvocation {
    pub program_id: Pubkey,
    pub depth: usize,
    pub compute_units_consumed: Option<u64>,
    pub status: InvocationStatus,
    pub messages: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum InvocationStatus {
    Success,
    Failed(String),
    Unknown,
}

pub fn parse(log_messages: &[String]) -> Vec<ProgramInvocation> {
    let mut invocations: Vec<ProgramInvocation> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();

    for line in log_messages {
        if let Some((pid, depth)) = parse_invoke(line) {
            invocations.push(ProgramInvocation {
                program_id: pid,
                depth,
                compute_units_consumed: None,
                status: InvocationStatus::Unknown,
                messages: Vec::new(),
            });
            stack.push(invocations.len() - 1);
            continue;
        }

        if let Some((_pid, cu)) = parse_consumed(line) {
            if let Some(&idx) = stack.last() {
                invocations[idx].compute_units_consumed = Some(cu);
            }
            continue;
        }

        if parse_success(line).is_some() {
            if let Some(idx) = stack.pop() {
                invocations[idx].status = InvocationStatus::Success;
            }
            continue;
        }

        if let Some(reason) = parse_failed(line) {
            if let Some(idx) = stack.pop() {
                invocations[idx].status = InvocationStatus::Failed(reason);
            }
            continue;
        }

        // Anything else (Program log:, Program data:, Program return:) attaches
        // to the currently active invocation as a raw line.
        if let Some(&idx) = stack.last() {
            invocations[idx].messages.push(line.clone());
        }
    }

    invocations
}

fn parse_invoke(line: &str) -> Option<(Pubkey, usize)> {
    // "Program <pid> invoke [<depth>]"
    let rest = line.strip_prefix("Program ")?;
    let (pid_str, after) = rest.split_once(' ')?;
    let after = after.strip_prefix("invoke [")?;
    let depth_str = after.strip_suffix(']')?;
    let pid = Pubkey::from_str(pid_str).ok()?;
    let depth: usize = depth_str.parse().ok()?;
    Some((pid, depth))
}

fn parse_consumed(line: &str) -> Option<(Pubkey, u64)> {
    // "Program <pid> consumed N of M compute units"
    let rest = line.strip_prefix("Program ")?;
    let (pid_str, after) = rest.split_once(' ')?;
    let after = after.strip_prefix("consumed ")?;
    let (n_str, _rest) = after.split_once(' ')?;
    let pid = Pubkey::from_str(pid_str).ok()?;
    let n: u64 = n_str.parse().ok()?;
    Some((pid, n))
}

fn parse_success(line: &str) -> Option<Pubkey> {
    // "Program <pid> success"
    let rest = line.strip_prefix("Program ")?;
    let (pid_str, tail) = rest.split_once(' ')?;
    if tail != "success" {
        return None;
    }
    Pubkey::from_str(pid_str).ok()
}

fn parse_failed(line: &str) -> Option<String> {
    // "Program <pid> failed: <reason>"
    let rest = line.strip_prefix("Program ")?;
    let (_pid_str, tail) = rest.split_once(' ')?;
    let reason = tail.strip_prefix("failed: ")?;
    Some(reason.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_simple_invocation() {
        let logs = vec![
            "Program 11111111111111111111111111111111 invoke [1]".to_string(),
            "Program log: hello".to_string(),
            "Program 11111111111111111111111111111111 consumed 150 of 200000 compute units"
                .to_string(),
            "Program 11111111111111111111111111111111 success".to_string(),
        ];
        let out = parse(&logs);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].depth, 1);
        assert_eq!(out[0].compute_units_consumed, Some(150));
        assert!(matches!(out[0].status, InvocationStatus::Success));
        assert_eq!(out[0].messages, vec!["Program log: hello"]);
    }

    #[test]
    fn folds_nested_cpi() {
        let logs = vec![
            "Program AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA invoke [1]".to_string(),
            "Program 11111111111111111111111111111111 invoke [2]".to_string(),
            "Program 11111111111111111111111111111111 consumed 100 of 200000 compute units"
                .to_string(),
            "Program 11111111111111111111111111111111 success".to_string(),
            "Program AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA consumed 500 of 200000 compute units".to_string(),
            "Program AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA success".to_string(),
        ];
        let out = parse(&logs);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].depth, 1);
        assert_eq!(out[1].depth, 2);
        assert_eq!(out[1].compute_units_consumed, Some(100));
    }
}
