// Fetch an Anchor program's IDL from its on-chain IDL account, decompress it,
// and parse it into a typed representation we can drive a Borsh decoder with.
//
// On-chain IDL account derivation (Anchor convention):
//   base = find_program_address(&[], program_id).0
//   idl_address = create_with_seed(&base, "anchor:idl", program_id)
//
// On-chain IDL account layout:
//   [0..8)   : 8-byte Anchor discriminator for IdlAccount (we don't validate)
//   [8..40)  : 32-byte authority pubkey
//   [40..44) : u32 little-endian length of compressed payload
//   [44..)   : zlib-compressed JSON IDL
//
// We support the Anchor >= 0.30 IDL format (explicit per-instruction
// discriminators, snake_case field names, structured types). Older IDLs are
// not yet handled — we surface that as "no IDL" so the caller can fall back.

use anyhow::{anyhow, Context, Result};
use flate2::read::ZlibDecoder;
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use solana_pubkey::Pubkey;
use std::collections::HashMap;
use std::io::Read;

use crate::rpc;

/// Top-level IDL view we actually use to decode instructions.
#[derive(Debug, Clone)]
pub struct Idl {
    pub program_name: String,
    pub instructions: Vec<IdlInstruction>,
    /// Custom-type definitions referenced from instruction args.
    pub types: HashMap<String, IdlTypeDef>,
    pub errors: Vec<IdlError>,
}

#[derive(Debug, Clone)]
pub struct IdlInstruction {
    pub name: String,
    /// 8-byte discriminator that prefixes the instruction data.
    pub discriminator: [u8; 8],
    pub args: Vec<IdlField>,
}

#[derive(Debug, Clone)]
pub struct IdlField {
    pub name: String,
    pub ty: IdlType,
}

#[derive(Debug, Clone)]
pub enum IdlType {
    Bool,
    U8,
    I8,
    U16,
    I16,
    U32,
    I32,
    U64,
    I64,
    U128,
    I128,
    F32,
    F64,
    Bytes,
    String,
    Pubkey,
    Option(Box<IdlType>),
    Vec(Box<IdlType>),
    Array(Box<IdlType>, usize),
    /// Named reference into `Idl::types`.
    Defined(String),
    /// Anything we don't yet model — record the JSON for diagnostic purposes.
    Unknown(Value),
}

#[derive(Debug, Clone)]
pub enum IdlTypeDef {
    Struct(Vec<IdlField>),
    /// Enum variants with optional payloads — payloads not yet decoded.
    Enum(Vec<String>),
}

#[derive(Debug, Clone)]
pub struct IdlError {
    pub code: u32,
    pub name: String,
    pub msg: Option<String>,
}

// ---------------------------------------------------------------------------
// Address derivation
// ---------------------------------------------------------------------------

pub fn idl_address(program_id: &Pubkey) -> Result<Pubkey> {
    let (base, _bump) = Pubkey::find_program_address(&[], program_id);
    Pubkey::create_with_seed(&base, "anchor:idl", program_id)
        .map_err(|e| anyhow!("create_with_seed failed: {e}"))
}

// ---------------------------------------------------------------------------
// On-chain fetch
// ---------------------------------------------------------------------------

pub fn fetch(rpc_url: &str, program_id: &Pubkey) -> Result<Option<Idl>> {
    let addr = idl_address(program_id)?;
    let raw = match rpc::fetch_account_data(rpc_url, &addr.to_string())? {
        Some(b) => b,
        None => return Ok(None),
    };
    if raw.len() < 44 {
        return Ok(None);
    }

    // Skip 8-byte account discriminator + 32-byte authority, then read u32 length.
    let len_bytes = &raw[40..44];
    let data_len = u32::from_le_bytes([len_bytes[0], len_bytes[1], len_bytes[2], len_bytes[3]])
        as usize;
    let body_start: usize = 44;
    let body_end = body_start
        .checked_add(data_len)
        .ok_or_else(|| anyhow!("IDL length overflow"))?;
    if body_end > raw.len() {
        return Err(anyhow!(
            "IDL claims {data_len} compressed bytes but account only has {}",
            raw.len() - body_start
        ));
    }
    let compressed = &raw[body_start..body_end];

    let mut decoder = ZlibDecoder::new(compressed);
    let mut json_bytes = Vec::new();
    decoder
        .read_to_end(&mut json_bytes)
        .context("failed to zlib-decompress IDL payload")?;

    let json: Value = serde_json::from_slice(&json_bytes)
        .context("IDL payload is not valid JSON")?;

    Ok(Some(parse_idl(json)))
}

