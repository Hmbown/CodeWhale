use std::io::{self, Write};

use clap::Parser;

use super::*;
use crate::{AuthArgs, AuthCommand, Cli, Commands, no_keyring_secrets};

const SENTINEL: &str = "cw-test-secret-never-log-7b30";
const AMBIENT_SENTINEL: &str = "cw-test-ambient-openrouter-never-log-94cd";

fn without_ambient_credentials() -> Vec<crate::tests::ScopedEnvVar> {
    let mut names = ProviderKind::ALL
        .into_iter()
        .flat_map(crate::provider_env_vars)
        .copied()
        .collect::<Vec<_>>();
    names.extend([
        codewhale_config::CLI_API_KEY_ENV,
        codewhale_config::CLI_API_KEY_SOURCE_ENV,
        codewhale_config::LEGACY_CLI_API_KEY_SOURCE_ENV,
    ]);
    names.sort_unstable();
    names.dedup();
    names
        .into_iter()
        .map(crate::tests::ScopedEnvVar::remove)
        .collect()
}

fn expect_credential_error(result: anyhow::Result<String>, context: &'static str) -> anyhow::Error {
    match result {
        Err(error) => error,
        Ok(_) => panic!("{context}: unexpectedly resolved a credential"),
    }
}

#[test]
fn command_parses_and_documents_the_pipe_only_boundary() {
    let cli = Cli::try_parse_from([
        "codewhale",
        "auth",
        "print-api-key",
        "--provider",
        "openrouter",
    ])
    .expect("parse command");
    assert!(matches!(
        cli.command,
        Some(Commands::Auth(AuthArgs {
            command: AuthCommand::PrintApiKey {
                provider: ProviderKind::Openrouter
            }
        }))
    ));

    let help = Cli::try_parse_from(["codewhale", "auth", "print-api-key", "--help"])
        .expect_err("help exits")
        .to_string();
    assert!(help.contains("runtime-effective API key"), "{help}");
    assert!(help.contains("refuses terminals"), "{help}");
}

#[test]
fn pipe_receives_exact_secret_and_one_newline() {
    let mut output = Vec::new();
    handoff_secret_line(&mut output, false, || Ok(SENTINEL.to_string())).expect("handoff");
    assert_eq!(output, format!("{SENTINEL}\n").as_bytes());
}

#[test]
fn terminal_refusal_happens_before_credential_resolution() {
    let resolved = std::cell::Cell::new(false);
    let mut output = Vec::new();
    let error = handoff_secret_line(&mut output, true, || {
        resolved.set(true);
        Ok(SENTINEL.to_string())
    })
    .expect_err("terminal must be refused");
    assert!(
        !resolved.get(),
        "terminal refusal must not read a credential"
    );
    assert!(output.is_empty());
    assert!(error.to_string().contains("refusing"));
    assert!(!error.to_string().contains(SENTINEL));
}

struct BrokenPipe;

impl Write for BrokenPipe {
    fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
        Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed"))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn closed_consumer_is_a_clean_pipe_settlement() {
    handoff_secret_line(&mut BrokenPipe, false, || Ok(SENTINEL.to_string()))
        .expect("broken pipe is settled");
}

#[derive(Default)]
struct PartialThenBrokenPipe {
    output: Vec<u8>,
}

impl Write for PartialThenBrokenPipe {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.output.is_empty() {
            let accepted = buf.len().min(5);
            self.output.extend_from_slice(&buf[..accepted]);
            return Ok(accepted);
        }
        Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed"))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn partial_then_closed_consumer_is_also_a_clean_settlement() {
    let mut writer = PartialThenBrokenPipe::default();
    handoff_secret_line(&mut writer, false, || Ok(SENTINEL.to_string()))
        .expect("partial broken pipe is settled");
    assert_eq!(writer.output, SENTINEL.as_bytes()[..5]);
}

#[derive(Default)]
struct PartialThenOtherFailure {
    output: Vec<u8>,
}

