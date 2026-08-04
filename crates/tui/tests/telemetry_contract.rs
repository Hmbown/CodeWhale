//! Process-level telemetry contract.
//!
//! Everything here drives the real `codewhale-tui` binary inside a sealed
//! `HOME`/`CODEWHALE_HOME`, with a loopback recorder standing in for the
//! telemetry endpoint. The unit tests in `codewhale-telemetry` prove the
//! predicate; these prove that the *emitting process* consults it — which is
//! the thing v1 of this design got wrong, because `resolve_runtime_options`
//! had no non-test caller and so neither `telemetry = false` in the config file
//! nor `CODEWHALE_TELEMETRY=0` was ever read by a process that would have sent.
//!
//! Two disciplines make the zero-request assertions non-vacuous:
//!
//! 1. `enabled_and_accepted_posts_exactly_one_batch` proves the client works.
//!    Without it every "sends zero requests" test would also pass against a
//!    client that never sends anything at all.
//! 2. Every off-test records the notice decision *and* points at a live
//!    recorder, so it cannot pass through the consent gate by accident.
//!
//! The recorder is `http://127.0.0.1:<port>` — loopback, which is the one place
//! `validate_endpoint` permits plaintext, and a packet that never leaves the
//! machine.

#![cfg(unix)]

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use codewhale_config::{SetupState, TELEMETRY_NOTICE_VERSION};
use serde_json::{Value, json};
use tempfile::TempDir;
use wait_timeout::ChildExt;
use wiremock::matchers::{method, path as path_matcher};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Where the recorder listens for batches.
const TELEMETRY_PATH: &str = "/v1/telemetry";
/// Where the mock model listens.
const MODEL_PATH: &str = "/v1/chat/completions";
const TEST_MODEL: &str = "telemetry-contract-model";

/// Sentinels planted through real inputs. None of these may appear in a batch.
///
/// Deliberately low-entropy: `crates/tui/src/fleet/ledger.rs` notes that
/// realistic-looking tokens trip secret scanning at push time.
const SENTINEL_PROMPT: &str = "tc-prompt-sentinel-do-not-collect";
const SENTINEL_FILENAME: &str = "tc-workspace-sentinel-file.txt";
const SENTINEL_PROVIDER_TABLE: &str = "tc_custom_provider_sentinel";
const SENTINEL_MCP_SERVER: &str = "tc-mcp-server-sentinel";
const SENTINEL_API_KEY: &str = "tc-api-key-sentinel-not-a-real-key";
/// The key lives only in the child's environment, never in a file, so the
/// "absent from every written file" assertion means something.
const SENTINEL_API_KEY_ENV: &str = "TC_SENTINEL_API_KEY";

const EXEC_TIMEOUT: Duration = Duration::from_secs(90);

// ── Fixture ──────────────────────────────────────────────────────────────

struct Fixture {
    _root: TempDir,
    home: PathBuf,
    codewhale_home: PathBuf,
    workspace: PathBuf,
    config_path: PathBuf,
    endpoint: Option<String>,
}

impl Fixture {
    fn new() -> Self {
        let root = TempDir::new().expect("fixture root");
        let home = root.path().join("home");
        let codewhale_home = root.path().join("codewhale-home");
        let workspace = root.path().join("workspace");
        for dir in [&home, &codewhale_home, &workspace] {
            std::fs::create_dir_all(dir).expect("create fixture dir");
        }
        let config_path = root.path().join("config.toml");
        std::fs::write(&config_path, "").expect("write config");
        Self {
            _root: root,
            home,
            codewhale_home,
            workspace,
            config_path,
            endpoint: None,
        }
    }

    /// Point this fixture at a loopback recorder.
    fn with_endpoint(mut self, base_url: &str) -> Self {
        self.endpoint = Some(format!("{base_url}{TELEMETRY_PATH}"));
        self
    }

    fn write_config(&self, body: &str) {
        std::fs::write(&self.config_path, body).expect("write config");
    }

    /// Record the answer a user would have given on a TTY.
    ///
    /// Machine-scoped consent: the notice is only ever *rendered* on a
    /// terminal, but the decision it records lives on this `CODEWHALE_HOME` and
    /// authorizes later non-TTY runs against the same home.
    fn record_notice(&self, opt_in: bool) {
        let mut state = SetupState::default();
        state.record_telemetry_notice(TELEMETRY_NOTICE_VERSION, opt_in);
        state
            .save_to(&self.codewhale_home.join("setup_state.json"))
            .expect("write setup state");
    }

