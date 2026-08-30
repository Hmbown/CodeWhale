//! Desktop Phase 0 acceptance for the daemon socket: spawn the daemon, connect
//! over the unix socket, complete the attach/claim handshake, round-trip
//! requests through the same JSON-RPC dispatcher the stdio transport uses,
//! and shut down cleanly (socket file removed, listener gone).

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use codewhale_app_server::daemon_socket::{
    DaemonSocketError, DaemonSocketOptions, bind_daemon_socket,
};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::task::JoinHandle;

static NONCE: AtomicU64 = AtomicU64::new(0);

/// A short, unique socket path: unix socket paths are capped near 100 bytes,
/// so `std::env::temp_dir()` (deep under `/var/folders` on macOS) is too long.
/// `/tmp` is the same choice the hooks crate's socket test makes.
fn short_socket_root(label: &str) -> PathBuf {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_millis()
        % 1_000_000;
    let nonce = NONCE.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let root = PathBuf::from("/tmp").join(format!("cw-ds-{label}-{pid}-{nonce}-{millis}"));
    assert!(
        root.as_os_str().len() < 60,
        "socket root too long for a unix socket test: {}",
        root.display()
    );
    root
}

struct Harness {
    root: PathBuf,
    socket_path: PathBuf,
    _config_dir: tempfile::TempDir,
}

impl Harness {
    fn new(label: &str) -> Self {
        let root = short_socket_root(label);
        let config_dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(config_dir.path().join("config.toml"), "").expect("config");
        Self {
            socket_path: root.join("run").join("daemon.sock"),
            root,
            _config_dir: config_dir,
        }
    }

    fn options(&self) -> DaemonSocketOptions {
        DaemonSocketOptions {
            socket_path: Some(self.socket_path.clone()),
            config_path: Some(self._config_dir.path().join("config.toml")),
        }
    }

    /// Bind and serve on a background task; returns the serve join handle.
    async fn spawn_daemon(&self) -> JoinHandle<Result<(), DaemonSocketError>> {
        let daemon = bind_daemon_socket(self.options())
            .await
            .expect("bind daemon socket");
        assert_eq!(daemon.local_path(), self.socket_path.as_path());
        tokio::spawn(daemon.serve())
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

struct Client {
    reader: BufReader<OwnedReadHalf>,
    writer: OwnedWriteHalf,
}

impl Client {
    async fn connect(path: &Path) -> Self {
        let stream = tokio::time::timeout(Duration::from_secs(5), UnixStream::connect(path))
            .await
            .expect("connect timeout")
            .expect("connect");
        let (rx, writer) = stream.into_split();
        Self {
            reader: BufReader::new(rx),
            writer,
        }
    }

    async fn call(&mut self, id: u64, method: &str, params: Value) -> Value {
        let line = serde_json::to_string(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }))
        .expect("encode");
        self.writer
            .write_all(format!("{line}\n").as_bytes())
            .await
            .expect("write");
        let mut response = String::new();
        let read = tokio::time::timeout(
            Duration::from_secs(10),
            self.reader.read_line(&mut response),
        )
        .await
        .expect("response timeout")
        .expect("read");
        assert!(
            read > 0,
            "daemon closed the connection before answering `{method}`"
        );
        let value: Value = serde_json::from_str(&response).expect("json response");
        assert_eq!(value["id"], json!(id), "response id mismatch: {value}");
        value
    }

    async fn attach(&mut self, id: u64, name: &str, mode: &str) -> Value {
        self.call(
            id,
            "daemon/attach",
            json!({ "client": { "name": name, "version": "0.0.0-test", "pid": std::process::id() }, "mode": mode }),
        )
        .await
    }

    /// Read until EOF; proves the daemon closed the socket.
    async fn wait_for_close(mut self) {
        let mut sink = String::new();
        let read = tokio::time::timeout(Duration::from_secs(10), self.reader.read_line(&mut sink))
            .await
            .expect("close timeout")
            .expect("read");
        assert_eq!(read, 0, "expected EOF, got: {sink}");
    }
}

async fn wait_for_socket_removed(path: &Path) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while path.exists() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("socket file must be removed on shutdown");
}