// ---------------------------------------------------------------------------
// JSON → typed IDL
// ---------------------------------------------------------------------------

fn parse_idl(json: Value) -> Idl {
    let program_name = json
        .get("metadata")
        .and_then(|m| m.get("name"))
        .and_then(|n| n.as_str())
        .or_else(|| json.get("name").and_then(|n| n.as_str()))
        .unwrap_or("(unnamed)")
        .to_string();

    let instructions = json
        .get("instructions")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(parse_instruction).collect())
        .unwrap_or_default();

    let mut types = HashMap::new();
    if let Some(arr) = json.get("types").and_then(|v| v.as_array()) {
        for entry in arr {
            if let Some((name, def)) = parse_typedef(entry) {
                types.insert(name, def);
            }
        }
    }

    let errors = json
        .get("errors")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(parse_error).collect())
        .unwrap_or_default();

    Idl {
        program_name,
        instructions,
        types,
        errors,
    }
}

fn parse_instruction(entry: &Value) -> Option<IdlInstruction> {
    let name = entry.get("name")?.as_str()?.to_string();
    // New-format IDLs (Anchor >= 0.30) carry an explicit discriminator.
    // Old-format IDLs don't — Anchor computed it at compile time as
    // sha256("global:" + snake_case(name))[..8]. Replicate that here so
    // pre-0.30 programs (whose IDL names are camelCase) still decode.
    let discriminator = entry
        .get("discriminator")
        .and_then(parse_discriminator)
        .unwrap_or_else(|| compute_global_discriminator(&name));
    let args = entry
        .get("args")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(parse_field).collect())
        .unwrap_or_default();
    Some(IdlInstruction {
        name,
        discriminator,
        args,
    })
}

/// Anchor's runtime discriminator: sha256("global:" + snake_case(name))[..8].
pub fn compute_global_discriminator(name: &str) -> [u8; 8] {
    let snake = camel_to_snake(name);
    let preimage = format!("global:{snake}");
    let hash = Sha256::digest(preimage.as_bytes());
    let mut out = [0u8; 8];
    out.copy_from_slice(&hash[..8]);
    out
}

/// Convert a camelCase or PascalCase identifier to snake_case.
/// Idempotent on names that are already snake_case.
fn camel_to_snake(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && c.is_ascii_uppercase() {
            out.push('_');
        }
        out.push(c.to_ascii_lowercase());
    }
    out
}

fn parse_discriminator(v: &Value) -> Option<[u8; 8]> {
    let arr = v.as_array()?;
    if arr.len() != 8 {
        return None;
    }
    let mut out = [0u8; 8];
    for (i, x) in arr.iter().enumerate() {
        out[i] = x.as_u64()? as u8;
    }
    Some(out)
}

fn parse_field(entry: &Value) -> Option<IdlField> {
    let name = entry.get("name")?.as_str()?.to_string();
    let ty = entry.get("type").map(parse_type).unwrap_or(IdlType::Unknown(Value::Null));
    Some(IdlField { name, ty })
}

fn parse_type(v: &Value) -> IdlType {
    if let Some(s) = v.as_str() {
        return primitive(s);
    }
    if let Some(obj) = v.as_object() {
        if let Some(inner) = obj.get("option") {
            return IdlType::Option(Box::new(parse_type(inner)));
        }
        if let Some(inner) = obj.get("vec") {
            return IdlType::Vec(Box::new(parse_type(inner)));
        }
        if let Some(arr) = obj.get("array").and_then(|v| v.as_array()) {
            if arr.len() == 2 {
                let inner = parse_type(&arr[0]);
                if let Some(n) = arr[1].as_u64() {
                    return IdlType::Array(Box::new(inner), n as usize);
                }
            }
        }
        if let Some(d) = obj.get("defined") {
            // New format: { "defined": { "name": "Foo" } }
            if let Some(name) = d.get("name").and_then(|n| n.as_str()) {
                return IdlType::Defined(name.to_string());
            }
            // Old format: { "defined": "Foo" }
            if let Some(name) = d.as_str() {
                return IdlType::Defined(name.to_string());
            }
        }
    }
    IdlType::Unknown(v.clone())
}