    fn setup_state_path(&self) -> PathBuf {
        self.codewhale_home.join("setup_state.json")
    }

    fn telemetry_root(&self) -> PathBuf {
        self.codewhale_home.join("telemetry")
    }

    fn command(&self) -> Command {
        let mut command = Command::new(codewhale_tui_binary());
        command
            .current_dir(&self.workspace)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").expect("PATH"))
            .env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env("XDG_DATA_HOME", self.home.join(".local").join("share"))
            .env("XDG_CACHE_HOME", self.home.join(".cache"))
            .env("CODEWHALE_HOME", &self.codewhale_home)
            .env("CODEWHALE_SECRET_BACKEND", "file")
            .env("CODEWHALE_MEMORY", "false")
            // A pinned mirror version keeps the release crate from issuing a
            // metadata request, so the only egress a test can observe is the
            // one this file is about.
            .env(
                "CODEWHALE_RELEASE_BASE_URL",
                "https://example.invalid/releases",
            )
            .env("DEEPSEEK_TUI_VERSION", env!("CARGO_PKG_VERSION"))
            .env("RUST_LOG", "warn")
            .stdin(Stdio::null());
        if let Some(endpoint) = &self.endpoint {
            command.env("CODEWHALE_TELEMETRY_ENDPOINT", endpoint);
        }
        command
    }

    /// The cheapest subcommand that still traverses the whole telemetry
    /// lifecycle: arm, `session_start`, dispatch, `session_end`, shutdown
    /// flush. `completions` loads no config of its own and touches no network,
    /// so anything the recorder sees came from telemetry.
    fn run_completions(&self) -> Output {
        let mut command = self.command();
        command.args([
            "--config",
            self.config_path.to_str().expect("config path"),
            "completions",
            "bash",
        ]);
        let output = command.output().expect("run codewhale-tui completions");
        assert!(
            output.status.success(),
            "completions failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    /// Every regular file under the sealed roots, for leak scanning.
    fn written_files(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for base in [&self.home, &self.codewhale_home, &self.workspace] {
            collect_files(base, &mut out);
        }
        out.push(self.config_path.clone());
        out
    }
}

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => collect_files(&path, out),
            Ok(kind) if kind.is_file() => out.push(path),
            _ => {}
        }
    }
}

fn codewhale_tui_binary() -> PathBuf {
    if let Some(path) = option_env!("CARGO_BIN_EXE_codewhale-tui") {
        return PathBuf::from(path);
    }
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_codewhale-tui") {
        return PathBuf::from(path);
    }
    let mut path = std::env::current_exe().expect("current test executable path");
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.push(format!("codewhale-tui{}", std::env::consts::EXE_SUFFIX));
    path
}

// ── Recorder ─────────────────────────────────────────────────────────────

/// A loopback endpoint that accepts every batch and keeps the body.
///
/// The body is the point. The recorder `crates/tui/tests/diagnostic_read_only.rs`
/// copies deliberately drops it; a telemetry contract that cannot read what was
/// sent can only assert "something happened".
async fn start_recorder() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path_matcher(TELEMETRY_PATH))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    server
}

/// Every request the recorder saw, batch bodies included.
async fn recorded_batches(server: &MockServer) -> Vec<Value> {
    let requests = server
        .received_requests()
        .await
        .expect("the recorder must retain its request log");
    requests
        .iter()
        .filter(|request| request.url.path() == TELEMETRY_PATH)
        .map(|request| {
            serde_json::from_slice::<Value>(&request.body).unwrap_or_else(|error| {
                panic!(
                    "a telemetry batch must be JSON: {error}\nbody: {}",
                    String::from_utf8_lossy(&request.body)
                )
            })
        })
        .collect()
}

/// How many chat completions the mock model served.
///
/// The sentinel test's whole claim is that a prompt which *did* reach a model
/// did not reach a batch, so a run where the turn never happened would be
/// vacuous.
async fn model_request_count(server: &MockServer) -> usize {
    server
        .received_requests()
        .await
        .expect("the recorder must retain its request log")
        .iter()
        .filter(|request| request.url.path() == MODEL_PATH)
        .count()
}

