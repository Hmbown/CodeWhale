pub(crate) fn ensure_rustls_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

#[allow(dead_code)]
pub(crate) fn reqwest_client() -> reqwest::Client {
    ensure_rustls_crypto_provider();
    reqwest::Client::new()
}

pub(crate) fn reqwest_client_builder() -> reqwest::ClientBuilder {
    ensure_rustls_crypto_provider();
    reqwest::Client::builder()
}

/// Lazily build (once) and return a shared, process-wide `reqwest::Client`
/// stored in `cell`, configuring it via `configure` on first use.
///
/// Building a fresh `Client` per tool call sets up a new connection pool and
/// TLS context every time and forfeits connection reuse. Callers should keep
/// one static client per distinct configuration and apply per-request
/// timeouts with `RequestBuilder::timeout` instead of baking them into the
/// `ClientBuilder`.
pub(crate) fn shared_client(
    cell: &'static std::sync::OnceLock<reqwest::Client>,
    configure: impl FnOnce(reqwest::ClientBuilder) -> reqwest::ClientBuilder,
) -> reqwest::Result<&'static reqwest::Client> {
    if let Some(client) = cell.get() {
        return Ok(client);
    }
    let client = configure(reqwest_client_builder()).build()?;
    // A concurrent first use may have won the race; the losing client is
    // simply dropped.
    Ok(cell.get_or_init(|| client))
}

pub(crate) fn reqwest_blocking_client_builder() -> reqwest::blocking::ClientBuilder {
    ensure_rustls_crypto_provider();
    reqwest::blocking::Client::builder()
}