fn primitive(s: &str) -> IdlType {
    match s {
        "bool" => IdlType::Bool,
        "u8" => IdlType::U8,
        "i8" => IdlType::I8,
        "u16" => IdlType::U16,
        "i16" => IdlType::I16,
        "u32" => IdlType::U32,
        "i32" => IdlType::I32,
        "u64" => IdlType::U64,
        "i64" => IdlType::I64,
        "u128" => IdlType::U128,
        "i128" => IdlType::I128,
        "f32" => IdlType::F32,
        "f64" => IdlType::F64,
        "bytes" => IdlType::Bytes,
        "string" => IdlType::String,
        "pubkey" | "publicKey" => IdlType::Pubkey,
        other => IdlType::Unknown(Value::String(other.to_string())),
    }
}

fn parse_typedef(entry: &Value) -> Option<(String, IdlTypeDef)> {
    let name = entry.get("name")?.as_str()?.to_string();
    let ty = entry.get("type")?;
    let kind = ty.get("kind")?.as_str()?;
    match kind {
        "struct" => {
            let fields = ty
                .get("fields")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(parse_field).collect())
                .unwrap_or_default();
            Some((name, IdlTypeDef::Struct(fields)))
        }
        "enum" => {
            let variants = ty
                .get("variants")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.get("name").and_then(|n| n.as_str()).map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            Some((name, IdlTypeDef::Enum(variants)))
        }
        _ => None,
    }
}

#[derive(Deserialize)]
struct RawError {
    code: u32,
    name: String,
    #[serde(default)]
    msg: Option<String>,
}

fn parse_error(entry: &Value) -> Option<IdlError> {
    let raw: RawError = serde_json::from_value(entry.clone()).ok()?;
    Some(IdlError {
        code: raw.code,
        name: raw.name,
        msg: raw.msg,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camel_to_snake_handles_common_cases() {
        assert_eq!(camel_to_snake("initialize"), "initialize");
        assert_eq!(camel_to_snake("initializeUser"), "initialize_user");
        assert_eq!(camel_to_snake("InitializeUser"), "initialize_user");
        assert_eq!(camel_to_snake("amountIn"), "amount_in");
        assert_eq!(camel_to_snake("alreadySnakeCase"), "already_snake_case");
        assert_eq!(camel_to_snake("already_snake"), "already_snake");
        assert_eq!(camel_to_snake(""), "");
    }

    #[test]
    fn discriminator_is_deterministic_and_8_bytes() {
        let a = compute_global_discriminator("initialize");
        let b = compute_global_discriminator("initialize");
        assert_eq!(a, b);
        assert_eq!(a.len(), 8);
    }

    #[test]
    fn discriminator_matches_across_camel_and_snake() {
        // Old-format IDL names are camelCase; Anchor hashes the snake_case form.
        // So both spellings must yield the same discriminator.
        let camel = compute_global_discriminator("initializeUser");
        let snake = compute_global_discriminator("initialize_user");
        assert_eq!(camel, snake);
    }

    #[test]
    fn discriminator_differs_across_names() {
        assert_ne!(
            compute_global_discriminator("initialize"),
            compute_global_discriminator("close")
        );
    }

    #[test]
    fn parse_instruction_falls_back_to_computed_discriminator() {
        // An IDL entry without an explicit "discriminator" field — old format.
        let entry = serde_json::json!({
            "name": "initializeUser",
            "args": []
        });
        let ix = parse_instruction(&entry).expect("parsed");
        assert_eq!(ix.name, "initializeUser");
        assert_eq!(ix.discriminator, compute_global_discriminator("initialize_user"));
    }

    #[test]
    fn parse_instruction_prefers_explicit_discriminator() {
        let entry = serde_json::json!({
            "name": "swap",
            "discriminator": [1, 2, 3, 4, 5, 6, 7, 8],
            "args": []
        });
        let ix = parse_instruction(&entry).expect("parsed");
        assert_eq!(ix.discriminator, [1, 2, 3, 4, 5, 6, 7, 8]);
    }
}