async fn assert_no_batches(server: &MockServer, why: &str) {
    let batches = recorded_batches(server).await;
    assert!(
        batches.is_empty(),
        "{why}: expected zero telemetry requests, recorded {}:\n{}",
        batches.len(),
        serde_json::to_string_pretty(&batches).unwrap_or_default()
    );
}

// ── The client works ─────────────────────────────────────────────────────

/// Without this, every zero-request test below would also pass against a
/// client that never sends anything.
#[tokio::test(flavor = "current_thread")]
async fn enabled_and_accepted_posts_exactly_one_batch() {
    let server = start_recorder().await;
    let fixture = Fixture::new().with_endpoint(&server.uri());
    fixture.write_config("telemetry = true\n");
    fixture.record_notice(true);

    fixture.run_completions();

    let batches = recorded_batches(&server).await;
    assert_eq!(batches.len(), 1, "one session is one batch");
    let batch = &batches[0];
    assert_eq!(batch["schema_version"], 1);
    assert_eq!(batch["surface"], "cli");
    assert!(
        batch["install_id"]
            .as_str()
            .is_some_and(|id| id.len() == 36),
        "install_id must be a UUID, got {:?}",
        batch["install_id"]
    );
    let events = batch["events"].as_array().expect("events array");
    assert!(
        events.iter().any(|event| event["event"] == "session_start"),
        "a batch must carry the session it describes: {batch}"
    );
    assert!(
        events.iter().any(|event| event["event"] == "session_end"),
        "the shutdown flush must carry session_end: {batch}"
    );
}

// ── Off is real ──────────────────────────────────────────────────────────

/// The only test that proves the emitting process reads the config *file*.
///
/// No environment variable is set here on purpose. `CODEWHALE_TELEMETRY=0` and
/// the config key travel different paths, and v1 of this design shipped a
/// kill switch that only the env half ever reached.
#[tokio::test(flavor = "current_thread")]
async fn config_file_only_opt_out_sends_zero_requests() {
    let server = start_recorder().await;
    let fixture = Fixture::new().with_endpoint(&server.uri());
    fixture.write_config("telemetry = false\n");
    fixture.record_notice(true);

    let output = fixture.run_completions();
    assert!(output.status.success());

    assert_no_batches(&server, "`telemetry = false` in the config file").await;
}

#[tokio::test(flavor = "current_thread")]
async fn telemetry_disabled_by_env_sends_zero_requests() {
    let server = start_recorder().await;
    let fixture = Fixture::new().with_endpoint(&server.uri());
    fixture.write_config("telemetry = true\n");
    fixture.record_notice(true);

    let mut command = fixture.command();
    command
        .env("CODEWHALE_TELEMETRY", "0")
        .args([
            "--config",
            fixture.config_path.to_str().expect("config path"),
            "completions",
            "bash",
        ])
        .output()
        .expect("run codewhale-tui completions");

    assert_no_batches(&server, "`CODEWHALE_TELEMETRY=0`").await;
}

/// An unparseable env value fails **closed**, rather than falling through to
/// the config file's `true`.
#[tokio::test(flavor = "current_thread")]
async fn an_unparseable_telemetry_env_value_sends_zero_requests() {
    let server = start_recorder().await;
    let fixture = Fixture::new().with_endpoint(&server.uri());
    fixture.write_config("telemetry = true\n");
    fixture.record_notice(true);

    fixture
        .command()
        .env("CODEWHALE_TELEMETRY", "maybe")
        .args([
            "--config",
            fixture.config_path.to_str().expect("config path"),
            "completions",
            "bash",
        ])
        .output()
        .expect("run codewhale-tui completions");

    assert_no_batches(&server, "`CODEWHALE_TELEMETRY=maybe`").await;
}

/// The notice record is an independent AND condition. A pre-existing
/// `telemetry = true` is not consent: the key has been settable and inert for
/// a long time, so anyone who set it set a no-op.
#[tokio::test(flavor = "current_thread")]
async fn telemetry_enabled_without_notice_sends_zero_requests() {
    let server = start_recorder().await;
    let fixture = Fixture::new().with_endpoint(&server.uri());
    fixture.write_config("telemetry = true\n");
    // Deliberately no `record_notice`.

    fixture.run_completions();

    assert_no_batches(&server, "`telemetry = true` with no notice decision").await;
    assert!(
        !fixture.telemetry_root().exists(),
        "a run that was never permitted to collect must not create {}",
        fixture.telemetry_root().display()
    );
}

