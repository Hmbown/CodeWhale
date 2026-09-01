//! Codewhale account and BYOK credential commands.
//!
//! This module is deliberately separate from the provider-facing `login` and
//! `auth` commands in `lib.rs`: those configure the local runtime, while this
//! surface signs a CLI profile into the managed Codewhale account and stores
//! provider keys in that account's remote vault.

use std::io::{self, IsTerminal, Read, Write};
use std::net::IpAddr;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, Subcommand, ValueEnum};
use codewhale_config::device_code::DevicePollOutcome;
use codewhale_config::{ConfigStore, ProviderKind};
use codewhale_secrets::Secrets;
use codewhale_secrets::account::{
    ACCOUNT_API_BASE_ENV as CLOUD_API_BASE_ENV, AccountAuthBundle as AuthBundle,
    AccountSessionStore, AccountUser as CloudUser, DEFAULT_ACCOUNT_API_BASE as DEFAULT_API_BASE,
    StoredAccountAuth as StoredCloudAuth, normalize_account_profile as normalized_profile,
    secure_account_session_secrets, validate_account_auth_bundle as validate_auth_bundle,
};
use reqwest::Url;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

pub(crate) mod machine;

const MAX_RESPONSE_BYTES: u64 = 256 * 1024;
const MIN_API_KEY_BYTES: usize = 8;
const MAX_API_KEY_BYTES: u64 = 4096;
const MAX_API_KEY_STDIN_BYTES: u64 = MAX_API_KEY_BYTES + 1024;
const MAX_KEY_LABEL_CHARS: usize = 80;
pub(crate) const DEFAULT_LOGIN_TIMEOUT_SECONDS: u64 = 600;
pub(crate) const MAX_LOGIN_TIMEOUT_SECONDS: u64 = 3600;

#[derive(Debug, Args)]
pub(crate) struct CloudArgs {
    /// Codewhale account API origin. HTTPS is required except for loopback HTTP.
    #[arg(long, global = true, value_name = "URL")]
    api_base: Option<String>,
    #[command(subcommand)]
    command: CloudCommand,
}

#[derive(Debug, Subcommand)]
enum CloudCommand {
    /// Sign this CLI profile in through the browser device flow.
    Login(CloudLoginArgs),
    /// Show the signed-in account for this CLI profile.
    Status,
    /// Remove this profile's local account session and revoke it when reachable.
    Logout,
    /// Manage provider API keys stored in the signed-in Codewhale account.
    ///
    /// These are credentials Codewhale presents *to* a model provider. For the
    /// machine tokens a customer presents *to* Codewhale, see `api-keys`.
    Keys(CloudKeysArgs),
    /// Manage Codewhale account API keys: machine tokens for CI.
    #[command(name = "api-keys")]
    ApiKeys(machine::ApiKeysArgs),
    /// Show the account this CLI authenticates as, preferring a machine key.
    Whoami,
    /// Check the account's agent-model precondition for machine work.
    Agent,
    /// Inspect the account document; local settings import is not available yet.
    Pull(CloudPullArgs),
    /// Push local settings to the account document (never automatic, --dry-run required).
    Push(CloudPushArgs),
}

#[derive(Debug, Args)]
struct CloudLoginArgs {
    /// Print the verification URL without trying to open a browser.
    #[arg(long, default_value_t = false)]
    no_open: bool,
    /// Maximum time to wait for browser authorization.
    #[arg(
        long = "timeout-seconds",
        default_value_t = DEFAULT_LOGIN_TIMEOUT_SECONDS,
        value_parser = clap::value_parser!(u64).range(1..=MAX_LOGIN_TIMEOUT_SECONDS)
    )]
    timeout_seconds: u64,
}

#[derive(Debug, Args)]
struct CloudPullArgs {
    /// Inspect the account document without writing local files.
    #[arg(long, default_value_t = false)]
    dry_run: bool,
}

#[derive(Debug, Args)]
struct CloudPushArgs {
    /// Show what would be pushed without writing the remote document.
    #[arg(long, default_value_t = false)]
    dry_run: bool,
}

#[derive(Debug, Args)]
struct CloudKeysArgs {
    #[command(subcommand)]
    command: CloudKeysCommand,
}

#[derive(Debug, Subcommand)]
enum CloudKeysCommand {
    /// List configured providers without revealing key values.
    List,
    /// Save a provider key to the signed-in Codewhale account.
    Set(CloudKeySetArgs),
    /// Remove a provider key from the signed-in Codewhale account.
    Remove { provider: CloudProvider },
}

