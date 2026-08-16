//! Process-wide TLS bootstrap plus the platform HTTP client constructors that
//! depend on it. Every reqwest client Codewhale builds goes through here so
//! the rustls crypto provider is installed exactly once, before the first
//! client, on every path (TUI, CLI, tests).

/// Install the rustls `ring` crypto provider if no provider is installed yet.
/// Idempotent; a second call is a no-op.
pub fn ensure_rustls_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// A ready platform HTTP client (provider installed, platform verifier).
pub fn reqwest_client() -> reqwest::Client {
    ensure_rustls_crypto_provider();
    reqwest_client_builder()
        .build()
        .expect("build platform HTTP client")
}

/// The platform HTTP client builder, with the crypto provider installed.
pub fn reqwest_client_builder() -> reqwest::ClientBuilder {
    ensure_rustls_crypto_provider();
    crate::platform_http_client_builder()
}

/// The blocking platform HTTP client builder, with the crypto provider installed.
pub fn reqwest_blocking_client_builder() -> reqwest::blocking::ClientBuilder {
    ensure_rustls_crypto_provider();
    crate::platform_blocking_http_client_builder()
}
