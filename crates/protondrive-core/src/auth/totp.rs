//! TOTP (RFC 6238) helper. We always use the standard 6-digit / 30-second / SHA-1
//! profile because that's what Proton uses for 2FA.

use totp_rs::{Algorithm, Secret, TOTP};

use crate::{Error, Result};

/// Generate the current 6-digit code from a Base32-encoded secret key.
pub fn current_code(base32_secret: &str) -> Result<String> {
    let secret = Secret::Encoded(base32_secret.trim().replace(' ', ""))
        .to_bytes()
        .map_err(|e| Error::Auth(format!("invalid TOTP secret: {e}")))?;
    let totp = TOTP::new(
        Algorithm::SHA1,
        6,
        1,
        30,
        secret,
        None,
        "Proton".to_string(),
    )
    .map_err(|e| Error::Auth(format!("could not build TOTP: {e}")))?;
    totp.generate_current()
        .map_err(|e| Error::Auth(format!("could not generate TOTP: {e}")))
}

/// Validate that a string parses as a Base32 TOTP secret. Used by the setup UI
/// before storing the secret in the keyring.
pub fn validate_secret(base32_secret: &str) -> Result<()> {
    current_code(base32_secret).map(|_| ())
}