#[derive(Debug, Args)]
struct CloudKeySetArgs {
    provider: CloudProvider,
    /// Read the key from stdin. Useful for pipes and secret-manager commands.
    #[arg(long = "api-key-stdin", conflicts_with = "from_local")]
    api_key_stdin: bool,
    /// Upload the locally resolved key (config, secret store, then environment).
    #[arg(long, conflicts_with = "api_key_stdin")]
    from_local: bool,
    /// Non-secret label shown beside the stored credential.
    #[arg(long, default_value = "Codewhale CLI")]
    label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum CloudProvider {
    Deepseek,
    Anthropic,
    Openai,
    Openrouter,
    Zai,
    Moonshot,
    Xai,
    #[value(name = "xiaomi", alias = "xiaomi-mimo")]
    Xiaomi,
}

impl CloudProvider {
    const ALL: [Self; 8] = [
        Self::Deepseek,
        Self::Anthropic,
        Self::Openai,
        Self::Openrouter,
        Self::Zai,
        Self::Moonshot,
        Self::Xai,
        Self::Xiaomi,
    ];

    fn slug(self) -> &'static str {
        match self {
            Self::Deepseek => "deepseek",
            Self::Anthropic => "anthropic",
            Self::Openai => "openai",
            Self::Openrouter => "openrouter",
            Self::Zai => "zai",
            Self::Moonshot => "moonshot",
            Self::Xai => "xai",
            Self::Xiaomi => "xiaomi",
        }
    }

    fn local_kind(self) -> ProviderKind {
        match self {
            Self::Deepseek => ProviderKind::Deepseek,
            Self::Anthropic => ProviderKind::Anthropic,
            Self::Openai => ProviderKind::Openai,
            Self::Openrouter => ProviderKind::Openrouter,
            Self::Zai => ProviderKind::Zai,
            Self::Moonshot => ProviderKind::Moonshot,
            Self::Xai => ProviderKind::Xai,
            Self::Xiaomi => ProviderKind::XiaomiMimo,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
}

pub(crate) struct CloudRequest {
    method: HttpMethod,
    path: String,
    bearer: Option<String>,
    body: Option<Vec<u8>>,
}

pub(crate) struct CloudResponse {
    status: u16,
    body: Vec<u8>,
    /// `Retry-After` in whole seconds, when the service supplied one. Kept on
    /// the response rather than re-parsed by callers so the retry policy has a
    /// single source for how long the server asked us to wait.
    retry_after: Option<u64>,
}

pub(crate) trait CloudTransport {
    fn execute(&self, request: CloudRequest) -> Result<CloudResponse>;
}

struct ReqwestTransport {
    base: Url,
    client: reqwest::blocking::Client,
}

impl ReqwestTransport {
    fn new(base: Url) -> Result<Self> {
        let client = reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(8))
            .timeout(Duration::from_secs(30))
            // Never replay bearer tokens or provider-key request bodies to a
            // redirect target. The control-plane origin is an explicit trust
            // boundary, so redirects are treated as ordinary non-2xx replies.
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("codewhale/", env!("CARGO_PKG_VERSION")))
            .build()
            .context("failed to initialize the Codewhale account HTTP client")?;
        Ok(Self { base, client })
    }
}

impl CloudTransport for ReqwestTransport {
    fn execute(&self, request: CloudRequest) -> Result<CloudResponse> {
        let url = self
            .base
            .join(request.path.trim_start_matches('/'))
            .context("failed to construct the Codewhale account request URL")?;
        let method = match request.method {
            HttpMethod::Get => reqwest::Method::GET,
            HttpMethod::Post => reqwest::Method::POST,
            HttpMethod::Put => reqwest::Method::PUT,
            HttpMethod::Delete => reqwest::Method::DELETE,
        };
        let mut builder = self
            .client
            .request(method, url)
            .header(reqwest::header::ACCEPT, "application/json");
        if let Some(token) = request.bearer {
            builder = builder.bearer_auth(token);
        }
        if let Some(body) = request.body {
            builder = builder
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body);
        }
        let response = builder
            .send()
            .context("could not reach the Codewhale service")?;
        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse::<u64>().ok());
        let mut body = Vec::new();
        response
            .take(MAX_RESPONSE_BYTES + 1)
            .read_to_end(&mut body)
            .context("failed to read the Codewhale service response")?;
        if body.len() as u64 > MAX_RESPONSE_BYTES {
            bail!("The Codewhale service returned an unexpectedly large response");
        }
        Ok(CloudResponse {
            status,
            body,
            retry_after,
        })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeviceStart {
    device_code: String,
    user_code: String,
    verification_uri: String,
    verification_uri_complete: String,
    expires_in: u64,
    interval: u64,
}

#[derive(Deserialize)]
struct MeResponse {
    user: CloudUser,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DeviceTokenRequest<'a> {
    device_code: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RefreshRequest<'a> {
    refresh_token: &'a str,
}

#[derive(Serialize)]
struct ModelKeyRequest<'a> {
    key: &'a str,
    label: &'a str,
}

pub(crate) struct CloudClient<'a, T: CloudTransport> {
    transport: &'a T,
    account_store: AccountSessionStore,
}