impl Write for PartialThenOtherFailure {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.output.is_empty() {
            let accepted = buf.len().min(5);
            self.output.extend_from_slice(&buf[..accepted]);
            return Ok(accepted);
        }
        Err(io::Error::other("sentinel-shaped operating system detail"))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn output_failure_never_formats_secret_or_os_detail() {
    let mut writer = PartialThenOtherFailure::default();
    let error = handoff_secret_line(&mut writer, false, || Ok(SENTINEL.to_string()))
        .expect_err("non-pipe error after partial output");
    let rendered = error.to_string();
    assert_eq!(writer.output, SENTINEL.as_bytes()[..5]);
    assert!(!rendered.contains(SENTINEL), "{rendered}");
    assert!(!rendered.contains("sentinel-shaped"), "{rendered}");
}

#[test]
fn resolution_failure_is_redacted() {
    let mut output = Vec::new();
    let error = handoff_secret_line(&mut output, false, || {
        anyhow::bail!("provider failed near {SENTINEL}")
    })
    .expect_err("resolution failure");
    assert_eq!(error.to_string(), "unavailable credential");
    assert!(output.is_empty());
}

#[test]
fn api_key_resolves_the_runtime_provider_slot_without_printing_it() {
    let _env_lock = crate::tests::env_lock();
    let _ambient_credentials = without_ambient_credentials();
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let mut store = ConfigStore::load(Some(path)).expect("store");
    store.config.providers.openrouter.api_key = Some("cw-test-handoff-openrouter-5ca1".to_string());
    let secrets = no_keyring_secrets();

    let value = resolve_api_key(
        &store,
        &secrets,
        ProviderKind::Openrouter,
        &CliRuntimeOverrides::default(),
    )
    .expect("resolved API key");
    assert!(
        value == "cw-test-handoff-openrouter-5ca1",
        "resolved an unexpected credential source"
    );
}

#[test]
fn api_key_uses_shared_secret_store_precedence() {
    use std::sync::Arc;

    use codewhale_secrets::{InMemoryKeyringStore, KeyringStore};

    let _env_lock = crate::tests::env_lock();
    let _ambient_credentials = without_ambient_credentials();
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let store = ConfigStore::load(Some(path)).expect("store");
    let inner = Arc::new(InMemoryKeyringStore::new());
    inner
        .set("deepseek", "cw-test-handoff-keyring-e19a")
        .expect("seed keyring");
    let secrets = Secrets::new(inner);

    let value = resolve_api_key(
        &store,
        &secrets,
        ProviderKind::Deepseek,
        &CliRuntimeOverrides::default(),
    )
    .expect("resolved API key");
    assert!(
        value == "cw-test-handoff-keyring-e19a",
        "resolved an unexpected credential source"
    );
}

#[test]
fn api_key_rejects_missing_and_bearer_owned_routes() {
    let _env_lock = crate::tests::env_lock();
    let _ambient_credentials = without_ambient_credentials();
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let mut store = ConfigStore::load(Some(path)).expect("store");
    let secrets = no_keyring_secrets();

    let missing = expect_credential_error(
        resolve_api_key(
            &store,
            &secrets,
            ProviderKind::Openrouter,
            &CliRuntimeOverrides::default(),
        ),
        "missing API key",
    );
    assert!(missing.to_string().contains("no runtime-effective API key"));

    store.config.providers.openai_codex.external_credentials =
        Some(codewhale_config::ExternalCredentialConsentToml::read_only(
            ProviderKind::OpenaiCodex,
            codewhale_config::ExternalCredentialSource::CodexCli,
            crate::openai_codex_auth_file_path(),
        ));
    let codex = expect_credential_error(
        resolve_api_key(
            &store,
            &secrets,
            ProviderKind::OpenaiCodex,
            &CliRuntimeOverrides::default(),
        ),
        "Codex token is not an API key",
    );
    assert!(codex.to_string().contains("bearer credentials"));

    store.config.provider = ProviderKind::Xai;
    store.config.providers.xai.auth_mode = Some("oauth".to_string());
    store.config.providers.xai.oauth_credential_generation =
        Some("xai-auth-0123456789abcdef0123456789abcdef.json".to_string());
    let xai = expect_credential_error(
        resolve_api_key(
            &store,
            &secrets,
            ProviderKind::Xai,
            &CliRuntimeOverrides::default(),
        ),
        "owned OAuth token is not an API key",
    );
    assert!(xai.to_string().contains("OAuth bearer credentials"));

    store.config.providers.xai.oauth_credential_generation = None;
    store.config.providers.xai.api_key = Some("cw-test-xai-key-must-not-win-91a4".to_string());
    store.config.providers.xai.external_credentials =
        Some(codewhale_config::ExternalCredentialConsentToml::read_only(
            ProviderKind::Xai,
            codewhale_config::ExternalCredentialSource::GrokCli,
            crate::grok_auth_file_path(),
        ));
    let external_xai = expect_credential_error(
        resolve_api_key(
            &store,
            &secrets,
            ProviderKind::Xai,
            &CliRuntimeOverrides::default(),
        ),
        "external xAI bearer is not an API key",
    );
    assert!(
        external_xai
            .to_string()
            .contains("OAuth bearer credentials")
    );

    store.config.providers.moonshot.auth_mode = Some("kimi_oauth".to_string());
    let kimi = expect_credential_error(
        resolve_api_key(
            &store,
            &secrets,
            ProviderKind::Moonshot,
            &CliRuntimeOverrides::default(),
        ),
        "imported bearer token is not an API key",
    );
    assert!(kimi.to_string().contains("bearer credentials"));
}

#[test]
fn missing_key_test_is_hermetic_against_ambient_openrouter_credentials() {
    let _env_lock = crate::tests::env_lock();
    let _ambient = crate::tests::ScopedEnvVar::set("OPENROUTER_API_KEY", AMBIENT_SENTINEL);
    let _ambient_credentials = without_ambient_credentials();
    let dir = tempfile::tempdir().expect("tempdir");
    let store = ConfigStore::load(Some(dir.path().join("config.toml"))).expect("store");
    let secrets = no_keyring_secrets();

    let error = expect_credential_error(
        resolve_api_key(
            &store,
            &secrets,
            ProviderKind::Openrouter,
            &CliRuntimeOverrides::default(),
        ),
        "ambient OpenRouter credential isolation",
    );
    let rendered = error.to_string();
    assert!(rendered.contains("no runtime-effective API key"));
    assert!(!rendered.contains(AMBIENT_SENTINEL));
}

#[test]
fn custom_endpoint_keys_stay_endpoint_bound_and_cli_overrides_win() {
    let _env_lock = crate::tests::env_lock();
    let _ambient_credentials = without_ambient_credentials();
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let mut store = ConfigStore::load(Some(path)).expect("store");
    let secrets = no_keyring_secrets();

    store.config.providers.xai.api_key = Some("cw-test-official-xai-key-72be".to_string());
    let custom_cli_route = CliRuntimeOverrides {
        base_url: Some("https://gateway.example.test/v1".to_string()),
        ..CliRuntimeOverrides::default()
    };
    let inherited = expect_credential_error(
        resolve_api_key(&store, &secrets, ProviderKind::Xai, &custom_cli_route),
        "an official route key must not cross onto a CLI custom endpoint",
    );
    assert!(
        inherited
            .to_string()
            .contains("no runtime-effective API key")
    );

    store.config.providers.xai.base_url = Some("https://bound-gateway.example.test/v1".to_string());
    store.config.providers.xai.api_key = Some("cw-test-bound-xai-key-e5f8".to_string());
    let bound = resolve_api_key(
        &store,
        &secrets,
        ProviderKind::Xai,
        &CliRuntimeOverrides::default(),
    )
    .expect("a route-local config key stays bound to its custom endpoint");
    assert!(
        bound == "cw-test-bound-xai-key-e5f8",
        "resolved an unexpected credential source"
    );

    store.config.providers.xai.auth_mode = Some("oauth".to_string());
    store.config.providers.xai.external_credentials =
        Some(codewhale_config::ExternalCredentialConsentToml::read_only(
            ProviderKind::Xai,
            codewhale_config::ExternalCredentialSource::GrokCli,
            crate::grok_auth_file_path(),
        ));
    let explicit = resolve_api_key(
        &store,
        &secrets,
        ProviderKind::Xai,
        &CliRuntimeOverrides {
            api_key: Some("cw-test-explicit-cli-key-d61c".to_string()),
            base_url: Some("https://another-gateway.example.test/v1".to_string()),
            ..CliRuntimeOverrides::default()
        },
    )
    .expect("an explicit CLI key wins for the exact requested endpoint");
    assert!(
        explicit == "cw-test-explicit-cli-key-d61c",
        "resolved an unexpected credential source"
    );

    store.config.providers.moonshot.auth_mode = Some("kimi_oauth".to_string());
    let kimi_override = resolve_api_key(
        &store,
        &secrets,
        ProviderKind::Moonshot,
        &CliRuntimeOverrides {
            api_key: Some("cw-test-explicit-kimi-key-bf42".to_string()),
            ..CliRuntimeOverrides::default()
        },
    )
    .expect("an explicit API key overrides a persisted imported-bearer mode");
    assert!(
        kimi_override == "cw-test-explicit-kimi-key-bf42",
        "resolved an unexpected credential source"
    );
}