#[tokio::test]
async fn owner_attaches_round_trips_and_shuts_down_cleanly() {
    let harness = Harness::new("owner");
    let server = harness.spawn_daemon().await;

    let socket_mode = std::fs::metadata(&harness.socket_path)
        .expect("socket metadata")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(socket_mode, 0o600, "socket must be private to the user");
    let dir_mode = std::fs::metadata(harness.socket_path.parent().expect("parent"))
        .expect("dir metadata")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(dir_mode, 0o700, "runtime dir must be private to the user");

    let mut client = Client::connect(&harness.socket_path).await;

    // Anything but healthz before attaching is refused with a typed error:
    // a read-only probe, a thread/* read, and a prompt run alike.
    for (id, method, params) in [
        (1, "capabilities", json!({})),
        (10, "thread/list", json!({})),
        (11, "prompt/run", json!({ "prompt": "hi" })),
    ] {
        let early = client.call(id, method, params).await;
        assert_eq!(early["error"]["code"], json!(-32010), "{method}: {early}");
        assert_eq!(early["error"]["data"]["error"], json!("attach_required"));
        assert_eq!(early["error"]["data"]["method"], json!(method));
    }

    // healthz is allowed pre-attach so a shell can probe liveness first.
    let health = client.call(2, "healthz", json!({})).await;
    assert_eq!(health["result"]["status"], json!("ok"), "{health}");
    assert_eq!(health["result"]["transport"], json!("unix-socket"));

    let attached = client.attach(3, "codewhale-desktop", "claim").await;
    assert_eq!(attached["result"]["attached"], json!(true), "{attached}");
    assert_eq!(attached["result"]["role"], json!("owner"));
    assert_eq!(attached["result"]["transport"], json!("unix-socket"));
    assert_eq!(
        attached["result"]["daemon"]["pid"],
        json!(std::process::id())
    );
    assert_eq!(
        attached["result"]["daemon"]["version"],
        json!(env!("CARGO_PKG_VERSION"))
    );
    assert_eq!(
        attached["result"]["owner"]["name"],
        json!("codewhale-desktop")
    );
    assert_eq!(attached["result"]["connections"], json!(1));

    // Post-attach, the socket transport advertises its own handshake next to
    // the stdio method set.
    let advertised = client.call(8, "capabilities", json!({})).await;
    let methods = advertised["result"]["methods"]
        .as_array()
        .expect("methods array");
    assert_eq!(methods[0], json!("healthz"), "{advertised}");
    assert_eq!(methods[1], json!("daemon/attach"), "{advertised}");
    assert!(methods.contains(&json!("shutdown")));

    // Round-trip JSON-RPC requests through the shared dispatcher: `app/*`
    // methods in, their JSON results out — byte-for-byte the shapes the
    // stdio transport emits. (No protocol-crate Op/EventMsg envelope is on
    // this wire; the framing is the stdio transport's newline-delimited
    // JSON-RPC.)
    let caps = client.call(4, "app/capabilities", json!({})).await;
    assert_eq!(caps["result"]["ok"], json!(true), "{caps}");
    assert!(caps["result"]["data"]["routes"].is_array());
    let config = client
        .call(5, "app/config/get", json!({ "key": "model" }))
        .await;
    assert_eq!(config["result"]["ok"], json!(true), "{config}");
    assert_eq!(config["result"]["data"]["key"], json!("model"));

    // A second attach on an attached connection is a typed refusal, not
    // method_not_found.
    let again = client.attach(6, "codewhale-desktop", "attach").await;
    assert_eq!(again["error"]["code"], json!(-32014), "{again}");

    let stopped = client.call(7, "shutdown", json!({})).await;
    assert_eq!(stopped["result"]["status"], json!("stopped"), "{stopped}");

    let outcome = tokio::time::timeout(Duration::from_secs(10), server)
        .await
        .expect("daemon must exit after the owner's shutdown")
        .expect("join");
    outcome.expect("serve result");
    wait_for_socket_removed(&harness.socket_path).await;
    client.wait_for_close().await;
}

