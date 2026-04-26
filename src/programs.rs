// Tiny registry of well-known program IDs → human-readable labels.
// Used as a fallback when a program has no on-chain Anchor IDL we can fetch.

pub fn label(program_id: &str) -> Option<&'static str> {
    Some(match program_id {
        "11111111111111111111111111111111" => "system",
        "ComputeBudget111111111111111111111111111111" => "compute-budget",
        "Vote111111111111111111111111111111111111111" => "vote",
        "Stake11111111111111111111111111111111111111" => "stake",
        "Config1111111111111111111111111111111111111" => "config",
        "AddressLookupTab1e1111111111111111111111111" => "alt",
        "BPFLoader2111111111111111111111111111111111" => "bpf-loader-2",
        "BPFLoaderUpgradeab1e11111111111111111111111" => "bpf-loader-upgradeable",
        "Ed25519SigVerify111111111111111111111111111" => "ed25519-precompile",
        "KeccakSecp256k11111111111111111111111111111" => "secp256k1-precompile",

        "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA" => "spl-token",
        "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb" => "spl-token-2022",
        "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL" => "spl-associated-token-account",
        "Memo1UhkJRfHyvLMcVucJwxXeuD728EqVDDwQDxFMNo" => "spl-memo (v1)",
        "MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr" => "spl-memo",
        "namesLPneVptA9Z5rqUDD9tMTWEJwofgaYwp8cawRkX" => "spl-name-service",
        "metaqbxxUerdq28cj1RbAWkYQm3ybzjb6a8bt518x1s" => "metaplex-token-metadata",

        "JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4" => "jupiter v6",
        "whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc" => "orca whirlpool",
        "9W959DqEETiGZocYWCQPaJ6sBmUzgfxXfqGeTEdp3aQP" => "phoenix",
        "MarBmsSgKXdrN1egZf5sqe1TMThczhMLJhrAGZIpKQq" => "marinade",
        "dRiftyHA39MWEi3m9aunc5MzRF1JYuBsbn6VPcn33UH" => "drift v2",
        _ => return None,
    })
}
