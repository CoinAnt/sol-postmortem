// Decode an Anchor-style instruction's args into a human-readable string,
// using the program's IDL to drive a Borsh deserializer.
//
// Borsh format reference (only the parts we use):
//   bool         : 1 byte (0 or 1)
//   u8/i8        : 1 byte
//   u16/i16      : 2 bytes LE
//   u32/i32      : 4 bytes LE
//   u64/i64      : 8 bytes LE
//   u128/i128    : 16 bytes LE
//   f32/f64      : IEEE-754 LE
//   bytes/string : u32 length LE, then bytes
//   option<T>    : 1 byte (0 = None, 1 = Some), then T if Some
//   vec<T>       : u32 length LE, then length × T
//   array<T,N>   : N × T (no length prefix)
//   pubkey       : 32 bytes
//   struct       : fields in declaration order, no separators

use crate::idl::{Idl, IdlType, IdlTypeDef};
use anyhow::{anyhow, bail, Result};
use solana_pubkey::Pubkey;

/// Outcome of trying to decode an instruction's data against an IDL.
#[derive(Debug)]
pub enum DecodeOutcome {
    /// We matched a discriminator and successfully decoded all args.
    Decoded {
        ix_name: String,
        args: Vec<(String, String)>,
    },
    /// Discriminator matched a known instruction but arg decoding failed
    /// partway through. Includes whatever we managed to decode.
    PartiallyDecoded {
        ix_name: String,
        args: Vec<(String, String)>,
        error: String,
    },
    /// No instruction in the IDL matched the data's first 8 bytes.
    NoMatch,
}

pub fn decode_instruction(idl: &Idl, data: &[u8]) -> DecodeOutcome {
    if data.len() < 8 {
        return DecodeOutcome::NoMatch;
    }
    let discriminator: [u8; 8] = data[..8].try_into().expect("len-checked above");

    let Some(ix) = idl
        .instructions
        .iter()
        .find(|ix| ix.discriminator == discriminator)
    else {
        return DecodeOutcome::NoMatch;
    };

    let mut cursor = Cursor::new(&data[8..]);
    let mut args = Vec::with_capacity(ix.args.len());
    for field in &ix.args {
        match decode_value(&mut cursor, &field.ty, idl) {
            Ok(v) => args.push((field.name.clone(), v)),
            Err(e) => {
                return DecodeOutcome::PartiallyDecoded {
                    ix_name: ix.name.clone(),
                    args,
                    error: format!("at field `{}`: {e}", field.name),
                };
            }
        }
    }

    DecodeOutcome::Decoded {
        ix_name: ix.name.clone(),
        args,
    }
}

// ---------------------------------------------------------------------------
// Borsh cursor + recursive decoder
// ---------------------------------------------------------------------------

struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn read(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.pos + n > self.buf.len() {
            bail!(
                "out of bytes: need {n} more at offset {} (have {} total)",
                self.pos,
                self.buf.len()
            );
        }
        let slice = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(slice)
    }

    fn read_u8(&mut self) -> Result<u8> {
        Ok(self.read(1)?[0])
    }

    fn read_u32(&mut self) -> Result<u32> {
        let b = self.read(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
}

fn decode_value(cur: &mut Cursor, ty: &IdlType, idl: &Idl) -> Result<String> {
    Ok(match ty {
        IdlType::Bool => match cur.read_u8()? {
            0 => "false".to_string(),
            1 => "true".to_string(),
            x => bail!("invalid bool byte: {x}"),
        },
        IdlType::U8 => format!("{}", cur.read_u8()?),
        IdlType::I8 => format!("{}", cur.read_u8()? as i8),
        IdlType::U16 => {
            let b = cur.read(2)?;
            format!("{}", u16::from_le_bytes([b[0], b[1]]))
        }
        IdlType::I16 => {
            let b = cur.read(2)?;
            format!("{}", i16::from_le_bytes([b[0], b[1]]))
        }
        IdlType::U32 => format!("{}", cur.read_u32()?),
        IdlType::I32 => {
            let b = cur.read(4)?;
            format!("{}", i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        }
        IdlType::U64 => {
            let b = cur.read(8)?;
            let mut a = [0u8; 8];
            a.copy_from_slice(b);
            format!("{}", u64::from_le_bytes(a))
        }
        IdlType::I64 => {
            let b = cur.read(8)?;
            let mut a = [0u8; 8];
            a.copy_from_slice(b);
            format!("{}", i64::from_le_bytes(a))
        }
        IdlType::U128 => {
            let b = cur.read(16)?;
            let mut a = [0u8; 16];
            a.copy_from_slice(b);
            format!("{}", u128::from_le_bytes(a))
        }
        IdlType::I128 => {
            let b = cur.read(16)?;
            let mut a = [0u8; 16];
            a.copy_from_slice(b);
            format!("{}", i128::from_le_bytes(a))
        }
        IdlType::F32 => {
            let b = cur.read(4)?;
            format!("{}", f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        }
        IdlType::F64 => {
            let b = cur.read(8)?;
            let mut a = [0u8; 8];
            a.copy_from_slice(b);
            format!("{}", f64::from_le_bytes(a))
        }
        IdlType::String => {
            let len = cur.read_u32()? as usize;
            let bytes = cur.read(len)?;
            let s = std::str::from_utf8(bytes)
                .map_err(|e| anyhow!("string was not valid UTF-8: {e}"))?;
            format!("\"{s}\"")
        }
        IdlType::Bytes => {
            let len = cur.read_u32()? as usize;
            let bytes = cur.read(len)?;
            // Show short bytestrings inline, summarise long ones.
            if len <= 16 {
                format!("0x{}", hex::encode(bytes))
            } else {
                format!("<{len} bytes: 0x{}…>", hex::encode(&bytes[..8]))
            }
        }
        IdlType::Pubkey => {
            let b = cur.read(32)?;
            let mut a = [0u8; 32];
            a.copy_from_slice(b);
            Pubkey::new_from_array(a).to_string()
        }
        IdlType::Option(inner) => {
            let tag = cur.read_u8()?;
            match tag {
                0 => "None".to_string(),
                1 => format!("Some({})", decode_value(cur, inner, idl)?),
                x => bail!("invalid option tag: {x}"),
            }
        }
        IdlType::Vec(inner) => {
            let n = cur.read_u32()? as usize;
            let mut elems = Vec::with_capacity(n);
            for _ in 0..n {
                elems.push(decode_value(cur, inner, idl)?);
            }
            format!("[{}]", elems.join(", "))
        }
        IdlType::Array(inner, n) => {
            let mut elems = Vec::with_capacity(*n);
            for _ in 0..*n {
                elems.push(decode_value(cur, inner, idl)?);
            }
            format!("[{}]", elems.join(", "))
        }
        IdlType::Defined(name) => match idl.types.get(name) {
            Some(IdlTypeDef::Struct(fields)) => {
                let mut parts = Vec::with_capacity(fields.len());
                for field in fields {
                    let v = decode_value(cur, &field.ty, idl)?;
                    parts.push(format!("{}: {}", field.name, v));
                }
                format!("{name} {{ {} }}", parts.join(", "))
            }
            Some(IdlTypeDef::Enum(variants)) => {
                let tag = cur.read_u8()? as usize;
                let v = variants
                    .get(tag)
                    .cloned()
                    .unwrap_or_else(|| format!("<unknown variant {tag}>"));
                format!("{name}::{v}")
            }
            None => bail!("type `{name}` referenced but not defined in IDL"),
        },
        IdlType::Unknown(v) => bail!("unsupported type in IDL: {v}"),
    })
}

/// Look up an Anchor-style program error code in the IDL's errors table.
pub fn lookup_error(idl: &Idl, code: u32) -> Option<String> {
    let e = idl.errors.iter().find(|e| e.code == code)?;
    Some(match &e.msg {
        Some(m) => format!("{} ({})", e.name, m),
        None => e.name.clone(),
    })
}

/// Anchor framework's built-in error codes (anchor_lang::error::ErrorCode).
/// These are not in any program's IDL — Anchor injects them at the framework
/// level, e.g. when account constraints fail or a discriminator doesn't match.
/// Returns the framework error name, suitable for display as a fallback.
pub fn anchor_framework_error(code: u32) -> Option<&'static str> {
    Some(match code {
        // Instructions
        100 => "InstructionMissing",
        101 => "InstructionFallbackNotFound",
        102 => "InstructionDidNotDeserialize",
        103 => "InstructionDidNotSerialize",
        // IDL instructions
        1000 => "IdlInstructionStub",
        1001 => "IdlInstructionInvalidProgram",
        1002 => "IdlAccountNotEmpty",
        // Event instructions
        1500 => "EventInstructionStub",
        // Constraints
        2000 => "ConstraintMut",
        2001 => "ConstraintHasOne",
        2002 => "ConstraintSigner",
        2003 => "ConstraintRaw",
        2004 => "ConstraintOwner",
        2005 => "ConstraintRentExempt",
        2006 => "ConstraintSeeds",
        2007 => "ConstraintExecutable",
        2008 => "ConstraintState",
        2009 => "ConstraintAssociated",
        2010 => "ConstraintAssociatedInit",
        2011 => "ConstraintClose",
        2012 => "ConstraintAddress",
        2013 => "ConstraintZero",
        2014 => "ConstraintTokenMint",
        2015 => "ConstraintTokenOwner",
        2016 => "ConstraintMintMintAuthority",
        2017 => "ConstraintMintFreezeAuthority",
        2018 => "ConstraintMintDecimals",
        2019 => "ConstraintSpace",
        2020 => "ConstraintAccountIsNone",
        2021 => "ConstraintTokenTokenProgram",
        2022 => "ConstraintMintTokenProgram",
        2023 => "ConstraintAssociatedTokenTokenProgram",
        // Require
        2500 => "RequireViolated",
        2501 => "RequireEqViolated",
        2502 => "RequireKeysEqViolated",
        2503 => "RequireNeqViolated",
        2504 => "RequireKeysNeqViolated",
        2505 => "RequireGtViolated",
        2506 => "RequireGteViolated",
        // Accounts
        3000 => "AccountDiscriminatorAlreadySet",
        3001 => "AccountDiscriminatorNotFound",
        3002 => "AccountDiscriminatorMismatch",
        3003 => "AccountDidNotDeserialize",
        3004 => "AccountDidNotSerialize",
        3005 => "AccountNotEnoughKeys",
        3006 => "AccountNotMutable",
        3007 => "AccountOwnedByWrongProgram",
        3008 => "InvalidProgramId",
        3009 => "InvalidProgramExecutable",
        3010 => "AccountNotSigner",
        3011 => "AccountNotSystemOwned",
        3012 => "AccountNotInitialized",
        3013 => "AccountNotProgramData",
        3014 => "AccountNotAssociatedTokenAccount",
        3015 => "AccountSysvarMismatch",
        3016 => "AccountReallocExceedsLimit",
        3017 => "AccountDuplicateReallocs",
        // Misc
        4100 => "DeclaredProgramIdMismatch",
        4101 => "TryingToInitPayerAsProgramAccount",
        4102 => "InvalidNumericConversion",
        5000 => "Deprecated",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::idl::{IdlError, IdlField, IdlInstruction};
    use std::collections::HashMap;

    fn idl_with(ix: IdlInstruction) -> Idl {
        Idl {
            program_name: "test".into(),
            instructions: vec![ix],
            types: HashMap::new(),
            errors: vec![],
        }
    }

    #[test]
    fn decodes_two_u64_args() {
        let ix = IdlInstruction {
            name: "swap".into(),
            discriminator: [1, 2, 3, 4, 5, 6, 7, 8],
            args: vec![
                IdlField {
                    name: "amount_in".into(),
                    ty: IdlType::U64,
                },
                IdlField {
                    name: "min_out".into(),
                    ty: IdlType::U64,
                },
            ],
        };
        let idl = idl_with(ix);

        let mut data = vec![1, 2, 3, 4, 5, 6, 7, 8];
        data.extend_from_slice(&1_000_000u64.to_le_bytes());
        data.extend_from_slice(&950_000u64.to_le_bytes());

        let out = decode_instruction(&idl, &data);
        match out {
            DecodeOutcome::Decoded { ix_name, args } => {
                assert_eq!(ix_name, "swap");
                assert_eq!(
                    args,
                    vec![
                        ("amount_in".into(), "1000000".into()),
                        ("min_out".into(), "950000".into()),
                    ]
                );
            }
            other => panic!("expected Decoded, got {other:?}"),
        }
    }

    #[test]
    fn no_match_when_discriminator_differs() {
        let ix = IdlInstruction {
            name: "swap".into(),
            discriminator: [1; 8],
            args: vec![],
        };
        let idl = idl_with(ix);
        let out = decode_instruction(&idl, &[9; 8]);
        assert!(matches!(out, DecodeOutcome::NoMatch));
    }

    #[test]
    fn looks_up_idl_error_with_message() {
        let idl = Idl {
            program_name: "test".into(),
            instructions: vec![],
            types: HashMap::new(),
            errors: vec![
                IdlError {
                    code: 6000,
                    name: "InsufficientLiquidity".into(),
                    msg: Some("not enough liquidity to fill the order".into()),
                },
                IdlError {
                    code: 6001,
                    name: "SlippageExceeded".into(),
                    msg: None,
                },
            ],
        };
        assert_eq!(
            lookup_error(&idl, 6000).as_deref(),
            Some("InsufficientLiquidity (not enough liquidity to fill the order)")
        );
        assert_eq!(lookup_error(&idl, 6001).as_deref(), Some("SlippageExceeded"));
        assert_eq!(lookup_error(&idl, 9999), None);
    }

    #[test]
    fn anchor_framework_codes_resolve() {
        assert_eq!(anchor_framework_error(101), Some("InstructionFallbackNotFound"));
        assert_eq!(anchor_framework_error(2006), Some("ConstraintSeeds"));
        assert_eq!(anchor_framework_error(3007), Some("AccountOwnedByWrongProgram"));
        assert_eq!(anchor_framework_error(99), None);
        assert_eq!(anchor_framework_error(7777), None);
    }

    #[test]
    fn partially_decoded_when_data_runs_out() {
        let ix = IdlInstruction {
            name: "deposit".into(),
            discriminator: [0xaa; 8],
            args: vec![
                IdlField {
                    name: "amount".into(),
                    ty: IdlType::U64,
                },
                IdlField {
                    name: "memo".into(),
                    ty: IdlType::U64,
                },
            ],
        };
        let idl = idl_with(ix);
        let mut data = vec![0xaa; 8];
        data.extend_from_slice(&42u64.to_le_bytes());
        // missing the second u64

        let out = decode_instruction(&idl, &data);
        match out {
            DecodeOutcome::PartiallyDecoded { ix_name, args, .. } => {
                assert_eq!(ix_name, "deposit");
                assert_eq!(args, vec![("amount".into(), "42".into())]);
            }
            _ => panic!("expected PartiallyDecoded"),
        }
    }
}