/// A decision recorded against a *different* notice version is stale: the
/// content changed, so the answer is owed again and nothing is collected in
/// the meantime.
#[tokio::test(flavor = "current_thread")]
async fn a_stale_notice_version_sends_zero_requests() {
    let server = start_recorder().await;
    let fixture = Fixture::new().with_endpoint(&server.uri());
    fixture.write_config("telemetry = true\n");
    let mut state = SetupState::default();
    state.record_telemetry_notice("0", true);
    state
        .save_to(&fixture.setup_state_path())
        .expect("write setup state");

    fixture.run_completions();

    assert_no_batches(&server, "a decision recorded for an older notice version").await;
}

// ── Nothing survives a disable ───────────────────────────────────────────

/// A human's "off" wipes: tombstone first, data truncated, lock file left in
/// place, identity removed.
#[tokio::test(flavor = "current_thread")]
async fn disabling_after_buffering_wipes_and_sends_nothing() {
    let server = start_recorder().await;
    let fixture = Fixture::new().with_endpoint(&server.uri());
    let root = fixture.telemetry_root();
    seed_consenting_home(&root);
    fixture.write_config("telemetry = false\n");
    fixture.record_notice(true);

    fixture.run_completions();

    assert_no_batches(&server, "an explicit opt-out with a populated buffer").await;
    assert!(
        root.join("disabled").exists(),
        "the tombstone is written first and never removed"
    );
    assert_eq!(
        std::fs::read(root.join("buffer.jsonl")).expect("buffer survives as an empty file"),
        Vec::<u8>::new(),
        "buffered events must be truncated, not sent"
    );
    assert!(
        root.join("buffer.jsonl.lock").exists(),
        "the lock file is never unlinked: replacing it would leave appenders \
         and compactors holding different inodes"
    );
    assert!(
        !root.join("install_id.json").exists(),
        "the install identity must not survive an opt-out"
    );
}

/// "Telemetry resolved to false" is the *default* state of every installation.
/// A wipe keyed on it would delete a consenting user's identity and unflushed
/// buffer every time they ran one command on a fresh machine profile.
#[tokio::test(flavor = "current_thread")]
async fn forced_off_run_preserves_a_consenting_users_state() {
    let server = start_recorder().await;
    let fixture = Fixture::new().with_endpoint(&server.uri());
    let root = fixture.telemetry_root();
    seed_consenting_home(&root);
    let before = snapshot(&root);
    // No `telemetry` key at all, and no notice decision: forced off, not an
    // answer.
    fixture.write_config("");

    fixture.run_completions();

    assert_no_batches(&server, "a forced-off run").await;
    assert_eq!(
        snapshot(&root),
        before,
        "a forced-off run must leave a consenting user's telemetry state byte-identical"
    );
}

/// A run that was never permitted to collect creates no directory at all —
/// which is also what makes the process panic hook, installed before the
/// command line is even parsed, write nothing for a disabled user.
#[tokio::test(flavor = "current_thread")]
async fn a_disabled_run_creates_no_telemetry_directory() {
    let server = start_recorder().await;
    let fixture = Fixture::new().with_endpoint(&server.uri());
    fixture.write_config("telemetry = false\n");
    fixture.record_notice(false);

    fixture.run_completions();

    assert_no_batches(&server, "a declined run").await;
    assert!(
        !fixture.telemetry_root().exists(),
        "nothing may be created for a user who declined on a fresh home"
    );
}

// ── The notice is never answered by silence ──────────────────────────────

