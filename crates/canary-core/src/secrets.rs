//! Secret generation for product-visible credentials.

const WEBHOOK_SECRET_ALPHABET: [char; 64] = [
    '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i',
    'j', 'k', 'l', 'm', 'n', 'o', 'p', 'q', 'r', 's', 't', 'u', 'v', 'w', 'x', 'y', 'z', 'A', 'B',
    'C', 'D', 'E', 'F', 'G', 'H', 'I', 'J', 'K', 'L', 'M', 'N', 'O', 'P', 'Q', 'R', 'S', 'T', 'U',
    'V', 'W', 'X', 'Y', 'Z', '_', '-',
];
const WEBHOOK_SECRET_LEN: usize = 32;
const API_KEY_SECRET_LEN: usize = 24;

/// Generate the shared secret returned once by the webhook create API.
pub fn webhook_secret() -> String {
    nanoid::nanoid!(WEBHOOK_SECRET_LEN, &WEBHOOK_SECRET_ALPHABET)
}

/// Generate the raw API key returned once by the key create API.
pub fn api_key(env: &str) -> String {
    format!("sk_{env}_{}", nanoid::nanoid!(API_KEY_SECRET_LEN))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn webhook_secret_has_phoenix_wire_length() {
        let secret = webhook_secret();

        assert_eq!(secret.len(), WEBHOOK_SECRET_LEN);
        assert!(
            secret
                .chars()
                .all(|character| WEBHOOK_SECRET_ALPHABET.contains(&character))
        );
    }

    #[test]
    fn api_key_matches_phoenix_wire_shape() {
        let key = api_key("live");

        assert!(key.starts_with("sk_live_"));
        assert_eq!(key.len(), "sk_live_".len() + API_KEY_SECRET_LEN);
    }
}
