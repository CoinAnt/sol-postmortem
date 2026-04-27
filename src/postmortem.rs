// In-memory representation of a transaction postmortem, built once from the
// raw RPC response and then handed to either the terminal renderer or the
// JSON serializer. Keeps the data extraction in one place and makes the
// JSON schema explicit for downstream tooling.

use serde::Serialize;
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

use crate::decode::{self, DecodeOutcome};
use crate::diffs::{self, DiffSummary};
use crate::idl::{self, Idl};
use crate::logs::{self, InvocationStatus, ProgramInvocation};
use crate::programs;

#[derive(Debug, Serialize)]
pub struct Postmortem {
    pub signature: String,
    pub slot: u64,
    pub block_time: Option<i64>,
    pub fee_lamports: u64,
    pub status: PostmortemStatus,
    pub trace: Vec<TraceNode>,
    pub diffs: DiffSummary,
}

#[derive(Debug, Serialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum PostmortemStatus {
    Success,
    Failed {
        /// The top-level instruction index that failed, when the error is an
        /// InstructionError. None for non-instruction tx-level failures.
        instruction_index: Option<u32>,
        /// Top-level program at the failing instruction index.
        top_program_id: Option<String>,
        /// Top-level program label resolved from IDL or known-program registry.
        top_program_label: Option<String>,
        /// Raw error code as it appeared in the RPC response (e.g. "Custom(101)").
        code: String,
        /// Resolved name when we could enrich it (IDL lookup or framework table).
        name: Option<String>,
        /// Where the resolved name came from.
        source: Option<NameSource>,
        /// When the error originated below the top level (CPI failure that
        /// propagated up), the deepest program in the failed chain whose IDL
        /// matched the code.
        originating_program_id: Option<String>,
        originating_program_label: Option<String>,
    },
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NameSource {
    Idl,
    AnchorFramework,
}