/// Deferral is not a decision. `--skip-onboarding` records nothing, prints
/// nothing, and sends nothing — unlike the constitution checkpoint, which does
/// persist a `Deferred` completion.
#[tokio::test(flavor = "current_thread")]
async fn skip_onboarding_writes_no_telemetry_decision() {
    let server = start_recorder().await;
    let fixture = Fixture::new().with_endpoint(&server.uri());
    fixture.write_config("telemetry = true\n");

    let mut command = fixture.command();
    let output = command
        .args([
            "--config",
            fixture.config_path.to_str().expect("config path"),
            "--skip-onboarding",
            "completions",
            "bash",
        ])
        .output()
        .expect("run codewhale-tui completions");
    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    for stream in [&stdout, &stderr] {
        assert!(
            !stream.contains("keep telemetry off"),
            "the notice must not be rendered on a path that cannot answer it"
        );
    }

    if let Some(state) = SetupState::load_from(&fixture.setup_state_path()) {
        assert_eq!(
            state.telemetry_notice_decided_for, None,
            "skip-onboarding must leave the telemetry decision unset"
        );
    }
    assert_no_batches(&server, "`--skip-onboarding`").await;
}

// ── Payload red lines, through a real turn ───────────────────────────────

/// Five sentinel classes planted through real inputs — the prompt, a workspace
/// filename, a custom `[providers.<name>]` table key, an MCP server name, and
/// the API key — asserted absent from every recorded batch.
///
/// The API key is held to the stricter standard the harness at
/// `crates/tui/tests/verifiers_harness_contract.rs` applies: absent from stdout,
/// stderr, and every file under the sealed roots as well. The other four
/// legitimately appear in files the product owns — a prompt is in the session
/// transcript, a provider table key is in the config the user wrote — so the
/// claim about them is precisely that they never reach a *batch*.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn batch_contains_no_planted_sentinel() {
    let server = start_recorder().await;
    mount_model(&server, Duration::ZERO).await;
    let fixture = Fixture::new().with_endpoint(&server.uri());
    plant_sentinels(&fixture, &server.uri());

    let output = run_exec(&fixture, SENTINEL_PROMPT);

    assert!(
        model_request_count(&server).await > 0,
        "the sentinel prompt must actually have reached a model, or this test \
         proves nothing\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let batches = recorded_batches(&server).await;
    assert!(
        !batches.is_empty(),
        "this test is only meaningful against a batch that was actually sent"
    );
    let serialized = serde_json::to_string(&batches).expect("serialize batches");
    for sentinel in [
        SENTINEL_PROMPT,
        SENTINEL_FILENAME,
        SENTINEL_PROVIDER_TABLE,
        SENTINEL_MCP_SERVER,
        SENTINEL_API_KEY,
    ] {
        assert!(
            !serialized.contains(sentinel),
            "sentinel `{sentinel}` reached a telemetry batch:\n{serialized}"
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stdout.contains(SENTINEL_API_KEY),
        "the API key leaked into stdout"
    );
    assert!(
        !stderr.contains(SENTINEL_API_KEY),
        "the API key leaked into stderr"
    );
    for file in fixture.written_files() {
        let Ok(bytes) = std::fs::read(&file) else {
            continue;
        };
        assert!(
            !String::from_utf8_lossy(&bytes).contains(SENTINEL_API_KEY),
            "the API key leaked into {}",
            file.display()
        );
    }
}

/// The documented mid-session opt-out — `codewhale config set telemetry false`,
/// written by another process — must be observed by a session that is already
/// running. The flush re-resolves from disk before it sends anything.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mid_session_opt_out_stops_the_shutdown_flush() {
    let server = start_recorder().await;
    // A slow first token gives the second writer a window while the session is
    // armed and buffering.
    mount_model(&server, Duration::from_secs(4)).await;
    let fixture = Fixture::new().with_endpoint(&server.uri());
    plant_sentinels(&fixture, &server.uri());

    let mut command = exec_command(&fixture, "hello");
    let mut child = command.spawn().expect("spawn codewhale-tui exec");

    // Wait until the session has armed and started buffering, then take the
    // documented way out from outside the process.
    let buffer = fixture.telemetry_root().join("buffer.jsonl");
    wait_until(Duration::from_secs(30), || buffer.exists());
    fixture.write_config(&sentinel_config(&server.uri(), false));

    let status = child
        .wait_timeout(EXEC_TIMEOUT)
        .expect("wait for codewhale-tui exec")
        .expect("codewhale-tui exec must exit");
    let _ = status;

    assert_no_batches(&server, "an opt-out written mid-session").await;
    let root = fixture.telemetry_root();
    assert!(
        root.join("disabled").exists(),
        "the opt-out wipe must leave a tombstone the next run also honours"
    );
}