impl<'a, T: CloudTransport> CloudClient<'a, T> {
    fn new(transport: &'a T, secrets: &'a Secrets, profile: &str, api_base: &'a str) -> Self {
        Self {
            transport,
            account_store: AccountSessionStore::new(secrets.clone(), Some(profile), api_base),
        }
    }

    fn start_device(&self) -> Result<DeviceStart> {
        let response = self.transport.execute(CloudRequest {
            method: HttpMethod::Post,
            path: "/api/cli/device/start".to_string(),
            bearer: None,
            body: Some(b"{}".to_vec()),
        })?;
        expect_json(response, &[200])
    }

    fn poll_device(
        &self,
        device: &DeviceStart,
        timeout: Duration,
        sleep: &mut dyn FnMut(Duration),
    ) -> Result<AuthBundle> {
        validate_device_code(&device.device_code)?;
        let server_lifetime =
            Duration::from_secs(device.expires_in.clamp(1, MAX_LOGIN_TIMEOUT_SECONDS));
        // The Codewhale account service answers HTTP 202 while the code is
        // still pending, so the first response is already meaningful: poll
        // immediately and sleep afterwards. It has no slow_down.
        let bundle = codewhale_config::device_code::DeviceCodePoll::new(
            timeout.min(server_lifetime),
            "Codewhale account login timed out; run `codewhale login` to try again",
        )
        .interval_seconds(Some(device.interval))
        .max_interval_seconds(10)
        .run(sleep, || {
            let response = self.transport.execute(CloudRequest {
                method: HttpMethod::Post,
                path: "/api/cli/device/token".to_string(),
                bearer: None,
                body: Some(json_body(&DeviceTokenRequest {
                    device_code: &device.device_code,
                })?),
            })?;
            match response.status {
                200 => {
                    let bundle: AuthBundle = parse_json_body(&response.body)?;
                    validate_auth_bundle(&bundle)?;
                    Ok(DevicePollOutcome::Complete(bundle))
                }
                202 => Ok(DevicePollOutcome::Pending),
                _ => Err(response_error(&response)),
            }
        })?;
        self.save_auth(bundle.clone())?;
        Ok(bundle)
    }

    fn load_auth(&self) -> Result<Option<StoredCloudAuth>> {
        self.account_store.load().context(
            "the local Codewhale account session is unreadable; run `codewhale account logout` and sign in again",
        )
    }

    fn save_auth(&self, bundle: AuthBundle) -> Result<()> {
        self.account_store
            .save(bundle)
            .context("failed to save the Codewhale account session in the local secret store")
    }

    fn clear_auth(&self) -> Result<()> {
        self.account_store
            .clear()
            .context("failed to remove the local Codewhale account session")
    }

    fn me(&self) -> Result<CloudUser> {
        let response = self.execute_authenticated(HttpMethod::Get, "/api/me", None)?;
        let me: MeResponse = expect_json(response, &[200])?;
        if me.user.id.trim().is_empty() {
            bail!("The Codewhale service returned an account without an ID");
        }
        if let Some(mut stored) = self.load_auth()? {
            stored.bundle.user = Some(me.user.clone());
            self.save_auth(stored.bundle)?;
        }
        Ok(me.user)
    }

    fn set_key(&self, provider: CloudProvider, key: &str, label: &str) -> Result<()> {
        let path = format!("/api/model-keys/{}", provider.slug());
        let response = self.execute_authenticated(
            HttpMethod::Put,
            &path,
            Some(json_body(&ModelKeyRequest { key, label })?),
        )?;
        expect_empty(response, &[200, 201])
    }

    fn remove_key(&self, provider: CloudProvider) -> Result<()> {
        let path = format!("/api/model-keys/{}", provider.slug());
        let response = self.execute_authenticated(HttpMethod::Delete, &path, None)?;
        expect_empty(response, &[200, 204])
    }

    fn logout(&self) -> Result<bool> {
        let stored = match self.load_auth() {
            Ok(Some(stored)) => stored,
            Ok(None) => {
                // `load` deliberately treats obsolete-schema and wrong-origin
                // records as signed out. Logout must still scrub their slot.
                self.clear_auth()?;
                return Ok(false);
            }
            Err(_) => {
                // Logout is also the recovery path for a corrupt or obsolete
                // local record, so it must remain able to remove that record.
                self.clear_auth()?;
                return Ok(false);
            }
        };
        let body = json_body(&RefreshRequest {
            refresh_token: &stored.bundle.refresh_token,
        })?;
        let remote_revoked = self
            .transport
            .execute(CloudRequest {
                method: HttpMethod::Post,
                path: "/api/auth/logout".to_string(),
                bearer: None,
                body: Some(body),
            })
            .is_ok_and(|response| (200..300).contains(&response.status));
        self.clear_auth()?;
        Ok(remote_revoked)
    }

