//! Unix-domain-socket daemon transport (Desktop Phase 0, socket half).
//!
//! The desktop shell attaches to a long-lived `codewhale app-server --socket`
//! daemon over a local socket instead of a TCP port: local multi-client,
//! peer-credential auth, nothing to firewall (CORE-PROTOCOL spec §5). The
//! wire is *identical* to the `--stdio` transport — newline-delimited
//! JSON-RPC 2.0 driven by the same [`crate::run_stdio_loop`] — with exactly
//! one addition in front of it: a `daemon/attach` handshake that establishes
//! who this client is and whether it owns the daemon.
//!
//! # Endpoint resolution
//!
//! In precedence order (see [`resolve_socket_path`]):
//!
//! 1. an explicit path (`--socket-path`);
//! 2. `$CODEWHALE_HOME/run/daemon.sock` when `CODEWHALE_HOME` is set — an
//!    explicit home is an isolation boundary, so its daemon must not collide
//!    with the default one;
//! 3. `$XDG_RUNTIME_DIR/codewhale/daemon.sock`;
//! 4. macOS: `~/Library/Application Support/codewhale/daemon.sock`;
//! 5. `~/.codewhale/run/daemon.sock`.
//!
//! Windows is reserved as the named pipe [`WINDOWS_NAMED_PIPE`]; binding
//! there returns [`DaemonSocketError::UnsupportedPlatform`] rather than
//! silently falling back to TCP.
//!
//! # Ownership
//!
//! Hermes' claim model, server-side: a client attaches with `mode: "claim"`
//! (it spawned the daemon and will manage its lifetime) or `mode: "attach"`
//! (it found a healthy daemon and is a guest). Only the current owner may
//! `shutdown` the daemon; guests get `not_daemon_owner`. When the owner
//! disconnects the slot frees, so a relaunched shell can re-claim the daemon
//! it left running — sessions survive UI restarts because the daemon does.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Basename of the daemon socket inside the Codewhale runtime directory.
pub const DAEMON_SOCKET_FILE_NAME: &str = "daemon.sock";

/// Reserved Windows endpoint. Not implemented yet; binding on Windows fails
/// with [`DaemonSocketError::UnsupportedPlatform`] naming this pipe.
pub const WINDOWS_NAMED_PIPE: &str = r"\\.\pipe\codewhale-daemon";

/// JSON-RPC method a client must send first on a daemon-socket connection.
pub const ATTACH_METHOD: &str = "daemon/attach";

/// Longest socket path the kernel accepts (`sun_path` minus the NUL).
pub const MAX_SOCKET_PATH_BYTES: usize = if cfg!(any(target_os = "macos", target_os = "ios")) {
    103
} else {
    107
};