/// Ctrl-C must not wait on a lock a second Codewhale process is holding.
///
/// This is why appends never take the compaction lock: `flock` is per-fd within
/// a process, so a blocking acquisition on the signal path would hang exit for
/// as long as any other holder lives.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ctrl_c_exits_while_a_second_process_holds_the_lock() {
    let server = start_recorder().await;
    mount_model(&server, Duration::from_secs(30)).await;
    let fixture = Fixture::new().with_endpoint(&server.uri());
    plant_sentinels(&fixture, &server.uri());

    let mut command = exec_command(&fixture, "hello");
    let mut child = command.spawn().expect("spawn codewhale-tui exec");

    let root = fixture.telemetry_root();
    let lock_path = root.join("buffer.jsonl.lock");
    // Arming itself takes the lock, so the holder below must wait until the
    // session is armed — otherwise this test would pin a deadlock it created.
    wait_until(Duration::from_secs(30), || {
        root.join("buffer.jsonl").exists()
    });
    let _holder = LockHolder::take(&lock_path);

    // SIGINT to the child alone; the process group belongs to the test runner.
    let pid = child.id() as libc::pid_t;
    // SAFETY: `kill` with a pid this process spawned and has not reaped.
    unsafe {
        libc::kill(pid, libc::SIGINT);
    }

    let started = Instant::now();
    let status = child
        .wait_timeout(Duration::from_secs(10))
        .expect("wait for codewhale-tui exec");
    let status = status.unwrap_or_else(|| {
        let _ = child.kill();
        panic!(
            "Ctrl-C blocked for {:?} while another process held the telemetry lock — \
             the signal path must append without taking it",
            started.elapsed()
        )
    });
    assert_eq!(
        status.code(),
        Some(130),
        "SIGINT must still exit 130 with the telemetry lock held elsewhere"
    );
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "Ctrl-C took {:?} while the telemetry lock was held elsewhere",
        started.elapsed()
    );
}

/// Holds the telemetry compaction lock for the lifetime of the value.
struct LockHolder {
    file: std::fs::File,
}

impl LockHolder {
    fn take(path: &Path) -> Self {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(path)
            .expect("open the telemetry lock");
        let fd = std::os::unix::io::AsRawFd::as_raw_fd(&file);
        // SAFETY: `fd` is owned by `file` and outlives the call.
        let taken = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
        assert_eq!(
            taken, 0,
            "the telemetry lock must be free before the test takes it"
        );
        Self { file }
    }
}

impl Drop for LockHolder {
    fn drop(&mut self) {
        let fd = std::os::unix::io::AsRawFd::as_raw_fd(&self.file);
        // SAFETY: same fd, still owned by `self`.
        unsafe {
            libc::flock(fd, libc::LOCK_UN);
        }
    }
}

// ── exec harness ─────────────────────────────────────────────────────────

fn sse_chunk(value: Value) -> String {
    format!(
        "data: {}\n\n",
        serde_json::to_string(&value).expect("SSE JSON")
    )
}

fn text_sse(text: &str) -> String {
    [
        sse_chunk(json!({
            "id": "chatcmpl-tc",
            "object": "chat.completion.chunk",
            "model": TEST_MODEL,
            "choices": [{"index": 0, "delta": {"content": text}, "finish_reason": null}]
        })),
        sse_chunk(json!({
            "id": "chatcmpl-tc",
            "object": "chat.completion.chunk",
            "model": TEST_MODEL,
            "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 7, "completion_tokens": 2, "total_tokens": 9}
        })),
        "data: [DONE]\n\n".to_string(),
    ]
    .join("")
}

async fn mount_model(server: &MockServer, delay: Duration) {
    Mock::given(method("GET"))
        .and(path_matcher("/v1/models"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_json(json!({
                    "object": "list",
                    "data": [{"id": TEST_MODEL, "object": "model"}]
                })),
        )
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path_matcher(MODEL_PATH))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .insert_header("cache-control", "no-cache")
                .set_body_string(text_sse("acknowledged"))
                .set_delay(delay),
        )
        .mount(server)
        .await;
}