    fn execute_authenticated(
        &self,
        method: HttpMethod,
        path: &str,
        body: Option<Vec<u8>>,
    ) -> Result<CloudResponse> {
        let Some(mut stored) = self.load_auth()? else {
            bail!("Not signed in. Run `codewhale login` first");
        };
        let first = self.transport.execute(CloudRequest {
            method,
            path: path.to_string(),
            bearer: Some(stored.bundle.access_token.clone()),
            body: body.clone(),
        })?;
        if first.status != 401 {
            return Ok(first);
        }

        let refresh = self.transport.execute(CloudRequest {
            method: HttpMethod::Post,
            path: "/api/auth/refresh".to_string(),
            bearer: None,
            body: Some(json_body(&RefreshRequest {
                refresh_token: &stored.bundle.refresh_token,
            })?),
        })?;
        match refresh.status {
            200 => {}
            401 => {
                self.clear_auth()?;
                bail!("The Codewhale account session expired. Run `codewhale login` again");
            }
            _ => return Err(response_error(&refresh)),
        }
        let mut next: AuthBundle = parse_json_body(&refresh.body)?;
        validate_auth_bundle(&next)?;
        if next.user.is_none() {
            next.user = stored.bundle.user.take();
        }
        self.save_auth(next.clone())?;

        let retried = self.transport.execute(CloudRequest {
            method,
            path: path.to_string(),
            bearer: Some(next.access_token),
            body,
        })?;
        if retried.status == 401 {
            self.clear_auth()?;
            bail!("The Codewhale account session expired. Run `codewhale login` again");
        }
        Ok(retried)
    }

    /// Whether an interactive session exists for this profile and origin.
    ///
    /// A management command asks this before it asks anything of the network,
    /// so "you have a machine key but no login" is answered locally instead of
    /// as a 403 from a route the key was never allowed to touch.
    fn has_session(&self) -> Result<bool> {
        Ok(self.load_auth()?.is_some())
    }

    /// `execute_authenticated`, retrying only what the caller marks replayable.
    ///
    /// `machine::Retry::Never` is not a default worth having: the one POST in
    /// this surface mints a secret shown exactly once, so a retry that quietly
    /// succeeded server-side would leave an unrevocable key behind.
    fn execute_authenticated_with_retry(
        &self,
        method: HttpMethod,
        path: &str,
        body: Option<Vec<u8>>,
        retry: machine::Retry,
        sleeper: &mut dyn FnMut(Duration),
    ) -> Result<CloudResponse> {
        let max_attempts = if retry == machine::Retry::Idempotent {
            3
        } else {
            1
        };
        let mut attempt = 1;
        loop {
            let response = self.execute_authenticated(method, path, body.clone())?;
            if (200..300).contains(&response.status) || attempt >= max_attempts {
                return Ok(response);
            }
            let retry_after = response.retry_after;
            if !machine::classify(&response).retryable {
                return Ok(response);
            }
            sleeper(machine::backoff_delay(attempt, retry_after));
            attempt += 1;
        }
    }
}

enum KeyReadMode {
    Stdin,
    HiddenPrompt(String),
}

pub(crate) fn run(args: CloudArgs, profile: Option<&str>, config: &ConfigStore) -> Result<()> {
    let machine = machine::MachineKeyEnv::from_process_env();
    let requested_base = machine::resolve_api_base(
        args.api_base.as_deref(),
        std::env::var(machine::MACHINE_API_BASE_ENV).ok().as_deref(),
        std::env::var(CLOUD_API_BASE_ENV).ok().as_deref(),
        DEFAULT_API_BASE,
    );
    if machine.is_present() {
        // A machine token is a bearer credential with no replay protection.
        // Refuse cleartext to a remote host before a transport exists, so
        // there is no code path on which the key could be written to a socket.
        machine::require_secure_base(&requested_base)?;
    }
    let api_base = validate_api_base(&requested_base)?;
    let transport = ReqwestTransport::new(api_base.url.clone())?;
    // Account refresh tokens require an OS credential manager. The ordinary
    // provider backend remains independently configurable for `--from-local`.
    let cloud_secrets = cloud_session_secrets()?;
    let provider_secrets = Secrets::auto_detect();
    let profile = normalized_profile(profile);
    let mut stdout = io::stdout().lock();
    let mut key_reader = |mode: KeyReadMode| match mode {
        KeyReadMode::Stdin => read_key_from_stdin(),
        KeyReadMode::HiddenPrompt(provider) => read_key_hidden(&provider),
    };
    let mut opener = |url: String| webbrowser::open(&url).is_ok();
    let mut sleeper = |duration| thread::sleep(duration);
    run_with(
        args.command,
        &profile,
        &api_base.display,
        config,
        &cloud_secrets,
        &provider_secrets,
        &machine,
        &transport,
        &mut stdout,
        &mut key_reader,
        &mut opener,
        &mut sleeper,
    )
}