#[tokio::test]
async fn guests_share_the_daemon_but_cannot_stop_it() {
    let harness = Harness::new("guest");
    let server = harness.spawn_daemon().await;

    let mut owner = Client::connect(&harness.socket_path).await;
    let claimed = owner.attach(1, "desktop-window-1", "claim").await;
    assert_eq!(claimed["result"]["role"], json!("owner"), "{claimed}");

    let mut guest = Client::connect(&harness.socket_path).await;
    let lost = guest.attach(1, "desktop-window-2", "claim").await;
    assert_eq!(lost["error"]["code"], json!(-32011), "{lost}");
    assert_eq!(
        lost["error"]["data"]["owner"]["name"],
        json!("desktop-window-1")
    );

    let attached = guest.attach(2, "desktop-window-2", "attach").await;
    assert_eq!(attached["result"]["role"], json!("attached"), "{attached}");
    assert_eq!(
        attached["result"]["owner"]["name"],
        json!("desktop-window-1")
    );
    assert_eq!(attached["result"]["connections"], json!(2));

    let health = guest.call(3, "healthz", json!({})).await;
    assert_eq!(health["result"]["status"], json!("ok"));

    let refused = guest.call(4, "shutdown", json!({})).await;
    assert_eq!(refused["error"]["code"], json!(-32012), "{refused}");
    assert_eq!(refused["error"]["data"]["error"], json!("not_daemon_owner"));
    assert!(
        !server.is_finished(),
        "a guest's shutdown must not stop the daemon"
    );
    assert!(harness.socket_path.exists());

    // Once the owner leaves, the slot frees and a relaunched shell can claim.
    drop(owner);
    let mut relaunched = Client::connect(&harness.socket_path).await;
    let reclaimed = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let response = relaunched.attach(1, "desktop-relaunch", "claim").await;
            if response.get("result").is_some() {
                return response;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("owner slot must free when the owner disconnects");
    assert_eq!(reclaimed["result"]["role"], json!("owner"), "{reclaimed}");

    // The guest is still attached and served while the new owner is in.
    let health = guest.call(5, "healthz", json!({})).await;
    assert_eq!(health["result"]["status"], json!("ok"));

    let stopped = relaunched.call(2, "shutdown", json!({})).await;
    assert_eq!(stopped["result"]["status"], json!("stopped"));
    tokio::time::timeout(Duration::from_secs(10), server)
        .await
        .expect("daemon exits")
        .expect("join")
        .expect("serve result");
    wait_for_socket_removed(&harness.socket_path).await;
    // The owner's shutdown closes every other connection, not just its own.
    guest.wait_for_close().await;
    relaunched.wait_for_close().await;
}

#[tokio::test]
async fn version_skew_is_refused_at_attach() {
    let harness = Harness::new("skew");
    let server = harness.spawn_daemon().await;
    let mut client = Client::connect(&harness.socket_path).await;
    let refused = client
        .call(
            1,
            "daemon/attach",
            json!({ "client": { "name": "old-desktop" }, "expect_daemon_version": "0.0.1-other" }),
        )
        .await;
    assert_eq!(refused["error"]["code"], json!(-32013), "{refused}");
    assert_eq!(
        refused["error"]["data"]["actual"],
        json!(env!("CARGO_PKG_VERSION"))
    );

    let daemon = bind_daemon_socket(harness.options()).await;
    // Meanwhile the original daemon is live, so a second bind must refuse.
    match daemon {
        Err(DaemonSocketError::AlreadyRunning { path }) => {
            assert_eq!(path, harness.socket_path);
        }
        Err(other) => panic!("unexpected error: {other}"),
        Ok(_) => panic!("second daemon must not replace a live socket"),
    }
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn stale_socket_is_cleaned_up_and_foreign_files_are_refused() {
    let harness = Harness::new("stale");
    std::fs::create_dir_all(harness.socket_path.parent().expect("parent")).expect("mkdir");

    // A socket file whose listener is gone: bind must reclaim it.
    {
        let dead = tokio::net::UnixListener::bind(&harness.socket_path).expect("bind dead");
        drop(dead);
    }
    assert!(
        harness.socket_path.exists(),
        "dropping a listener leaves the file"
    );
    let daemon = bind_daemon_socket(harness.options())
        .await
        .expect("stale socket must be reclaimed");
    let handle = daemon.shutdown_handle();
    let server = tokio::spawn(daemon.serve());
    let mut client = Client::connect(&harness.socket_path).await;
    let health = client.call(1, "healthz", json!({})).await;
    assert_eq!(health["result"]["status"], json!("ok"));
    handle.trigger();
    tokio::time::timeout(Duration::from_secs(10), server)
        .await
        .expect("daemon exits on handle")
        .expect("join")
        .expect("serve result");
    wait_for_socket_removed(&harness.socket_path).await;

    // A regular file at the path is never deleted.
    std::fs::write(&harness.socket_path, b"not a socket").expect("write file");
    match bind_daemon_socket(harness.options()).await {
        Err(DaemonSocketError::NotASocket { path }) => assert_eq!(path, harness.socket_path),
        Err(other) => panic!("unexpected error: {other}"),
        Ok(_) => panic!("must refuse to replace a non-socket"),
    }
    assert_eq!(
        std::fs::read(&harness.socket_path).expect("file intact"),
        b"not a socket"
    );
}