#[derive(Debug, Serialize)]
pub struct TraceNode {
    pub depth: usize,
    pub program_id: String,
    pub program_label: String,
    pub instruction: Option<DecodedInstruction>,
    pub compute_units: Option<u64>,
    pub status: NodeStatus,
    pub fail_reason: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeStatus {
    Ok,
    Fail,
    Unknown,
}

#[derive(Debug, Serialize)]
pub struct DecodedInstruction {
    pub name: String,
    pub args: Vec<DecodedArg>,
    /// Set when arg decoding got partway through and then failed; the args
    /// vector contains everything we managed to decode before the error.
    pub partial_decode_error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct DecodedArg {
    pub name: String,
    /// Stringified value (Borsh decoder produces strings — large ints fit
    /// safely without JS precision loss).
    pub value: String,
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

pub fn assemble(
    rpc_url: &str,
    signature: &str,
    tx: &EncodedConfirmedTransactionWithStatusMeta,
) -> Postmortem {
    let meta = tx
        .transaction
        .meta
        .as_ref()
        .expect("rpc::fetch_transaction guarantees meta is present");

    let log_messages: Vec<String> = match &meta.log_messages {
        OptionSerializer::Some(v) => v.clone(),
        _ => Vec::new(),
    };
    let inner_instructions: &[UiInnerInstructions] = match &meta.inner_instructions {
        OptionSerializer::Some(v) => v.as_slice(),
        _ => &[],
    };

    let executed = build_executed(&tx.transaction.transaction, inner_instructions);

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
    let trace = build_trace(&invocations, &executed, &idl_cache);

    let status = build_status(meta.err.as_ref(), &invocations, &idl_cache);
    let diffs = diffs::compute(&tx.transaction.transaction, meta);

    Postmortem {
        signature: signature.to_string(),
        slot: tx.slot,
        block_time: tx.block_time,
        fee_lamports: meta.fee,
        status,
        trace,
        diffs,
    }
}

// ---------------------------------------------------------------------------
// Executed-instruction flattening (top-level + inner_instructions in order)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct ExecutedIx {
    program_id: String,
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
// Trace assembly
// ---------------------------------------------------------------------------

fn build_trace(
    invocations: &[ProgramInvocation],
    executed: &[ExecutedIx],
    idl_cache: &HashMap<String, Option<Idl>>,
) -> Vec<TraceNode> {
    let mut out = Vec::with_capacity(invocations.len());
    let mut exec_iter = executed.iter();

    for inv in invocations {
        let exec = exec_iter.next();
        let pid_str = inv.program_id.to_string();

        let instruction = match exec {
            Some(ix) if ix.program_id == pid_str => decode_for(&ix.program_id, &ix.data, idl_cache),
            _ => None,
        };

        let (status, fail_reason) = match &inv.status {
            InvocationStatus::Success => (NodeStatus::Ok, None),
            InvocationStatus::Failed(r) => (NodeStatus::Fail, Some(r.clone())),
            InvocationStatus::Unknown => (NodeStatus::Unknown, None),
        };

        out.push(TraceNode {
            depth: inv.depth,
            program_id: pid_str.clone(),
            program_label: label_for(&pid_str, idl_cache),
            instruction,
            compute_units: inv.compute_units_consumed,
            status,
            fail_reason,
        });
    }
    out
}

fn decode_for(
    program_id: &str,
    data: &[u8],
    idl_cache: &HashMap<String, Option<Idl>>,
) -> Option<DecodedInstruction> {
    let idl = idl_cache.get(program_id)?.as_ref()?;
    match decode::decode_instruction(idl, data) {
        DecodeOutcome::Decoded { ix_name, args } => Some(DecodedInstruction {
            name: ix_name,
            args: args
                .into_iter()
                .map(|(name, value)| DecodedArg { name, value })
                .collect(),
            partial_decode_error: None,
        }),
        DecodeOutcome::PartiallyDecoded {
            ix_name,
            args,
            error,
        } => Some(DecodedInstruction {
            name: ix_name,
            args: args
                .into_iter()
                .map(|(name, value)| DecodedArg { name, value })
                .collect(),
            partial_decode_error: Some(error),
        }),
        DecodeOutcome::NoMatch => None,
    }
}

pub fn label_for(program_id: &str, idl_cache: &HashMap<String, Option<Idl>>) -> String {
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
// Status assembly
// ---------------------------------------------------------------------------

fn build_status(
    err: Option<&TransactionError>,
    invocations: &[ProgramInvocation],
    idl_cache: &HashMap<String, Option<Idl>>,
) -> PostmortemStatus {
    let Some(err) = err else {
        return PostmortemStatus::Success;
    };

    let TransactionError::InstructionError(idx, ref ix_err) = err else {
        return PostmortemStatus::Failed {
            instruction_index: None,
            top_program_id: None,
            top_program_label: None,
            code: format!("{err:?}"),
            name: None,
            source: None,
            originating_program_id: None,
            originating_program_label: None,
        };
    };

    let idx = *idx as usize;
    let top_pid = invocations
        .iter()
        .filter(|inv| inv.depth == 1)
        .nth(idx)
        .map(|inv| inv.program_id.to_string());
    let top_label = top_pid.as_deref().map(|p| label_for(p, idl_cache));

    let code = format!("{ix_err:?}");

    let (name, source, origin_pid) = if let InstructionError::Custom(c) = ix_err {
        // 1. Walk the failed CPI chain from deepest up.
        let from_idl = failed_chain_from_deepest(invocations)
            .into_iter()
            .find_map(|pid| {
                let idl = idl_cache.get(&pid.to_string())?.as_ref()?;
                decode::lookup_error(idl, *c).map(|name| (name, pid))
            });
        match from_idl {
            Some((n, pid)) => (Some(n), Some(NameSource::Idl), Some(pid.to_string())),
            None => match decode::anchor_framework_error(*c) {
                Some(n) => (
                    Some(format!("{n} (Anchor framework)")),
                    Some(NameSource::AnchorFramework),
                    None,
                ),
                None => (None, None, None),
            },
        }
    } else {
        (None, None, None)
    };

    let origin_label = origin_pid.as_deref().map(|p| label_for(p, idl_cache));

    PostmortemStatus::Failed {
        instruction_index: Some(idx as u32),
        top_program_id: top_pid,
        top_program_label: top_label,
        code,
        name,
        source,
        originating_program_id: origin_pid,
        originating_program_label: origin_label,
    }
}

fn failed_chain_from_deepest(invocations: &[ProgramInvocation]) -> Vec<Pubkey> {
    let mut failed: Vec<&ProgramInvocation> = invocations
        .iter()
        .filter(|inv| matches!(inv.status, InvocationStatus::Failed(_)))
        .collect();
    failed.sort_by(|a, b| b.depth.cmp(&a.depth));
    failed.into_iter().map(|inv| inv.program_id).collect()
}
