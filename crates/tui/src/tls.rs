pub(crate) fn ensure_rustls_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

#[allow(dead_code)] // Used by tests (runtime_api/tests.rs); callers migrated to reqwest_client_builder (see #3490)
pub(crate) fn reqwest_client() -> reqwest::Client {
    ensure_rustls_crypto_provider();
    reqwest::Client::new()
}

pub(crate) fn reqwest_client_builder() -> reqwest::ClientBuilder {
    ensure_rustls_crypto_provider();
    reqwest::Client::builder()
}

pub(crate) fn reqwest_blocking_client_builder() -> reqwest::blocking::ClientBuilder {
    ensure_rustls_crypto_provider();
    reqwest::blocking::Client::builder()
}