/// Typed failures of the daemon socket transport.
#[derive(Debug, thiserror::Error)]
pub enum DaemonSocketError {
    /// The platform has no daemon socket implementation. Never a silent
    /// fallback: the caller must pick another transport explicitly.
    #[error(
        "the daemon socket transport is not supported on {platform}; the reserved endpoint \
         there is the named pipe {planned_endpoint}, which is not implemented yet"
    )]
    UnsupportedPlatform {
        platform: &'static str,
        planned_endpoint: &'static str,
    },
    /// No home directory (or runtime directory) to derive a default path from.
    #[error(
        "cannot resolve the Codewhale runtime directory for the daemon socket: no home directory"
    )]
    RuntimeDirUnavailable,
    /// `CODEWHALE_HOME` is set but not a usable absolute path.
    #[error("invalid CODEWHALE_HOME override: {0}")]
    InvalidHomeOverride(String),
    /// Unix socket paths are limited to roughly one hundred bytes.
    #[error("daemon socket path {} is {len} bytes; this platform allows at most {max}", path.display())]
    PathTooLong {
        path: PathBuf,
        len: usize,
        max: usize,
    },
    /// Something other than a socket already sits at the path. Refused so a
    /// misconfigured path can never delete a user's file.
    #[error("{} exists and is not a unix socket; refusing to remove it", path.display())]
    NotASocket { path: PathBuf },
    /// A daemon answered on the socket: this one must not replace it.
    #[error(
        "a live listener already answers on {}; refusing to replace it (another codewhale daemon, or something else bound to this path)",
        path.display()
    )]
    AlreadyRunning { path: PathBuf },
    /// The liveness probe neither connected nor was refused within the
    /// budget. Refused rather than clobbered; remove the file by hand if the
    /// old daemon is truly gone.
    #[error("liveness probe of {} timed out; refusing to replace a socket that may be live", path.display())]
    ProbeTimedOut { path: PathBuf },
    /// Filesystem or socket I/O failed.
    #[error("{context} ({})", path.display())]
    Io {
        context: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// The app-server state (config, state store, runtime) failed to build.
    #[error("failed to build daemon state")]
    State(#[source] anyhow::Error),
}

/// How to start the daemon socket transport.
#[derive(Debug, Clone, Default)]
pub struct DaemonSocketOptions {
    /// Explicit socket path; `None` resolves the platform default.
    pub socket_path: Option<PathBuf>,
    /// Explicit config file, like `app-server --config`.
    pub config_path: Option<PathBuf>,
}

/// Inputs to [`resolve_socket_path`], separated from the environment so the
/// precedence rules are a pure, testable function.
#[derive(Debug, Clone, Default)]
pub struct SocketPathInputs {
    /// `--socket-path`.
    pub explicit: Option<PathBuf>,
    /// A valid explicit `CODEWHALE_HOME`.
    pub codewhale_home_override: Option<PathBuf>,
    /// `$XDG_RUNTIME_DIR`, when set and non-empty.
    pub xdg_runtime_dir: Option<PathBuf>,
    /// The user's home directory.
    pub user_home: Option<PathBuf>,
    /// Whether the macOS Application Support layout applies.
    pub macos: bool,
}

impl SocketPathInputs {
    /// Capture the live environment.
    pub fn from_environment(explicit: Option<PathBuf>) -> Result<Self, DaemonSocketError> {
        let codewhale_home_override = codewhale_paths::codewhale_home_override()
            .map_err(|err| DaemonSocketError::InvalidHomeOverride(err.to_string()))?;
        let xdg_runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from);
        Ok(Self {
            explicit,
            codewhale_home_override,
            xdg_runtime_dir,
            user_home: codewhale_paths::user_home(),
            macos: cfg!(target_os = "macos"),
        })
    }
}

/// Apply the precedence rules documented at the module level and enforce the
/// kernel's path-length limit.
pub fn resolve_socket_path(inputs: &SocketPathInputs) -> Result<PathBuf, DaemonSocketError> {
    let path = if let Some(explicit) = inputs.explicit.clone() {
        explicit
    } else if let Some(home) = inputs.codewhale_home_override.clone() {
        home.join("run").join(DAEMON_SOCKET_FILE_NAME)
    } else if let Some(runtime_dir) = inputs.xdg_runtime_dir.clone() {
        runtime_dir.join("codewhale").join(DAEMON_SOCKET_FILE_NAME)
    } else {
        let user_home = inputs
            .user_home
            .clone()
            .ok_or(DaemonSocketError::RuntimeDirUnavailable)?;
        if inputs.macos {
            user_home
                .join("Library")
                .join("Application Support")
                .join("codewhale")
                .join(DAEMON_SOCKET_FILE_NAME)
        } else {
            user_home
                .join(codewhale_paths::CODEWHALE_APP_DIR)
                .join("run")
                .join(DAEMON_SOCKET_FILE_NAME)
        }
    };
    let len = path.as_os_str().len();
    if len > MAX_SOCKET_PATH_BYTES {
        return Err(DaemonSocketError::PathTooLong {
            path,
            len,
            max: MAX_SOCKET_PATH_BYTES,
        });
    }
    Ok(path)
}

/// The socket path this host would use with no explicit override.
#[cfg(unix)]
pub fn default_socket_path() -> Result<PathBuf, DaemonSocketError> {
    resolve_socket_path(&SocketPathInputs::from_environment(None)?)
}

/// The socket path this host would use with no explicit override.
#[cfg(not(unix))]
pub fn default_socket_path() -> Result<PathBuf, DaemonSocketError> {
    Err(unsupported_platform())
}

/// Who is on the other end of a daemon-socket connection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientIdentity {
    /// Product name of the client, e.g. `codewhale-desktop`.
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
}

/// Ownership intent carried by `daemon/attach`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachMode {
    /// A guest: use the daemon, never stop it.
    #[default]
    Attach,
    /// The daemon's owner: may `shutdown`. Fails if a live owner exists.
    Claim,
}

/// Role granted by a successful attach.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachRole {
    Owner,
    Attached,
}