fn cloud_session_secrets() -> Result<Secrets> {
    // Codex-style storage contract: the OS credential manager is preferred
    // but never required; without one, sessions live in the private 0600
    // Codewhale secrets file. Only an unresolvable store path fails here.
    secure_account_session_secrets().map_err(|err| anyhow!(err.to_string()))
}

/// `codewhale login` is a convenience entry to the account device flow — the
/// same path as `codewhale account login`, without re-spelling the subcommand.
pub(crate) fn run_account_login(
    no_open: bool,
    timeout_seconds: u64,
    profile: Option<&str>,
    config: &ConfigStore,
) -> Result<()> {
    run(
        CloudArgs {
            api_base: None,
            command: CloudCommand::Login(CloudLoginArgs {
                no_open,
                timeout_seconds,
            }),
        },
        profile,
        config,
    )
}

pub(crate) fn reject_inline_api_key(api_key: Option<&str>) -> Result<()> {
    if api_key.is_some() {
        bail!(
            "`codewhale account` does not accept the global `--api-key` flag because command-line values can leak through shell history. Use `account keys set <provider>` for a hidden prompt, `--api-key-stdin`, or `--from-local`"
        );
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run_with<T: CloudTransport, W: Write>(
    command: CloudCommand,
    profile: &str,
    api_base: &str,
    config: &ConfigStore,
    cloud_secrets: &Secrets,
    provider_secrets: &Secrets,
    machine: &machine::MachineKeyEnv,
    transport: &T,
    out: &mut W,
    key_reader: &mut dyn FnMut(KeyReadMode) -> Result<String>,
    opener: &mut dyn FnMut(String) -> bool,
    sleeper: &mut dyn FnMut(Duration),
) -> Result<()> {
    let client = CloudClient::new(transport, cloud_secrets, profile, api_base);
    match command {
        CloudCommand::Login(login) => {
            let device = client.start_device()?;
            validate_user_code(&device.user_code)?;
            let verification_uri = validate_verification_url(
                &device.verification_uri,
                api_base,
                &device.user_code,
                false,
            )?;
            let verification_uri_complete = validate_verification_url(
                &device.verification_uri_complete,
                api_base,
                &device.user_code,
                true,
            )?;
            writeln!(out, "Codewhale account sign-in")?;
            writeln!(out, "Code: {}", device.user_code)?;
            writeln!(out, "Open: {verification_uri}")?;
            writeln!(out, "Profile: {}", printable(profile))?;
            if !login.no_open && !opener(verification_uri_complete) {
                writeln!(
                    out,
                    "Browser could not be opened; use the URL and code above."
                )?;
            }
            let _ =
                client.poll_device(&device, Duration::from_secs(login.timeout_seconds), sleeper)?;
            let user = client.me()?;
            write_account(out, "Signed in to Codewhale.", profile, api_base, &user)?;
            Ok(())
        }
        CloudCommand::Status => match client.load_auth()? {
            Some(_) => {
                let user = client.me()?;
                write_account(out, "Signed in to Codewhale.", profile, api_base, &user)
            }
            None => {
                writeln!(out, "Not signed in to Codewhale.")?;
                writeln!(out, "Profile: {}", printable(profile))?;
                writeln!(out, "API: {api_base}")?;
                writeln!(out, "Run `codewhale login` to sign in.")?;
                Ok(())
            }
        },
        CloudCommand::Logout => {
            let remote_revoked = client.logout()?;
            writeln!(out, "Removed the local Codewhale account session.")?;
            writeln!(out, "Profile: {}", printable(profile))?;
            if !remote_revoked {
                writeln!(
                    out,
                    "Remote revocation was not confirmed; the local tokens are gone."
                )?;
            }
            Ok(())
        }
        CloudCommand::Keys(keys) => match keys.command {
            CloudKeysCommand::List => {
                let user = client.me()?;
                write_account(out, "Codewhale account keys.", profile, api_base, &user)?;
                for provider in CloudProvider::ALL {
                    let state = user.model_keys.get(provider.slug());
                    if state.is_some_and(|state| state.configured) {
                        writeln!(out, "{}: set", provider.slug())?;
                    } else {
                        writeln!(out, "{}: not set", provider.slug())?;
                    }
                }
                Ok(())
            }
            CloudKeysCommand::Set(set) => {
                let user = client.me()?;
                let key = if set.from_local {
                    resolve_local_key(config, provider_secrets, set.provider)?.ok_or_else(|| {
                        anyhow!(
                            "No local {} API key was found in config, the secret store, or the environment",
                            set.provider.slug()
                        )
                    })?
                } else if set.api_key_stdin {
                    key_reader(KeyReadMode::Stdin)?
                } else {
                    key_reader(KeyReadMode::HiddenPrompt(set.provider.slug().to_string()))?
                };
                let key = key.trim().to_string();
                validate_api_key(&key)?;
                let label = validate_label(&set.label)?;
                client.set_key(set.provider, &key, &label)?;
                writeln!(
                    out,
                    "Saved {} for Codewhale account {} (profile {}).",
                    set.provider.slug(),
                    printable(&user.id),
                    printable(profile)
                )?;
                Ok(())
            }
            CloudKeysCommand::Remove { provider } => {
                let user = client.me()?;
                client.remove_key(provider)?;
                writeln!(
                    out,
                    "Removed {} from Codewhale account {} (profile {}).",
                    provider.slug(),
                    printable(&user.id),
                    printable(profile)
                )?;
                Ok(())
            }
        },
        CloudCommand::ApiKeys(api_keys) => {
            machine::run_api_keys(api_keys, &client, machine, out, sleeper)
        }
        CloudCommand::Whoami => match machine.resolve()? {
            // A present machine key wins and never falls back: silently
            // downgrading a machine credential to a human one is how CI ends
            // up running as the wrong identity.
            Some(key) => {
                let machine_client = machine::MachineClient::new(transport, key);
                let who = machine_client.whoami(sleeper)?;
                machine::write_whoami(out, &who, api_base, machine_client.key_head())
            }
            None => {
                let user = client.me()?;
                write_account(out, "Signed in to Codewhale.", profile, api_base, &user)
            }
        },
        CloudCommand::Agent => {
            // The agent route is machine-key-only by design, so CI and humans
            // never blur in an audit trail. There is no session fallback.
            let key = machine.require()?;
            let machine_client = machine::MachineClient::new(transport, key);
            let agent = machine_client.agent(sleeper)?.agent;
            machine::write_agent(out, &agent)
        }
        CloudCommand::Pull(args) => {
            if !args.dry_run {
                bail!(
                    "Account settings import is not available yet; local config was not changed. Run `codewhale account pull --dry-run` to inspect the signed-in account."
                );
            }
            let user = client.me()?;
            // `/api/me` currently exposes account identity and key metadata,
            // not a versioned settings document that can be applied locally.
            // Stay read-only and explicit until that import contract exists.
            writeln!(out, "Account settings (pull --dry-run):")?;
            writeln!(out, "Account ID: {}", printable(&user.id))?;
            writeln!(out, "Profile: {}", printable(profile))?;
            writeln!(out, "API: {api_base}")?;
            writeln!(
                out,
                "dry-run: remote settings import is not available; local config unchanged"
            )?;
            // Show the invariant: Bearer custody stays in the OS keyring, never in config.toml.
            writeln!(
                out,
                "Secure custody: Bearer tokens remain in the OS keyring"
            )?;
            Ok(())
        }
        CloudCommand::Push(args) => {
            let user = client.me()?;
            if !args.dry_run {
                bail!(
                    "Push is never automatic; re-run with --dry-run to preview, then confirm explicitly"
                );
            }
            writeln!(out, "Account settings (push --dry-run):")?;
            writeln!(out, "Account ID: {}", printable(&user.id))?;
            writeln!(out, "Profile: {}", printable(profile))?;
            writeln!(out, "API: {api_base}")?;
            writeln!(
                out,
                "dry-run: would PATCH /api/me/preferences with If-Match revision check (412 on conflict)"
            )?;
            writeln!(
                out,
                "No credentials, paths, or env are copied; only explicit fields (field-level last-writer-wins)"
            )?;
            Ok(())
        }
    }
}

/// Resolve the account's configured agent route for a machine-key run.
///
/// `codewhale review` hard-errors when a model resolves to several configured
/// routes. When CI authenticates with a machine key, the account has already
/// answered that question, so its configured provider is the disambiguator —
/// no new flag, and no guess. Returns `None` when no machine key is set, which
/// leaves the ordinary local resolution untouched.
pub(crate) fn machine_review_provider() -> Result<Option<ProviderKind>> {
    let machine = machine::MachineKeyEnv::from_process_env();
    let Some(key) = machine.resolve()? else {
        return Ok(None);
    };
    let requested_base = machine::resolve_api_base(
        None,
        std::env::var(machine::MACHINE_API_BASE_ENV).ok().as_deref(),
        std::env::var(CLOUD_API_BASE_ENV).ok().as_deref(),
        DEFAULT_API_BASE,
    );
    machine::require_secure_base(&requested_base)?;
    let api_base = validate_api_base(&requested_base)?;
    let transport = ReqwestTransport::new(api_base.url)?;
    let client = machine::MachineClient::new(&transport, key);
    // The call that actually needs a model is the call that refuses without
    // one, so this precondition runs before any review work starts.
    let agent = client.agent(&mut |duration| thread::sleep(duration))?.agent;
    machine::review_provider_from_agent(&agent).map(Some)
}

fn write_account<W: Write>(
    out: &mut W,
    heading: &str,
    profile: &str,
    api_base: &str,
    user: &CloudUser,
) -> Result<()> {
    writeln!(out, "{heading}")?;
    writeln!(out, "Account ID: {}", printable(&user.id))?;
    if !user.display_name.trim().is_empty() {
        writeln!(out, "Name: {}", printable(&user.display_name))?;
    }
    if !user.email.trim().is_empty() {
        writeln!(out, "Email: {}", printable(&user.email))?;
    }
    if !user.plan.trim().is_empty() {
        writeln!(out, "Plan: {}", printable(&user.plan))?;
    }
    writeln!(out, "Profile: {}", printable(profile))?;
    writeln!(out, "API: {api_base}")?;
    Ok(())
}

struct ValidatedApiBase {
    url: Url,
    display: String,
}

fn validate_api_base(value: &str) -> Result<ValidatedApiBase> {
    let mut url = Url::parse(value.trim()).context("invalid Codewhale account API base URL")?;
    if !url.username().is_empty() || url.password().is_some() {
        bail!("Codewhale account API base URL must not contain credentials");
    }
    if url.query().is_some() || url.fragment().is_some() {
        bail!("Codewhale account API base URL must not contain a query or fragment");
    }
    if !matches!(url.path(), "" | "/") {
        bail!("Codewhale account API base URL must be an origin without a path");
    }
    let host = url
        .host_str()
        .ok_or_else(|| anyhow!("Codewhale account API base URL must include a host"))?;
    let allowed = url.scheme() == "https" || (url.scheme() == "http" && is_loopback_host(host));
    if !allowed {
        bail!(
            "Codewhale account API base URL must use HTTPS (loopback HTTP is allowed for testing)"
        );
    }
    url.set_path("/");
    let display = url.as_str().trim_end_matches('/').to_string();
    Ok(ValidatedApiBase { url, display })
}

fn validate_verification_url(
    value: &str,
    api_base: &str,
    user_code: &str,
    complete: bool,
) -> Result<String> {
    let url =
        Url::parse(value).context("The Codewhale service returned an invalid verification URL")?;
    if value != url.as_str() {
        bail!("The Codewhale service returned an unsafe verification URL");
    }
    let host = url.host_str().ok_or_else(|| {
        anyhow!("The Codewhale service returned a verification URL without a host")
    })?;
    if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
        bail!("The Codewhale service returned an unsafe verification URL");
    }
    if url.path() != "/cli/authorize" {
        bail!("The Codewhale service returned an unsafe verification URL");
    }

    let api = Url::parse(api_base).context("invalid Codewhale account API base URL")?;
    let canonical_api = api.scheme() == "https"
        && api.host_str() == Some("api.codewhale.net")
        && api.port_or_known_default() == Some(443);
    let loopback_api = api.host_str().is_some_and(is_loopback_host);
    if canonical_api {
        if url.scheme() != "https"
            || !host.eq_ignore_ascii_case("app.codewhale.net")
            || url.port_or_known_default() != Some(443)
        {
            bail!("The Codewhale service returned an untrusted verification origin");
        }
    } else if loopback_api {
        if !matches!(url.scheme(), "http" | "https") || !is_loopback_host(host) {
            bail!("The Codewhale service returned an untrusted verification origin");
        }
    } else {
        bail!(
            "Browser login is only enabled for the canonical Codewhale account API or a loopback test API"
        );
    }

    let query = url.query_pairs().collect::<Vec<_>>();
    if complete {
        if query.len() != 1 || query[0].0 != "user_code" || query[0].1 != user_code {
            bail!("The Codewhale service returned an unsafe verification URL");
        }
    } else if !query.is_empty() {
        bail!("The Codewhale service returned an unsafe verification URL");
    }
    Ok(url.to_string())
}

fn is_loopback_host(host: &str) -> bool {
    let host = host
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(host);
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

fn validate_user_code(code: &str) -> Result<()> {
    const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let bytes = code.as_bytes();
    if bytes.len() != 14
        || bytes[4] != b'-'
        || bytes[9] != b'-'
        || bytes
            .iter()
            .enumerate()
            .any(|(index, byte)| !matches!(index, 4 | 9) && !ALPHABET.contains(byte))
    {
        bail!("The Codewhale service returned an invalid user code");
    }
    Ok(())
}

fn validate_device_code(code: &str) -> Result<()> {
    if code.len() != 43
        || !code
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        bail!("The Codewhale service returned an invalid device authorization response");
    }
    Ok(())
}

fn validate_api_key(key: &str) -> Result<()> {
    let bytes = key.len();
    if bytes < MIN_API_KEY_BYTES || bytes as u64 > MAX_API_KEY_BYTES {
        bail!("API key must be {MIN_API_KEY_BYTES}-{MAX_API_KEY_BYTES} UTF-8 bytes");
    }
    if key.chars().any(is_ascii_control) {
        bail!("API key contains invalid control characters");
    }
    Ok(())
}

fn validate_label(label: &str) -> Result<String> {
    let label = label.split_whitespace().collect::<Vec<_>>().join(" ");
    if label.is_empty()
        || label.chars().count() > MAX_KEY_LABEL_CHARS
        || label.chars().any(is_ascii_control)
    {
        bail!("key label must contain 1-{MAX_KEY_LABEL_CHARS} characters");
    }
    Ok(label)
}

fn is_ascii_control(character: char) -> bool {
    character <= '\u{001f}' || character == '\u{007f}'
}

fn resolve_local_key(
    config: &ConfigStore,
    secrets: &Secrets,
    provider: CloudProvider,
) -> Result<Option<String>> {
    let kind = provider.local_kind();
    let provider_config = config.config.providers.for_provider(kind);
    let from_config = provider_config.api_key.clone().or_else(|| {
        (kind == ProviderKind::Deepseek)
            .then(|| config.config.api_key.clone())
            .flatten()
    });
    if let Some(value) = from_config
        .and_then(resolve_config_key_reference)
        .filter(|value| !value.trim().is_empty())
    {
        return Ok(Some(value));
    }
    if let Some(value) = secrets
        .get(kind.as_str())
        .context("failed to read the local provider secret store")?
        .filter(|value| !value.trim().is_empty())
    {
        return Ok(Some(value));
    }
    Ok(kind.provider().env_vars().iter().find_map(|name| {
        std::env::var(name)
            .ok()
            .filter(|value| !value.trim().is_empty())
    }))
}

fn resolve_config_key_reference(value: String) -> Option<String> {
    let trimmed = value.trim();
    let Some(variable) = trimmed.strip_prefix('$') else {
        return Some(value);
    };
    if variable.is_empty()
        || !variable
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return None;
    }
    std::env::var(variable)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

fn read_key_from_stdin() -> Result<String> {
    let mut bytes = Vec::new();
    io::stdin()
        .take(MAX_API_KEY_STDIN_BYTES + 1)
        .read_to_end(&mut bytes)
        .context("failed to read API key from stdin")?;
    parse_key_input(bytes)
}

fn parse_key_input(bytes: Vec<u8>) -> Result<String> {
    if bytes.len() as u64 > MAX_API_KEY_STDIN_BYTES {
        bail!("API key input is unexpectedly large");
    }
    let value = String::from_utf8(bytes).context("API key from stdin is not valid UTF-8")?;
    let value = value.trim().to_string();
    validate_api_key(&value)?;
    Ok(value)
}

fn read_key_hidden(provider: &str) -> Result<String> {
    if !io::stdin().is_terminal() {
        bail!("interactive key entry requires a terminal; use `--api-key-stdin` for piped input");
    }
    let term = console::Term::stderr();
    term.write_str(&format!("Enter {provider} API key: "))
        .context("failed to write API key prompt")?;
    let value = term
        .read_secure_line()
        .context("failed to read API key securely")?;
    term.write_line("").ok();
    let value = value.trim().to_string();
    validate_api_key(&value)?;
    Ok(value)
}

fn json_body(value: &impl Serialize) -> Result<Vec<u8>> {
    serde_json::to_vec(value).context("failed to encode Codewhale account request")
}

fn expect_json<T: DeserializeOwned>(response: CloudResponse, statuses: &[u16]) -> Result<T> {
    if !statuses.contains(&response.status) {
        return Err(response_error(&response));
    }
    parse_json_body(&response.body)
}

fn expect_empty(response: CloudResponse, statuses: &[u16]) -> Result<()> {
    if statuses.contains(&response.status) {
        Ok(())
    } else {
        Err(response_error(&response))
    }
}

fn parse_json_body<T: DeserializeOwned>(body: &[u8]) -> Result<T> {
    serde_json::from_slice(body).context("The Codewhale service returned an invalid JSON response")
}

fn response_error(response: &CloudResponse) -> anyhow::Error {
    let code = serde_json::from_slice::<serde_json::Value>(&response.body)
        .ok()
        .and_then(|body| {
            body.get("code")
                .and_then(serde_json::Value::as_str)
                .or_else(|| {
                    body.get("error")
                        .and_then(|error| error.get("code"))
                        .and_then(serde_json::Value::as_str)
                })
                .and_then(safe_error_code)
        });
    match code {
        Some(code) => anyhow!(
            "Codewhale account request failed (HTTP {}, code {code})",
            response.status
        ),
        None => anyhow!(
            "Codewhale account request failed (HTTP {})",
            response.status
        ),
    }
}

fn safe_error_code(code: &str) -> Option<String> {
    if code.is_empty()
        || code.len() > 80
        || !code
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        return None;
    }
    Some(code.to_string())
}

fn printable(value: &str) -> String {
    value
        .chars()
        .filter(|character| !character.is_control())
        .take(200)
        .collect::<String>()
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests;
