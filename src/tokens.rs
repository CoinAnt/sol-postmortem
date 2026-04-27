// Tiny registry of well-known SPL token mints → friendly symbols.
// Used to make the balance-change section more legible.

pub fn symbol(mint: &str) -> Option<&'static str> {
    Some(match mint {
        "So11111111111111111111111111111111111111112" => "WSOL",
        "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v" => "USDC",
        "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB" => "USDT",
        "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263" => "BONK",
        "JUPyiwrYJFskUPiHa7hkeR8VUtAeFoSYbKedZNsDvCN" => "JUP",
        "mSoLzYCxHdYgdzU16g5QSh3i5K3z3KZK7ytfqcJm7So" => "mSOL",
        "7vfCXTUXx5WJV5JADk17DUJ4ksgau7utNKj4b963voxs" => "ETH (Wormhole)",
        "5z3EqYQo9HiCEs3R84RCDMu2n7anpDMxRhdK8PSWmrRC" => "PONKE",
        _ => return None,
    })
}