/// `daemon/attach` params.
#[derive(Debug, Clone, Deserialize)]
pub struct AttachParams {
    pub client: ClientIdentity,
    #[serde(default)]
    pub mode: AttachMode,
    /// Bundle-skew guard: when set, the daemon refuses the attach unless its
    /// own version string matches exactly.
    #[serde(default)]
    pub expect_daemon_version: Option<String>,
}

/// The typed refusal every non-unix entry point returns. Unused in the unix
/// library build by construction; the tests pin its wording on every host.
#[cfg_attr(unix, allow(dead_code))]
fn unsupported_platform() -> DaemonSocketError {
    DaemonSocketError::UnsupportedPlatform {
        platform: std::env::consts::OS,
        planned_endpoint: WINDOWS_NAMED_PIPE,
    }
}

#[cfg(unix)]
mod platform {
    use std::collections::HashMap;
    use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt};
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use anyhow::Result;
    use serde_json::{Value, json};
    use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, BufReader, Lines};
    use tokio::net::{UnixListener, UnixStream};
    use tokio::sync::watch;
    use tokio::task::JoinSet;

    use super::{
        ATTACH_METHOD, AttachMode, AttachParams, AttachRole, ClientIdentity, DaemonSocketError,
        DaemonSocketOptions, SocketPathInputs, resolve_socket_path,
    };
    use crate::{
        AppState, AppTransport, JsonRpcError, ParsedStdioLine, ShutdownAuthority, StdioLoopExit,
        StdioLoopPolicy, build_state_with_transport, dispatch_stdio_request_with_writer,
        jsonrpc_error, jsonrpc_result, legacy_deepseek_compat, params_or_object, parse_params,
        parse_stdio_line, run_stdio_loop, write_stdio_line,
    };

    /// How long the stale-socket probe waits for a connect to resolve.
    const PROBE_TIMEOUT: Duration = Duration::from_secs(1);

    /// Facts about this daemon, reported in every attach reply.
    #[derive(Debug)]
    struct DaemonInfo {
        pid: u32,
        version: &'static str,
        /// Owner uid of the socket file, i.e. the daemon's effective uid.
        uid: u32,
        socket_path: PathBuf,
        started_at: Instant,
    }

    #[derive(Debug, Default)]
    struct ConnectionRegistry {
        next_id: u64,
        connections: HashMap<u64, ClientIdentity>,
        owner: Option<u64>,
    }

    impl ConnectionRegistry {
        fn register(&mut self, client: ClientIdentity) -> u64 {
            self.next_id += 1;
            let id = self.next_id;
            self.connections.insert(id, client);
            id
        }

        /// Take the owner slot, or report who holds it.
        fn claim(&mut self, id: u64) -> Result<(), ClientIdentity> {
            if let Some(owner_id) = self.owner
                && owner_id != id
                && let Some(owner) = self.connections.get(&owner_id)
            {
                return Err(owner.clone());
            }
            self.owner = Some(id);
            Ok(())
        }

        fn owner(&self) -> Option<ClientIdentity> {
            self.owner.and_then(|id| self.connections.get(&id)).cloned()
        }

        fn remove(&mut self, id: u64) {
            self.connections.remove(&id);
            if self.owner == Some(id) {
                self.owner = None;
            }
        }
    }

    /// Releases the registry slot (and the owner claim) on drop, whichever
    /// way the connection ends.
    struct ConnectionGuard {
        registry: Arc<Mutex<ConnectionRegistry>>,
        id: u64,
        role: AttachRole,
    }

    impl Drop for ConnectionGuard {
        fn drop(&mut self) {
            if let Ok(mut registry) = self.registry.lock() {
                registry.remove(self.id);
            }
        }
    }

    /// Removes the socket file when the server stops, however it stops.
    struct SocketFileGuard(PathBuf);

    impl Drop for SocketFileGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    /// Asks a running [`DaemonSocket::serve`] to stop.
    #[derive(Debug, Clone)]
    pub struct DaemonShutdownHandle(Arc<watch::Sender<bool>>);

    impl DaemonShutdownHandle {
        /// Idempotent; safe to call from any task or signal handler.
        pub fn trigger(&self) {
            self.0.send_replace(true);
        }
    }

    #[derive(Clone)]
    struct ConnectionContext {
        state: AppState,
        registry: Arc<Mutex<ConnectionRegistry>>,
        info: Arc<DaemonInfo>,
        shutdown: DaemonShutdownHandle,
    }

    /// A bound, not yet serving, daemon socket.
    pub struct DaemonSocket {
        listener: UnixListener,
        path: PathBuf,
        state: AppState,
        shutdown: Arc<watch::Sender<bool>>,
    }

    impl DaemonSocket {
        /// Where clients connect.
        #[must_use]
        pub fn local_path(&self) -> &Path {
            &self.path
        }

        /// A handle that stops [`Self::serve`] from outside (signals, tests).
        #[must_use]
        pub fn shutdown_handle(&self) -> DaemonShutdownHandle {
            DaemonShutdownHandle(Arc::clone(&self.shutdown))
        }

        /// Accept clients until the owner sends `shutdown` or the handle is
        /// triggered. Removes the socket file on the way out.
        pub async fn serve(self) -> Result<(), DaemonSocketError> {
            let Self {
                listener,
                path,
                state,
                shutdown,
            } = self;
            let _socket_file = SocketFileGuard(path.clone());
            let uid = std::fs::metadata(&path)
                .map_err(|source| DaemonSocketError::Io {
                    context: "failed to stat the daemon socket",
                    path: path.clone(),
                    source,
                })?
                .uid();
            let context = ConnectionContext {
                state,
                registry: Arc::new(Mutex::new(ConnectionRegistry::default())),
                info: Arc::new(DaemonInfo {
                    pid: std::process::id(),
                    version: env!("CARGO_PKG_VERSION"),
                    uid,
                    socket_path: path.clone(),
                    started_at: Instant::now(),
                }),
                shutdown: DaemonShutdownHandle(Arc::clone(&shutdown)),
            };
            let mut shutdown_rx = shutdown.subscribe();
            let mut connections = JoinSet::new();

            loop {
                if *shutdown_rx.borrow() {
                    break;
                }
                tokio::select! {
                    accepted = listener.accept() => match accepted {
                        Ok((stream, _)) => {
                            connections.spawn(handle_connection(context.clone(), stream));
                        }
                        Err(err) => {
                            tracing::warn!(error = %err, "daemon socket accept failed");
                            tokio::time::sleep(Duration::from_millis(50)).await;
                        }
                    },
                    changed = shutdown_rx.changed() => {
                        if changed.is_err() || *shutdown_rx.borrow() {
                            break;
                        }
                    }
                }
            }

            // The owner's `shutdown` reply was flushed before its loop
            // returned, so aborting what is left loses nothing a client
            // still needs.
            connections.shutdown().await;
            Ok(())
        }
    }

    /// Resolve the path, clear a stale socket, bind with `0600`, and build
    /// the shared app state. Does not accept anything until
    /// [`DaemonSocket::serve`].
    pub async fn bind_daemon_socket(
        options: DaemonSocketOptions,
    ) -> Result<DaemonSocket, DaemonSocketError> {
        let path = resolve_socket_path(&SocketPathInputs::from_environment(options.socket_path)?)?;
        ensure_private_parent_dir(&path)?;
        clear_stale_socket(&path).await?;

        let listener = UnixListener::bind(&path).map_err(|source| DaemonSocketError::Io {
            context: "failed to bind the daemon socket",
            path: path.clone(),
            source,
        })?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).map_err(
            |source| DaemonSocketError::Io {
                context: "failed to restrict daemon socket permissions to 0600",
                path: path.clone(),
                source,
            },
        )?;

        let state = build_state_with_transport(options.config_path, None, AppTransport::Socket)
            .map_err(DaemonSocketError::State)?;
        let (shutdown, _) = watch::channel(false);
        Ok(DaemonSocket {
            listener,
            path,
            state,
            shutdown: Arc::new(shutdown),
        })
    }

    /// Create the socket's directory as `0700` when it does not exist. An
    /// existing directory is left as the operator made it.
    fn ensure_private_parent_dir(path: &Path) -> Result<(), DaemonSocketError> {
        let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        else {
            return Ok(());
        };
        if parent.is_dir() {
            return Ok(());
        }
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)
            .map_err(|source| DaemonSocketError::Io {
                context: "failed to create the daemon runtime directory",
                path: parent.to_path_buf(),
                source,
            })
    }

    /// Remove a socket file nobody answers on; refuse to touch anything else.
    async fn clear_stale_socket(path: &Path) -> Result<(), DaemonSocketError> {
        let metadata = match std::fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(source) => {
                return Err(DaemonSocketError::Io {
                    context: "failed to inspect the daemon socket path",
                    path: path.to_path_buf(),
                    source,
                });
            }
        };
        if !metadata.file_type().is_socket() {
            return Err(DaemonSocketError::NotASocket {
                path: path.to_path_buf(),
            });
        }
        match tokio::time::timeout(PROBE_TIMEOUT, UnixStream::connect(path)).await {
            Ok(Ok(_live)) => Err(DaemonSocketError::AlreadyRunning {
                path: path.to_path_buf(),
            }),
            Ok(Err(_refused)) => {
                std::fs::remove_file(path).map_err(|source| DaemonSocketError::Io {
                    context: "failed to remove a stale daemon socket",
                    path: path.to_path_buf(),
                    source,
                })
            }
            Err(_elapsed) => Err(DaemonSocketError::ProbeTimedOut {
                path: path.to_path_buf(),
            }),
        }
    }

    async fn handle_connection(context: ConnectionContext, stream: UnixStream) {
        match stream.peer_cred() {
            Ok(cred) if cred.uid() == context.info.uid => {}
            Ok(cred) => {
                tracing::warn!(
                    peer_uid = cred.uid(),
                    daemon_uid = context.info.uid,
                    "rejected daemon socket peer: uid mismatch"
                );
                return;
            }
            Err(err) => {
                tracing::warn!(error = %err, "rejected daemon socket peer: no peer credentials");
                return;
            }
        }

        let (rx, mut writer) = stream.into_split();
        let mut lines = BufReader::new(rx).lines();
        let guard = match handshake(&context, &mut lines, &mut writer).await {
            Ok(Some(guard)) => guard,
            Ok(None) => return,
            Err(err) => {
                tracing::debug!(error = %err, "daemon socket handshake aborted");
                return;
            }
        };

        let policy = StdioLoopPolicy {
            transport: AppTransport::Socket,
            shutdown: match guard.role {
                AttachRole::Owner => ShutdownAuthority::Granted,
                AttachRole::Attached => ShutdownAuthority::Denied,
            },
        };
        // The guard moves into the loop so the claim is released when the
        // socket closes, not when a long-running turn finally returns.
        let exit = run_stdio_loop(&context.state, lines, writer, policy, Some(guard)).await;
        match exit {
            Ok(StdioLoopExit::Shutdown) => context.shutdown.trigger(),
            Ok(StdioLoopExit::InputClosed) => {}
            Err(err) => tracing::debug!(error = %err, "daemon socket connection ended with error"),
        }
    }

    /// Serve `healthz` and wait for `daemon/attach`; everything else is
    /// refused with `attach_required` until the client attaches.
    async fn handshake<R, W>(
        context: &ConnectionContext,
        lines: &mut Lines<R>,
        writer: &mut W,
    ) -> Result<Option<ConnectionGuard>>
    where
        R: AsyncBufRead + Unpin,
        W: AsyncWrite + Unpin,
    {
        loop {
            let Some(line) = lines.next_line().await? else {
                return Ok(None);
            };
            let request = match parse_stdio_line(&line) {
                ParsedStdioLine::Blank => continue,
                ParsedStdioLine::Rejected(response) => {
                    write_stdio_line(writer, &response).await?;
                    continue;
                }
                ParsedStdioLine::Request(request) => request,
            };
            let id = request.id.clone();
            match request.method.as_str() {
                "healthz" | "app/healthz" => {
                    let response = match dispatch_stdio_request_with_writer(
                        &context.state,
                        writer,
                        &request.method,
                        request.params,
                        AppTransport::Socket,
                    )
                    .await
                    {
                        Ok(dispatch) => jsonrpc_result(id, dispatch.result),
                        Err(err) => jsonrpc_error(id, err),
                    };
                    write_stdio_line(writer, &response).await?;
                }
                ATTACH_METHOD => match attach(context, request.params) {
                    Ok((result, guard)) => {
                        write_stdio_line(writer, &jsonrpc_result(id, result)).await?;
                        return Ok(Some(guard));
                    }
                    Err(err) => write_stdio_line(writer, &jsonrpc_error(id, err)).await?,
                },
                other => {
                    write_stdio_line(
                        writer,
                        &jsonrpc_error(id, JsonRpcError::attach_required(other)),
                    )
                    .await?;
                }
            }
        }
    }

    fn attach(
        context: &ConnectionContext,
        params: Value,
    ) -> Result<(Value, ConnectionGuard), JsonRpcError> {
        let params: AttachParams = parse_params(params_or_object(params))?;
        if params.client.name.trim().is_empty() {
            return Err(JsonRpcError::invalid_params(
                "client.name must not be empty",
            ));
        }
        if let Some(expected) = params.expect_daemon_version.as_deref()
            && expected != context.info.version
        {
            return Err(JsonRpcError::daemon_version_skew(
                expected,
                context.info.version,
            ));
        }

        let mut registry = context
            .registry
            .lock()
            .map_err(|_| JsonRpcError::internal("daemon connection registry poisoned"))?;
        let id = registry.register(params.client.clone());
        let role = match params.mode {
            AttachMode::Attach => AttachRole::Attached,
            AttachMode::Claim => match registry.claim(id) {
                Ok(()) => AttachRole::Owner,
                Err(owner) => {
                    registry.remove(id);
                    let owner = serde_json::to_value(owner)
                        .map_err(|err| JsonRpcError::internal(err.to_string()))?;
                    return Err(JsonRpcError::daemon_already_claimed(&owner));
                }
            },
        };
        let owner = registry.owner();
        let connections = registry.connections.len();
        drop(registry);

        let info = &context.info;
        let result = json!({
            "attached": true,
            "connection_id": id,
            "role": role,
            "transport": AppTransport::Socket.label(),
            "daemon": {
                "service": legacy_deepseek_compat::SERVICE_NAME,
                "pid": info.pid,
                "version": info.version,
                "socket_path": info.socket_path.display().to_string(),
                "uptime_ms": u64::try_from(info.started_at.elapsed().as_millis()).unwrap_or(u64::MAX),
            },
            "owner": owner,
            "connections": connections,
        });
        Ok((
            result,
            ConnectionGuard {
                registry: Arc::clone(&context.registry),
                id,
                role,
            },
        ))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn client(name: &str) -> ClientIdentity {
            ClientIdentity {
                name: name.to_string(),
                version: None,
                pid: None,
            }
        }

        #[test]
        fn registry_claim_is_exclusive_until_the_owner_leaves() {
            let mut registry = ConnectionRegistry::default();
            let first = registry.register(client("desktop-a"));
            let second = registry.register(client("desktop-b"));

            assert!(registry.claim(first).is_ok());
            assert_eq!(registry.claim(second), Err(client("desktop-a")));
            assert!(
                registry.claim(first).is_ok(),
                "re-claim by the owner is idempotent"
            );

            registry.remove(first);
            assert_eq!(registry.owner(), None);
            assert!(registry.claim(second).is_ok());
            assert_eq!(registry.owner(), Some(client("desktop-b")));
        }

        #[test]
        fn removing_a_guest_keeps_the_owner() {
            let mut registry = ConnectionRegistry::default();
            let owner = registry.register(client("owner"));
            let guest = registry.register(client("guest"));
            registry.claim(owner).expect("claim");
            registry.remove(guest);
            assert_eq!(registry.owner(), Some(client("owner")));
            assert_eq!(registry.connections.len(), 1);
        }
    }
}