/// A config whose provider table key, MCP server name, and workspace file are
/// all sentinels, so a leak has somewhere to come from.
fn sentinel_config(base_url: &str, telemetry: bool) -> String {
    format!(
        "telemetry = {telemetry}\nprovider = \"{SENTINEL_PROVIDER_TABLE}\"\n\n\
         [providers.{SENTINEL_PROVIDER_TABLE}]\n\
         kind = \"openai-compatible\"\n\
         base_url = \"{base_url}/v1\"\n\
         model = \"{TEST_MODEL}\"\n\
         api_key_env = \"{SENTINEL_API_KEY_ENV}\"\n"
    )
}

fn plant_sentinels(fixture: &Fixture, base_url: &str) {
    fixture.write_config(&sentinel_config(base_url, true));
    fixture.record_notice(true);
    std::fs::write(
        fixture.workspace.join(SENTINEL_FILENAME),
        "sentinel workspace file\n",
    )
    .expect("plant workspace file");
    std::fs::write(
        fixture.codewhale_home.join("mcp.json"),
        json!({"mcpServers": {SENTINEL_MCP_SERVER: {"command": "/bin/true", "args": []}}})
            .to_string(),
    )
    .expect("plant MCP config");
}

fn exec_command(fixture: &Fixture, prompt: &str) -> Command {
    let mut command = fixture.command();
    command
        .env(
            "CODEWHALE_MCP_CONFIG",
            fixture.codewhale_home.join("mcp.json"),
        )
        .env(SENTINEL_API_KEY_ENV, SENTINEL_API_KEY)
        .args([
            "--config",
            fixture.config_path.to_str().expect("config path"),
            "--workspace",
            fixture.workspace.to_str().expect("workspace path"),
            "--no-project-config",
            "--skip-onboarding",
            "exec",
            "--auto",
            "--output-format",
            "stream-json",
            "--",
            prompt,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

fn run_exec(fixture: &Fixture, prompt: &str) -> Output {
    let mut command = exec_command(fixture, prompt);
    let mut child = command.spawn().expect("spawn codewhale-tui exec");
    let stdout = read_in_background(child.stdout.take().expect("stdout pipe"));
    let stderr = read_in_background(child.stderr.take().expect("stderr pipe"));
    let status = match child.wait_timeout(EXEC_TIMEOUT).expect("wait for exec") {
        Some(status) => status,
        None => {
            let _ = child.kill();
            panic!("codewhale-tui exec did not exit within {EXEC_TIMEOUT:?}");
        }
    };
    Output {
        status,
        stdout: stdout.join().expect("stdout reader"),
        stderr: stderr.join().expect("stderr reader"),
    }
}

fn read_in_background(mut pipe: impl Read + Send + 'static) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut buffer = Vec::new();
        let _ = pipe.read_to_end(&mut buffer);
        buffer
    })
}

fn wait_until(limit: Duration, mut ready: impl FnMut() -> bool) {
    let started = Instant::now();
    while started.elapsed() < limit {
        if ready() {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("condition was not reached within {limit:?}");
}

// ── Seeded state ─────────────────────────────────────────────────────────

/// A home that already belongs to a consenting user: an identity, a populated
/// buffer, a flush record, and the lock file.
fn seed_consenting_home(root: &Path) {
    std::fs::create_dir_all(root).expect("create telemetry root");
    std::fs::write(
        root.join("install_id.json"),
        json!({
            "schema_version": 1,
            "install_id": "11111111-2222-3333-4444-555555555555",
            "rotated_at": "2026-01-01T00:00:00Z"
        })
        .to_string(),
    )
    .expect("seed install id");
    std::fs::write(
        root.join("state.json"),
        json!({"schema_version": 1, "last_version": "0.0.1"}).to_string(),
    )
    .expect("seed state");
    std::fs::write(
        root.join("buffer.jsonl"),
        format!(
            "{}\n",
            json!({"event": "session_start", "source": "unknown"})
        ),
    )
    .expect("seed buffer");
    std::fs::write(root.join("buffer.jsonl.lock"), b"").expect("seed lock file");
}

fn snapshot(root: &Path) -> Vec<(String, Vec<u8>)> {
    let mut files = Vec::new();
    collect_files(root, &mut files);
    let mut out: Vec<(String, Vec<u8>)> = files
        .into_iter()
        .map(|path| {
            let name = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .into_owned();
            let bytes = std::fs::read(&path).unwrap_or_default();
            (name, bytes)
        })
        .collect();
    out.sort();
    out
}