#[cfg(not(unix))]
mod platform {
    use std::path::Path;

    use super::{DaemonSocketError, DaemonSocketOptions, unsupported_platform};

    /// Placeholder until the Windows named pipe lands; cannot be constructed.
    pub struct DaemonSocket {
        never: std::convert::Infallible,
    }

    /// Placeholder handle for the unsupported platform.
    #[derive(Debug, Clone)]
    pub struct DaemonShutdownHandle(());

    impl DaemonShutdownHandle {
        pub fn trigger(&self) {}
    }

    impl DaemonSocket {
        #[must_use]
        pub fn local_path(&self) -> &Path {
            match self.never {}
        }

        #[must_use]
        pub fn shutdown_handle(&self) -> DaemonShutdownHandle {
            match self.never {}
        }

        pub async fn serve(self) -> Result<(), DaemonSocketError> {
            match self.never {}
        }
    }

    /// Always [`DaemonSocketError::UnsupportedPlatform`] here.
    pub async fn bind_daemon_socket(
        _options: DaemonSocketOptions,
    ) -> Result<DaemonSocket, DaemonSocketError> {
        Err(unsupported_platform())
    }
}

pub use platform::{DaemonShutdownHandle, DaemonSocket, bind_daemon_socket};

/// `codewhale app-server --socket`: bind, announce, serve until the owner's
/// `shutdown` or a termination signal.
pub async fn run_daemon_socket(options: DaemonSocketOptions) -> anyhow::Result<()> {
    let daemon = bind_daemon_socket(options).await?;
    let path: &Path = daemon.local_path();
    tracing::info!(path = %path.display(), "codewhale daemon listening on unix socket");
    eprintln!("codewhale daemon: listening on {}", path.display());

    let handle = daemon.shutdown_handle();
    tokio::spawn(async move {
        crate::shutdown_signal().await;
        handle.trigger();
    });
    daemon.serve().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs() -> SocketPathInputs {
        SocketPathInputs {
            explicit: None,
            codewhale_home_override: None,
            xdg_runtime_dir: None,
            user_home: Some(PathBuf::from("/home/whale")),
            macos: false,
        }
    }

    #[test]
    fn explicit_path_wins() {
        let resolved = resolve_socket_path(&SocketPathInputs {
            explicit: Some(PathBuf::from("/tmp/x.sock")),
            codewhale_home_override: Some(PathBuf::from("/iso")),
            xdg_runtime_dir: Some(PathBuf::from("/run/user/1000")),
            ..inputs()
        })
        .expect("resolve");
        assert_eq!(resolved, PathBuf::from("/tmp/x.sock"));
    }

    #[test]
    fn explicit_codewhale_home_isolates_the_daemon() {
        let resolved = resolve_socket_path(&SocketPathInputs {
            codewhale_home_override: Some(PathBuf::from("/iso/home")),
            xdg_runtime_dir: Some(PathBuf::from("/run/user/1000")),
            ..inputs()
        })
        .expect("resolve");
        assert_eq!(resolved, PathBuf::from("/iso/home/run/daemon.sock"));
    }

    #[test]
    fn xdg_runtime_dir_beats_home_layouts() {
        let resolved = resolve_socket_path(&SocketPathInputs {
            xdg_runtime_dir: Some(PathBuf::from("/run/user/1000")),
            macos: true,
            ..inputs()
        })
        .expect("resolve");
        assert_eq!(
            resolved,
            PathBuf::from("/run/user/1000/codewhale/daemon.sock")
        );
    }

    #[test]
    fn macos_defaults_to_application_support() {
        let resolved = resolve_socket_path(&SocketPathInputs {
            macos: true,
            user_home: Some(PathBuf::from("/Users/whale")),
            ..inputs()
        })
        .expect("resolve");
        assert_eq!(
            resolved,
            PathBuf::from("/Users/whale/Library/Application Support/codewhale/daemon.sock")
        );
    }

    #[test]
    fn linux_defaults_to_dot_codewhale_run() {
        let resolved = resolve_socket_path(&inputs()).expect("resolve");
        assert_eq!(
            resolved,
            PathBuf::from("/home/whale/.codewhale/run/daemon.sock")
        );
    }

    #[test]
    fn no_home_is_a_typed_error() {
        let err = resolve_socket_path(&SocketPathInputs {
            user_home: None,
            ..inputs()
        })
        .expect_err("must fail");
        assert!(
            matches!(err, DaemonSocketError::RuntimeDirUnavailable),
            "{err}"
        );
    }

    #[test]
    fn over_long_paths_are_refused_before_bind() {
        let long = PathBuf::from(format!(
            "/{}/daemon.sock",
            "d".repeat(MAX_SOCKET_PATH_BYTES)
        ));
        let err = resolve_socket_path(&SocketPathInputs {
            explicit: Some(long.clone()),
            ..inputs()
        })
        .expect_err("must fail");
        match err {
            DaemonSocketError::PathTooLong { path, len, max } => {
                assert_eq!(path, long);
                assert!(len > max);
                assert_eq!(max, MAX_SOCKET_PATH_BYTES);
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn unsupported_platform_error_names_the_named_pipe() {
        let err = unsupported_platform();
        let text = err.to_string();
        assert!(text.contains(WINDOWS_NAMED_PIPE), "{text}");
        assert!(text.contains("not implemented"), "{text}");
    }

    #[test]
    fn attach_mode_defaults_to_guest() {
        let params: AttachParams =
            serde_json::from_value(serde_json::json!({ "client": { "name": "x" } }))
                .expect("parse");
        assert_eq!(params.mode, AttachMode::Attach);
        assert_eq!(params.expect_daemon_version, None);
    }
}
