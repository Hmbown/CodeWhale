//! Naming and cleanup policy for Codewhale-owned xAI OAuth generations.
//!
//! Config stores only a validated basename. Callers can therefore never turn
//! the generation pointer into an arbitrary path read or deletion primitive.

#[cfg(unix)]
use std::ffi::CString;
use std::fs::{self, File};
use std::io::{Read as _, Write as _};
use std::path::{Component, Path, PathBuf};
#[cfg(not(windows))]
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
#[cfg(not(windows))]
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};

pub const XAI_OAUTH_GENERATION_PREFIX: &str = "xai-auth-";
pub const XAI_OAUTH_GENERATION_SUFFIX: &str = ".json";
pub const LEGACY_XAI_OAUTH_FILE_NAME: &str = "xai-auth.json";
pub const CHATGPT_OAUTH_GENERATION_PREFIX: &str = "chatgpt-auth-";
pub const CHATGPT_OAUTH_GENERATION_SUFFIX: &str = ".json";
pub const LEGACY_CHATGPT_OAUTH_FILE_NAME: &str = "chatgpt-oauth.json";
const XAI_OAUTH_LIFECYCLE_LOCK_FILE_NAME: &str = ".xai-oauth.lock";
const XAI_OAUTH_FILE_LIMIT: u64 = 1024 * 1024;

/// Stable handle to Codewhale's private xAI OAuth directory.
///
/// The lexical `$CODEWHALE_HOME/credentials` boundary is retained verbatim.
/// Unix opens every component relative to the preceding directory with
/// `O_NOFOLLOW`; Windows keeps non-delete-shared handles to every component and
/// rejects reparse points. Holding this value therefore pins the directory
/// identity for the duration of one lifecycle operation.
#[derive(Debug)]
pub struct XaiOAuthCredentialStore {
    directory: PathBuf,
    #[cfg(unix)]
    directory_handle: File,
    #[cfg(windows)]
    _component_handles: Vec<File>,
}

/// Files retired from an active xAI OAuth epoch before a mode switch commits.
///
/// Unix hides original names behind private tombstones immediately. Windows
/// relies on the lifecycle lock, then deletes exact handles after commit. A
/// failed config mutation restores or retains the prior files; a successful
/// mutation removes them.
#[derive(Debug)]
pub struct XaiOAuthRevocation {
    retired: Vec<(String, String)>,
}

#[must_use]
pub fn is_valid_xai_oauth_generation(value: &str) -> bool {
    is_valid_owned_oauth_generation(
        value,
        XAI_OAUTH_GENERATION_PREFIX,
        XAI_OAUTH_GENERATION_SUFFIX,
    )
}

pub fn validate_xai_oauth_generation(value: &str) -> Result<&str> {
    if !is_valid_xai_oauth_generation(value) {
        bail!(
            "invalid Codewhale-owned xAI OAuth generation; expected xai-auth-<32 lowercase hex>.json"
        );
    }
    Ok(value)
}

#[must_use]
pub fn is_valid_chatgpt_oauth_generation(value: &str) -> bool {
    is_valid_owned_oauth_generation(
        value,
        CHATGPT_OAUTH_GENERATION_PREFIX,
        CHATGPT_OAUTH_GENERATION_SUFFIX,
    )
}

pub fn validate_chatgpt_oauth_generation(value: &str) -> Result<&str> {
    if !is_valid_chatgpt_oauth_generation(value) {
        bail!(
            "invalid Codewhale-owned ChatGPT OAuth generation; expected chatgpt-auth-<32 lowercase hex>.json"
        );
    }
    Ok(value)
}

fn is_valid_owned_oauth_generation(value: &str, prefix: &str, suffix: &str) -> bool {
    let path = Path::new(value);
    if path.components().count() != 1
        || !matches!(path.components().next(), Some(Component::Normal(_)))
        || path.file_name().and_then(|name| name.to_str()) != Some(value)
    {
        return false;
    }
    let Some(id) = value
        .strip_prefix(prefix)
        .and_then(|value| value.strip_suffix(suffix))
    else {
        return false;
    };
    id.len() == 32
        && id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub fn xai_oauth_credentials_dir() -> Result<PathBuf> {
    lexical_absolute_path(&crate::codewhale_home()?.join("credentials"))
}

/// Make an owned path absolute without resolving any filesystem component.
/// Canonicalization is deliberately forbidden here: following an existing
/// `credentials` symlink would erase the lexical Codewhale-owned boundary and
/// turn an external directory into an apparently valid destination.
fn lexical_absolute_path(path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .context("resolving the Codewhale credentials directory")?
            .join(path)
    };
    if absolute
        .components()
        .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        bail!(
            "Codewhale credentials directory must be lexically normalized: {}",
            crate::quote_os_path(&absolute)
        );
    }
    Ok(absolute)
}

pub fn xai_oauth_generation_path(generation: &str) -> Result<PathBuf> {
    Ok(xai_oauth_credentials_dir()?.join(validate_xai_oauth_generation(generation)?))
}

pub fn legacy_xai_oauth_path() -> Result<PathBuf> {
    Ok(xai_oauth_credentials_dir()?.join(LEGACY_XAI_OAUTH_FILE_NAME))
}

pub fn chatgpt_oauth_generation_path(generation: &str) -> Result<PathBuf> {
    Ok(xai_oauth_credentials_dir()?.join(validate_chatgpt_oauth_generation(generation)?))
}

pub fn legacy_chatgpt_oauth_path() -> Result<PathBuf> {
    Ok(xai_oauth_credentials_dir()?.join(LEGACY_CHATGPT_OAUTH_FILE_NAME))
}

/// Serialize every Codewhale-owned xAI OAuth lifecycle mutation across threads
/// and processes while pinning the lexical credentials directory.
///
/// Lock order is always xAI lifecycle first, then config document. Callers must
/// not invoke this function recursively.
pub fn with_xai_oauth_lifecycle_lock<T>(
    operation: impl FnOnce(&XaiOAuthCredentialStore) -> Result<T>,
) -> Result<T> {
    static PROCESS_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let _process_guard = PROCESS_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| anyhow::anyhow!("xAI OAuth lifecycle lock was poisoned"))?;
    let store = XaiOAuthCredentialStore::open()?;
    let lock_file = store.open_lock_file()?;
    let mut lock = fd_lock::RwLock::new(lock_file);
    let _guard = lock.write().with_context(|| {
        format!(
            "failed to acquire xAI OAuth lifecycle lock in {}",
            crate::quote_os_path(store.directory())
        )
    })?;
    operation(&store)
}

/// Run an authority mode switch while the prior owned OAuth epoch is hidden
/// from concurrent Codewhale readers. A failed authority mutation restores the
/// old files; a successful mutation permanently removes them.
pub fn with_xai_oauth_revocation_transaction<T>(
    operation: impl FnOnce() -> Result<T>,
) -> Result<T> {
    with_xai_oauth_lifecycle_lock(|store| {
        let revocation = store.stage_revocation()?;
        match operation() {
            Ok(value) => {
                revocation.commit(store).context(
                    "xAI OAuth authority changed, but retired owned credentials could not be removed",
                )?;
                Ok(value)
            }
            Err(error) => {
                if let Err(rollback) = revocation.rollback(store) {
                    return Err(error).context(format!(
                        "also failed to restore the prior xAI OAuth epoch: {rollback:#}"
                    ));
                }
                Err(error)
            }
        }
    })
}

impl XaiOAuthCredentialStore {
    fn open() -> Result<Self> {
        let directory = xai_oauth_credentials_dir()?;
        open_owned_credentials_directory(&directory)
    }

    #[must_use]
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    pub fn path_for(&self, name: &str) -> Result<PathBuf> {
        validate_owned_auth_name(name)?;
        Ok(self.directory.join(name))
    }

    pub fn read_to_string(&self, name: &str) -> Result<Option<String>> {
        validate_owned_auth_name(name)?;
        let Some(mut file) = self.open_owned_file_for_read(name)? else {
            return Ok(None);
        };
        let metadata = validate_owned_file_handle(&file, &self.directory.join(name))?;
        if metadata.len() > XAI_OAUTH_FILE_LIMIT {
            bail!(
                "Codewhale-owned xAI OAuth file {} exceeds the {} byte limit",
                crate::quote_os_path(&self.directory.join(name)),
                XAI_OAUTH_FILE_LIMIT
            );
        }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        (&mut file)
            .take(XAI_OAUTH_FILE_LIMIT + 1)
            .read_to_end(&mut bytes)
            .with_context(|| {
                format!(
                    "reading Codewhale-owned xAI OAuth file {}",
                    crate::quote_os_path(&self.directory.join(name))
                )
            })?;
        if bytes.len() as u64 > XAI_OAUTH_FILE_LIMIT {
            bail!(
                "Codewhale-owned xAI OAuth file {} exceeds the {} byte limit",
                crate::quote_os_path(&self.directory.join(name)),
                XAI_OAUTH_FILE_LIMIT
            );
        }
        String::from_utf8(bytes).map(Some).map_err(|_| {
            anyhow::anyhow!(
                "Codewhale-owned xAI OAuth file {} is not valid UTF-8",
                crate::quote_os_path(&self.directory.join(name))
            )
        })
    }

    pub fn write(&self, name: &str, bytes: &[u8], allow_replace: bool) -> Result<()> {
        validate_owned_auth_name(name)?;
        anyhow::ensure!(
            bytes.len() as u64 <= XAI_OAUTH_FILE_LIMIT,
            "refusing oversized xAI OAuth credential payload"
        );
        self.write_owned_file(name, bytes, allow_replace)
    }

    pub fn remove(&self, name: &str) -> Result<bool> {
        validate_owned_auth_name(name)?;
        self.remove_raw(name)
    }

    pub fn clear_all(&self) -> Result<usize> {
        let mut removed = 0;
        for name in self.owned_auth_names()? {
            if self.remove(&name)? {
                removed += 1;
            }
        }
        Ok(removed)
    }

    pub fn clear_chatgpt(&self) -> Result<usize> {
        let mut removed = 0;
        for name in chatgpt_auth_names_in_store(self)? {
            if self.remove(&name)? {
                removed += 1;
            }
        }
        Ok(removed)
    }

    /// Stage every active owned credential before a config mode switch. The
    /// generation basename is the OAuth epoch; the lifecycle lock prevents a
    /// stale Codewhale reader from using it while authority changes.
    pub fn stage_revocation(&self) -> Result<XaiOAuthRevocation> {
        #[cfg(windows)]
        {
            // Every Codewhale reader/writer takes the lifecycle lock, so a
            // Windows mode switch can retain the exact active basenames until
            // the config commit succeeds. `commit` then opens each leaf with
            // DELETE access and marks that exact handle for deletion. This
            // avoids path-based rename races and makes rollback a no-op.
            Ok(XaiOAuthRevocation {
                retired: self
                    .owned_auth_names()?
                    .into_iter()
                    .map(|name| (name, String::new()))
                    .collect(),
            })
        }

        #[cfg(not(windows))]
        {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let mut retired = Vec::new();
            for (index, name) in self.owned_auth_names()?.into_iter().enumerate() {
                let tombstone = format!(
                    ".xai-oauth-retired-{}-{nonce}-{}-{index}.tmp",
                    std::process::id(),
                    COUNTER.fetch_add(1, Ordering::Relaxed)
                );
                if let Err(error) = self.rename_raw(&name, &tombstone) {
                    let rollback = XaiOAuthRevocation { retired };
                    if let Err(rollback_error) = rollback.rollback(self) {
                        return Err(error).context(format!(
                        "also failed to restore previously retired xAI OAuth files: {rollback_error:#}"
                    ));
                    }
                    return Err(error);
                }
                retired.push((name, tombstone));
            }
            Ok(XaiOAuthRevocation { retired })
        }
    }

    fn owned_auth_names(&self) -> Result<Vec<String>> {
        owned_auth_names_in_store(self)
    }

    fn open_lock_file(&self) -> Result<File> {
        self.open_internal_file(XAI_OAUTH_LIFECYCLE_LOCK_FILE_NAME)
    }
}

#[cfg(unix)]
fn owned_auth_names_in_store(store: &XaiOAuthCredentialStore) -> Result<Vec<String>> {
    use std::ffi::CStr;
    use std::os::fd::AsRawFd as _;

    // `fdopendir` consumes its descriptor, so enumerate through a duplicate of
    // the pinned directory handle. No pathname is resolved after the store is
    // opened, even if the lexical directory is renamed or replaced.
    let duplicated =
        unsafe { libc::fcntl(store.directory_handle.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
    if duplicated < 0 {
        return Err(std::io::Error::last_os_error())
            .context("duplicating Codewhale credentials directory handle");
    }
    // SAFETY: `duplicated` is an owned directory descriptor. `closedir` below
    // assumes ownership on the successful conversion.
    let stream = unsafe { libc::fdopendir(duplicated) };
    if stream.is_null() {
        let error = std::io::Error::last_os_error();
        // SAFETY: `fdopendir` failed and therefore did not consume the fd.
        unsafe { libc::close(duplicated) };
        return Err(error).context("enumerating Codewhale credentials directory");
    }
    let mut names = Vec::new();
    loop {
        // SAFETY: `stream` remains live until `closedir`; each returned entry
        // is valid until the next call and copied before then.
        let entry = unsafe { libc::readdir(stream) };
        if entry.is_null() {
            break;
        }
        // SAFETY: POSIX `dirent::d_name` is NUL terminated.
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
        let Ok(name) = name.to_str() else {
            continue;
        };
        if name == LEGACY_XAI_OAUTH_FILE_NAME || is_valid_xai_oauth_generation(name) {
            names.push(name.to_string());
        }
    }
    // SAFETY: `stream` is still owned and has not previously been closed.
    if unsafe { libc::closedir(stream) } != 0 {
        return Err(std::io::Error::last_os_error())
            .context("closing Codewhale credentials directory enumeration");
    }
    names.sort();
    Ok(names)
}

#[cfg(unix)]
fn chatgpt_auth_names_in_store(store: &XaiOAuthCredentialStore) -> Result<Vec<String>> {
    use std::ffi::CStr;
    use std::os::fd::AsRawFd as _;

    let duplicated =
        unsafe { libc::fcntl(store.directory_handle.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
    if duplicated < 0 {
        return Err(std::io::Error::last_os_error())
            .context("duplicating Codewhale credentials directory handle");
    }
    let stream = unsafe { libc::fdopendir(duplicated) };
    if stream.is_null() {
        let error = std::io::Error::last_os_error();
        unsafe { libc::close(duplicated) };
        return Err(error).context("enumerating Codewhale credentials directory");
    }
    let mut names = Vec::new();
    loop {
        let entry = unsafe { libc::readdir(stream) };
        if entry.is_null() {
            break;
        }
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
        let Ok(name) = name.to_str() else {
            continue;
        };
        if is_chatgpt_owned_auth_name(name) {
            names.push(name.to_string());
        }
    }
    if unsafe { libc::closedir(stream) } != 0 {
        return Err(std::io::Error::last_os_error())
            .context("closing Codewhale credentials directory enumeration");
    }
    names.sort();
    Ok(names)
}

#[cfg(not(unix))]
fn owned_auth_names_in_store(store: &XaiOAuthCredentialStore) -> Result<Vec<String>> {
    let mut names = Vec::new();
    let entries = fs::read_dir(&store.directory).with_context(|| {
        format!(
            "failed to inspect Codewhale credentials directory {}",
            crate::quote_os_path(&store.directory)
        )
    })?;
    for entry in entries {
        let entry = entry.with_context(|| {
            format!(
                "failed to inspect Codewhale credentials directory {}",
                crate::quote_os_path(&store.directory)
            )
        })?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name == LEGACY_XAI_OAUTH_FILE_NAME || is_valid_xai_oauth_generation(name) {
            names.push(name.to_string());
        }
    }
    names.sort();
    Ok(names)
}

#[cfg(not(unix))]
fn chatgpt_auth_names_in_store(store: &XaiOAuthCredentialStore) -> Result<Vec<String>> {
    let mut names = Vec::new();
    let entries = fs::read_dir(&store.directory).with_context(|| {
        format!(
            "failed to inspect Codewhale credentials directory {}",
            crate::quote_os_path(&store.directory)
        )
    })?;
    for entry in entries {
        let entry = entry.with_context(|| {
            format!(
                "failed to inspect Codewhale credentials directory {}",
                crate::quote_os_path(&store.directory)
            )
        })?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if is_chatgpt_owned_auth_name(name) {
            names.push(name.to_string());
        }
    }
    names.sort();
    Ok(names)
}

impl XaiOAuthRevocation {
    /// Restore the old epoch after a config mutation fails. Restoration is
    /// fail-closed: an unexpected replacement at an original name is never
    /// overwritten.
    pub fn rollback(self, store: &XaiOAuthCredentialStore) -> Result<()> {
        #[cfg(windows)]
        {
            let _ = store;
            Ok(())
        }
        #[cfg(not(windows))]
        {
            let mut first_error = None;
            for (original, tombstone) in self.retired.into_iter().rev() {
                let result = store.rename_raw(&tombstone, &original).with_context(|| {
                    format!(
                        "restoring retired xAI OAuth file {}",
                        crate::quote_os_path(&store.directory.join(original))
                    )
                });
                if result.is_err() && first_error.is_none() {
                    first_error = result.err();
                }
            }
            if let Some(error) = first_error {
                return Err(error);
            }
            Ok(())
        }
    }

    /// Permanently remove retired bytes after the replacement config commits.
    pub fn commit(self, store: &XaiOAuthCredentialStore) -> Result<usize> {
        let mut removed = 0;
        for (_original, _tombstone) in self.retired {
            #[cfg(windows)]
            let target = _original;
            #[cfg(not(windows))]
            let target = _tombstone;
            if store.remove_raw(&target)? {
                removed += 1;
            }
        }
        Ok(removed)
    }
}

fn validate_owned_auth_name(name: &str) -> Result<()> {
    anyhow::ensure!(
        name == LEGACY_XAI_OAUTH_FILE_NAME
            || name == LEGACY_CHATGPT_OAUTH_FILE_NAME
            || is_valid_xai_oauth_generation(name)
            || is_valid_chatgpt_oauth_generation(name),
        "invalid Codewhale-owned OAuth basename"
    );
    Ok(())
}

fn is_chatgpt_owned_auth_name(name: &str) -> bool {
    name == LEGACY_CHATGPT_OAUTH_FILE_NAME || is_valid_chatgpt_oauth_generation(name)
}

fn validate_private_basename(name: &str) -> Result<()> {
    let path = Path::new(name);
    anyhow::ensure!(
        path.components().count() == 1
            && matches!(path.components().next(), Some(Component::Normal(_)))
            && path.file_name().and_then(|value| value.to_str()) == Some(name),
        "xAI OAuth private basename must be one UTF-8 path component"
    );
    Ok(())
}

#[cfg(unix)]
fn open_owned_credentials_directory(directory: &Path) -> Result<XaiOAuthCredentialStore> {
    use std::os::fd::FromRawFd as _;
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

    anyhow::ensure!(
        directory.is_absolute(),
        "xAI OAuth credentials directory must be absolute"
    );
    // SAFETY: the literal root path contains no interior NUL and the returned
    // descriptor is immediately owned by `File`.
    let root_fd = unsafe {
        libc::open(
            c"/".as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if root_fd < 0 {
        return Err(std::io::Error::last_os_error()).context("opening filesystem root");
    }
    // SAFETY: `root_fd` is a newly owned descriptor on the success path above.
    let mut current = unsafe { File::from_raw_fd(root_fd) };
    for component in directory.components() {
        let Component::Normal(name) = component else {
            if matches!(component, Component::RootDir) {
                continue;
            }
            bail!(
                "Codewhale credentials directory has an unsupported component: {}",
                crate::quote_os_path(directory)
            );
        };
        let name = cstring_from_os_str(name)?;
        let mut fd = unsafe {
            libc::openat(
                std::os::fd::AsRawFd::as_raw_fd(&current),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )
        };
        if fd < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::NotFound {
            // SAFETY: both the parent descriptor and component pointer remain
            // valid for this call. `mkdirat` cannot follow the missing leaf.
            let created = unsafe {
                libc::mkdirat(
                    std::os::fd::AsRawFd::as_raw_fd(&current),
                    name.as_ptr(),
                    0o700,
                )
            };
            if created != 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() != std::io::ErrorKind::AlreadyExists {
                    return Err(error).with_context(|| {
                        format!(
                            "creating a component of Codewhale credentials directory {}",
                            crate::quote_os_path(directory)
                        )
                    });
                }
            }
            // SAFETY: same stable parent/component arguments as above.
            fd = unsafe {
                libc::openat(
                    std::os::fd::AsRawFd::as_raw_fd(&current),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                )
            };
        }
        if fd < 0 {
            return Err(std::io::Error::last_os_error()).with_context(|| {
                format!(
                    "opening Codewhale credentials directory without following links: {}",
                    crate::quote_os_path(directory)
                )
            });
        }
        // SAFETY: `fd` is a newly owned descriptor on the success path above.
        current = unsafe { File::from_raw_fd(fd) };
    }
    let metadata = current.metadata().with_context(|| {
        format!(
            "inspecting Codewhale credentials directory {}",
            crate::quote_os_path(directory)
        )
    })?;
    anyhow::ensure!(
        metadata.is_dir(),
        "Codewhale credentials path must be a directory"
    );
    anyhow::ensure!(
        metadata.uid() == unsafe { libc::geteuid() },
        "Codewhale credentials directory must be owned by the current user"
    );
    current
        .set_permissions(fs::Permissions::from_mode(0o700))
        .with_context(|| {
            format!(
                "securing Codewhale credentials directory {}",
                crate::quote_os_path(directory)
            )
        })?;
    Ok(XaiOAuthCredentialStore {
        directory: directory.to_path_buf(),
        directory_handle: current,
    })
}

#[cfg(unix)]
fn cstring_from_os_str(value: &std::ffi::OsStr) -> Result<CString> {
    use std::os::unix::ffi::OsStrExt as _;
    CString::new(value.as_bytes()).context("owned xAI OAuth path contains an interior NUL")
}

#[cfg(unix)]
impl XaiOAuthCredentialStore {
    fn open_at(&self, name: &str, flags: i32, mode: libc::mode_t) -> Result<Option<File>> {
        use std::os::fd::AsRawFd as _;
        use std::os::fd::FromRawFd as _;

        validate_private_basename(name)?;
        let name = CString::new(name).context("xAI OAuth basename contains an interior NUL")?;
        // SAFETY: the stable directory descriptor and component pointer remain
        // valid for the call; a successful descriptor is transferred to File.
        let fd = unsafe {
            libc::openat(
                self.directory_handle.as_raw_fd(),
                name.as_ptr(),
                flags | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                libc::c_uint::from(mode),
            )
        };
        if fd < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::NotFound {
                return Ok(None);
            }
            return Err(error).with_context(|| {
                format!(
                    "opening Codewhale-owned xAI OAuth path {}",
                    crate::quote_os_path(&self.directory.join(name.to_string_lossy().as_ref()))
                )
            });
        }
        // SAFETY: `fd` is newly owned on the success path above.
        Ok(Some(unsafe { File::from_raw_fd(fd) }))
    }

    fn open_owned_file_for_read(&self, name: &str) -> Result<Option<File>> {
        self.open_at(name, libc::O_RDONLY, 0)
    }

    fn open_internal_file(&self, name: &str) -> Result<File> {
        use std::os::unix::fs::PermissionsExt as _;
        let file = self
            .open_at(name, libc::O_RDWR | libc::O_CREAT, 0o600)?
            .context("xAI OAuth lifecycle lock disappeared while opening")?;
        validate_owned_file_handle(&file, &self.directory.join(name))?;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        Ok(file)
    }

    fn write_owned_file(&self, name: &str, bytes: &[u8], allow_replace: bool) -> Result<()> {
        use std::os::fd::AsRawFd as _;
        use std::os::unix::fs::PermissionsExt as _;

        let temp_name = format!(
            ".xai-oauth-write-{}-{}.tmp",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        let mut temp = self
            .open_at(
                &temp_name,
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
                0o600,
            )?
            .context("creating private xAI OAuth temporary file")?;
        let result = (|| -> Result<()> {
            temp.write_all(bytes)
                .context("writing xAI OAuth temporary file")?;
            temp.flush().context("flushing xAI OAuth temporary file")?;
            temp.set_permissions(fs::Permissions::from_mode(0o600))?;
            temp.sync_all()
                .context("syncing xAI OAuth temporary file")?;

            let target =
                CString::new(name).context("xAI OAuth basename contains an interior NUL")?;
            let temporary =
                CString::new(temp_name.as_str()).context("temporary basename contains NUL")?;
            if allow_replace {
                if let Some(existing) = self.open_owned_file_for_read(name)? {
                    validate_owned_file_handle(&existing, &self.directory.join(name))?;
                }
                // SAFETY: both names are relative to the same stable directory
                // handle; rename is atomic and cannot escape that directory.
                if unsafe {
                    libc::renameat(
                        self.directory_handle.as_raw_fd(),
                        temporary.as_ptr(),
                        self.directory_handle.as_raw_fd(),
                        target.as_ptr(),
                    )
                } != 0
                {
                    return Err(std::io::Error::last_os_error())
                        .context("atomically replacing xAI OAuth credentials");
                }
            } else {
                // `linkat` installs the unique generation without clobbering an
                // existing path. The temporary link is removed immediately.
                // SAFETY: all descriptors/names remain valid for both calls.
                if unsafe {
                    libc::linkat(
                        self.directory_handle.as_raw_fd(),
                        temporary.as_ptr(),
                        self.directory_handle.as_raw_fd(),
                        target.as_ptr(),
                        0,
                    )
                } != 0
                {
                    return Err(std::io::Error::last_os_error())
                        .context("installing a new xAI OAuth generation without replacement");
                }
                if unsafe {
                    libc::unlinkat(self.directory_handle.as_raw_fd(), temporary.as_ptr(), 0)
                } != 0
                {
                    let error = std::io::Error::last_os_error();
                    // The target and staging name still reference the same
                    // inode. Remove the just-installed target so the generic
                    // error cleanup can safely retire the single remaining
                    // staging link instead of leaving an inert secret with
                    // link count two.
                    unsafe {
                        libc::unlinkat(self.directory_handle.as_raw_fd(), target.as_ptr(), 0)
                    };
                    return Err(error).context("removing xAI OAuth generation staging link");
                }
            }
            self.directory_handle
                .sync_all()
                .context("syncing Codewhale credentials directory")?;
            Ok(())
        })();
        drop(temp);
        if result.is_err() {
            let _ = self.remove_raw(&temp_name);
        }
        result
    }

    fn remove_raw(&self, name: &str) -> Result<bool> {
        use std::os::fd::AsRawFd as _;
        validate_private_basename(name)?;
        let Some(file) = self.open_owned_file_for_read(name)? else {
            return Ok(false);
        };
        validate_owned_file_handle(&file, &self.directory.join(name))?;
        drop(file);
        let name = CString::new(name).context("xAI OAuth basename contains an interior NUL")?;
        // SAFETY: the name is one component relative to the stable credentials
        // directory descriptor and was validated immediately above.
        if unsafe { libc::unlinkat(self.directory_handle.as_raw_fd(), name.as_ptr(), 0) } != 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::NotFound {
                return Ok(false);
            }
            return Err(error).context("removing Codewhale-owned xAI OAuth file");
        }
        Ok(true)
    }

    fn rename_raw(&self, from: &str, to: &str) -> Result<()> {
        use std::os::fd::AsRawFd as _;
        validate_private_basename(from)?;
        validate_private_basename(to)?;
        let source = self
            .open_owned_file_for_read(from)?
            .context("xAI OAuth source disappeared before retirement")?;
        validate_owned_file_handle(&source, &self.directory.join(from))?;
        anyhow::ensure!(
            self.open_owned_file_for_read(to)?.is_none(),
            "refusing to replace an existing xAI OAuth retirement path"
        );
        drop(source);
        let from = CString::new(from).context("xAI OAuth basename contains an interior NUL")?;
        let to = CString::new(to).context("xAI OAuth basename contains an interior NUL")?;
        // SAFETY: both names are one component relative to the same pinned
        // directory descriptor.
        if unsafe {
            libc::renameat(
                self.directory_handle.as_raw_fd(),
                from.as_ptr(),
                self.directory_handle.as_raw_fd(),
                to.as_ptr(),
            )
        } != 0
        {
            return Err(std::io::Error::last_os_error()).context("retiring xAI OAuth file");
        }
        Ok(())
    }
}

#[cfg(unix)]
fn validate_owned_file_handle(file: &File, path: &Path) -> Result<fs::Metadata> {
    use std::os::unix::fs::MetadataExt as _;
    let metadata = file.metadata().with_context(|| {
        format!(
            "inspecting Codewhale-owned xAI OAuth file {}",
            crate::quote_os_path(path)
        )
    })?;
    anyhow::ensure!(metadata.is_file(), "xAI OAuth path must be a regular file");
    anyhow::ensure!(
        metadata.uid() == unsafe { libc::geteuid() },
        "xAI OAuth file must be owned by the current user"
    );
    anyhow::ensure!(
        metadata.nlink() == 1,
        "xAI OAuth file must not have multiple filesystem links"
    );
    Ok(metadata)
}

#[cfg(windows)]
fn open_owned_credentials_directory(directory: &Path) -> Result<XaiOAuthCredentialStore> {
    use std::os::windows::fs::OpenOptionsExt as _;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_READ,
        FILE_SHARE_READ, FILE_SHARE_WRITE, WRITE_DAC, WRITE_OWNER,
    };

    anyhow::ensure!(
        directory.is_absolute(),
        "xAI OAuth credentials directory must be absolute"
    );
    let mut current = PathBuf::new();
    let mut handles = Vec::new();
    for component in directory.components() {
        match component {
            Component::Prefix(prefix) => current.push(prefix.as_os_str()),
            Component::RootDir => current.push(Path::new(r"\")),
            Component::Normal(name) => {
                current.push(name);
                match fs::create_dir(&current) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(error) => {
                        return Err(error).with_context(|| {
                            format!(
                                "creating a component of Codewhale credentials directory {}",
                                crate::quote_os_path(directory)
                            )
                        });
                    }
                }
                let mut options = fs::OpenOptions::new();
                options
                    .read(true)
                    .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
                    .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT);
                let handle = options.open(&current).with_context(|| {
                    format!(
                        "opening Codewhale credentials directory component {}",
                        crate::quote_os_path(&current)
                    )
                })?;
                validate_windows_handle_path(&handle, &current, true)?;
                handles.push(handle);
            }
            Component::CurDir | Component::ParentDir => bail!(
                "Codewhale credentials directory must be lexically normalized: {}",
                crate::quote_os_path(directory)
            ),
        }
    }
    anyhow::ensure!(
        !handles.is_empty(),
        "Codewhale credentials directory cannot be a volume root"
    );
    let mut secure_options = fs::OpenOptions::new();
    secure_options
        .access_mode(FILE_GENERIC_READ | WRITE_DAC | WRITE_OWNER)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT);
    let final_directory = secure_options.open(directory).with_context(|| {
        format!(
            "opening Codewhale credentials directory for owner-only security: {}",
            crate::quote_os_path(directory)
        )
    })?;
    validate_windows_handle_path(&final_directory, directory, true)?;
    secure_windows_owner_only_handle(&final_directory, true)
        .context("securing Codewhale credentials directory for the current user")?;
    verify_windows_owner_only_handle(&final_directory)
        .context("verifying Codewhale credentials directory ownership")?;
    handles.push(final_directory);
    Ok(XaiOAuthCredentialStore {
        directory: directory.to_path_buf(),
        _component_handles: handles,
    })
}

#[cfg(windows)]
impl XaiOAuthCredentialStore {
    fn open_windows_file(&self, name: &str, read: bool, write: bool) -> Result<Option<File>> {
        use std::os::windows::fs::OpenOptionsExt as _;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };

        validate_private_basename(name)?;
        let path = self.directory.join(name);
        let mut options = fs::OpenOptions::new();
        options
            .read(read)
            .write(write)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        match options.open(&path) {
            Ok(file) => {
                validate_owned_file_handle(&file, &path)?;
                Ok(Some(file))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error).with_context(|| {
                format!(
                    "opening Codewhale-owned xAI OAuth path {}",
                    crate::quote_os_path(&path)
                )
            }),
        }
    }

    fn open_owned_file_for_read(&self, name: &str) -> Result<Option<File>> {
        self.open_windows_file(name, true, false)
    }

    fn open_internal_file(&self, name: &str) -> Result<File> {
        use std::os::windows::fs::OpenOptionsExt as _;
        use windows_sys::Win32::Storage::FileSystem::{
            DELETE, FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_READ, FILE_GENERIC_WRITE,
            FILE_SHARE_READ, FILE_SHARE_WRITE, WRITE_DAC, WRITE_OWNER,
        };

        validate_private_basename(name)?;
        let path = self.directory.join(name);
        for _ in 0..8 {
            if let Some(existing) = self.open_windows_file(name, true, true)? {
                return Ok(existing);
            }

            let mut options = fs::OpenOptions::new();
            options
                // `access_mode` supplies the exact Win32 access mask below,
                // while Rust still requires the portable write intent to be
                // set before it permits `create_new`.
                .write(true)
                .access_mode(
                    FILE_GENERIC_READ | FILE_GENERIC_WRITE | WRITE_DAC | WRITE_OWNER | DELETE,
                )
                .create_new(true)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
            let file = match options.open(&path) {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!(
                            "creating Codewhale-owned xAI OAuth lifecycle lock {}",
                            crate::quote_os_path(&path)
                        )
                    });
                }
            };
            let secured = (|| -> Result<()> {
                validate_windows_file_shape(&file, &path)?;
                secure_windows_owner_only_handle(&file, false)
                    .context("securing a new xAI OAuth lifecycle lock")?;
                validate_owned_file_handle(&file, &path)?;
                Ok(())
            })();
            if let Err(error) = secured {
                let cleanup = mark_windows_file_handle_for_deletion(&file);
                return match cleanup {
                    Ok(()) => Err(error),
                    Err(cleanup) => Err(error).context(format!(
                        "also failed to delete the empty lifecycle lock: {cleanup:#}"
                    )),
                };
            }
            return Ok(file);
        }
        bail!("xAI OAuth lifecycle lock changed repeatedly while opening")
    }

    fn write_owned_file(&self, name: &str, bytes: &[u8], allow_replace: bool) -> Result<()> {
        let path = self.directory.join(name);
        if let Some(existing) = self.open_owned_file_for_read(name)? {
            anyhow::ensure!(
                allow_replace,
                "refusing to replace an existing xAI OAuth generation"
            );
            drop(existing);
        }
        let mut temporary = tempfile::NamedTempFile::new_in(&self.directory)
            .context("creating private xAI OAuth temporary file")?;
        let temporary_path = temporary.path().to_path_buf();
        let security_handle =
            reopen_windows_file_for_owner_security(temporary.as_file(), &temporary_path)?;
        secure_windows_owner_only_handle(&security_handle, false)
            .context("securing a new xAI OAuth temporary file before writing credentials")?;
        validate_owned_file_handle(&security_handle, &temporary_path)
            .context("verifying a new xAI OAuth temporary file before writing credentials")?;
        let write_result = (|| -> Result<()> {
            temporary
                .write_all(bytes)
                .context("writing xAI OAuth temporary file")?;
            temporary
                .flush()
                .context("flushing xAI OAuth temporary file")?;
            temporary
                .as_file()
                .sync_all()
                .context("syncing xAI OAuth temporary file")?;
            Ok(())
        })();
        if let Err(error) = write_result {
            return Err(cleanup_windows_secret_after_error(
                &security_handle,
                error,
                "temporary file",
            ));
        }
        let persisted = if allow_replace {
            match temporary.persist(&path) {
                Ok(file) => file,
                Err(error) => {
                    let tempfile::PersistError { error, file } = error;
                    let persistence_error = anyhow::Error::new(error)
                        .context("atomically replacing xAI OAuth credentials");
                    let error = cleanup_windows_secret_after_error(
                        &security_handle,
                        persistence_error,
                        "temporary file",
                    );
                    drop(file);
                    return Err(error);
                }
            }
        } else {
            match temporary.persist_noclobber(&path) {
                Ok(file) => file,
                Err(error) => {
                    let tempfile::PersistError { error, file } = error;
                    let persistence_error = anyhow::Error::new(error)
                        .context("installing a new xAI OAuth generation without replacement");
                    let error = cleanup_windows_secret_after_error(
                        &security_handle,
                        persistence_error,
                        "temporary file",
                    );
                    drop(file);
                    return Err(error);
                }
            }
        };
        if let Err(error) = validate_persisted_windows_owned_file(&persisted, &path) {
            // MoveFileEx has already published this exact object. Delete it by
            // handle rather than trusting the pathname again. For a refresh
            // replacement this can leave the unchanged config pointer missing;
            // that fail-closed availability outcome is safer than retaining a
            // generation that failed the post-publication invariant check.
            return Err(cleanup_windows_secret_after_error(
                &security_handle,
                error,
                "rejected generation",
            ));
        }
        Ok(())
    }

    fn remove_raw(&self, name: &str) -> Result<bool> {
        use std::os::windows::fs::OpenOptionsExt as _;
        use windows_sys::Win32::Storage::FileSystem::{
            DELETE, FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_READ, FILE_SHARE_DELETE,
            FILE_SHARE_READ, FILE_SHARE_WRITE,
        };

        validate_private_basename(name)?;
        let path = self.directory.join(name);
        let mut options = fs::OpenOptions::new();
        options
            .access_mode(FILE_GENERIC_READ | DELETE)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        let file = match options.open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error).context("opening xAI OAuth file for exact deletion"),
        };
        validate_owned_file_handle(&file, &path)?;
        mark_windows_file_handle_for_deletion(&file)?;
        drop(file);
        Ok(true)
    }
}

#[cfg(windows)]
fn reopen_windows_file_for_owner_security(file: &File, path: &Path) -> Result<File> {
    use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _};
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::Storage::FileSystem::{
        DELETE, FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_READ, FILE_GENERIC_WRITE,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, ReOpenFile, WRITE_DAC, WRITE_OWNER,
    };

    // ReOpenFile derives a new handle from the already-created temporary file,
    // so no pathname can be substituted between creation and hardening.
    let handle = unsafe {
        ReOpenFile(
            file.as_raw_handle(),
            FILE_GENERIC_READ | FILE_GENERIC_WRITE | WRITE_DAC | WRITE_OWNER | DELETE,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            FILE_FLAG_OPEN_REPARSE_POINT,
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(std::io::Error::last_os_error())
            .context("reopening a new xAI OAuth temporary file for owner-only security");
    }
    // SAFETY: ReOpenFile returned a newly owned handle on the success path.
    let reopened = unsafe { File::from_raw_handle(handle) };
    validate_windows_file_shape(&reopened, path)?;
    Ok(reopened)
}

#[cfg(windows)]
fn mark_windows_file_handle_for_deletion(file: &File) -> Result<()> {
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_DISPOSITION_INFO, FileDispositionInfo, SetFileInformationByHandle,
    };

    let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
    // SAFETY: the disposition buffer has the documented structure and the
    // handle remains owned until after the call. Windows marks this exact file
    // object delete-pending rather than resolving the path again.
    if unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle(),
            FileDispositionInfo,
            (&raw const disposition).cast(),
            std::mem::size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error())
            .context("marking exact xAI OAuth file handle for deletion");
    }
    Ok(())
}

#[cfg(windows)]
fn cleanup_windows_secret_after_error(
    file: &File,
    error: anyhow::Error,
    label: &str,
) -> anyhow::Error {
    match mark_windows_file_handle_for_deletion(file) {
        Ok(()) => error,
        Err(cleanup) => error.context(format!(
            "also failed to delete the xAI OAuth {label} by exact handle: {cleanup:#}"
        )),
    }
}

#[cfg(all(windows, test))]
static WINDOWS_POST_PERSIST_VALIDATION_FAILURE: Mutex<Option<PathBuf>> = Mutex::new(None);

#[cfg(windows)]
fn validate_persisted_windows_owned_file(file: &File, path: &Path) -> Result<fs::Metadata> {
    #[cfg(test)]
    {
        let mut injected = WINDOWS_POST_PERSIST_VALIDATION_FAILURE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if injected.as_deref() == Some(path) {
            *injected = None;
            bail!("injected post-persistence xAI OAuth validation failure");
        }
    }
    validate_owned_file_handle(file, path)
}

#[cfg(all(windows, test))]
fn fail_next_windows_post_persist_validation(path: &Path) {
    *WINDOWS_POST_PERSIST_VALIDATION_FAILURE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(path.to_path_buf());
}

#[cfg(windows)]
fn validate_owned_file_handle(file: &File, path: &Path) -> Result<fs::Metadata> {
    let metadata = validate_windows_file_shape(file, path)?;
    verify_windows_owner_only_handle(file)
        .context("Codewhale-owned xAI OAuth file is not current-user-only")?;
    Ok(metadata)
}

#[cfg(windows)]
fn validate_windows_file_shape(file: &File, path: &Path) -> Result<fs::Metadata> {
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };

    let metadata = validate_windows_handle_path(file, path, false)?;
    let mut information = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: both pointers remain valid for the duration of the call.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) } == 0 {
        return Err(std::io::Error::last_os_error())
            .context("inspecting xAI OAuth file link count");
    }
    anyhow::ensure!(
        information.nNumberOfLinks == 1,
        "xAI OAuth file must not have multiple filesystem links"
    );
    Ok(metadata)
}

#[cfg(windows)]
fn validate_windows_handle_path(
    file: &File,
    expected: &Path,
    expect_directory: bool,
) -> Result<fs::Metadata> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt as _;
    use std::os::windows::fs::MetadataExt as _;
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_NAME_NORMALIZED, GetFinalPathNameByHandleW,
        VOLUME_NAME_DOS,
    };

    let metadata = file.metadata().with_context(|| {
        format!(
            "inspecting Codewhale-owned path {}",
            crate::quote_os_path(expected)
        )
    })?;
    anyhow::ensure!(
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0,
        "Codewhale-owned xAI OAuth path must not be a reparse point"
    );
    anyhow::ensure!(
        if expect_directory {
            metadata.is_dir()
        } else {
            metadata.is_file()
        },
        "Codewhale-owned xAI OAuth path has the wrong filesystem type"
    );
    let flags = FILE_NAME_NORMALIZED | VOLUME_NAME_DOS;
    let handle = file.as_raw_handle();
    // SAFETY: null output asks only for the required UTF-16 length.
    let needed = unsafe { GetFinalPathNameByHandleW(handle, std::ptr::null_mut(), 0, flags) };
    if needed == 0 {
        return Err(std::io::Error::last_os_error())
            .context("resolving Codewhale-owned xAI OAuth handle path");
    }
    let mut buffer = vec![0u16; needed as usize + 1];
    // SAFETY: the buffer is writable and the handle remains valid.
    let written = unsafe {
        GetFinalPathNameByHandleW(handle, buffer.as_mut_ptr(), buffer.len() as u32, flags)
    };
    if written == 0 || written as usize >= buffer.len() {
        return Err(std::io::Error::last_os_error())
            .context("resolving Codewhale-owned xAI OAuth handle path");
    }
    let actual = OsString::from_wide(&buffer[..written as usize]);
    anyhow::ensure!(
        normalize_windows_path_for_comparison(Path::new(&actual))?
            == normalize_windows_path_for_comparison(expected)?,
        "Codewhale-owned xAI OAuth path was redirected while opening"
    );
    Ok(metadata)
}

#[cfg(windows)]
fn normalize_windows_path_for_comparison(path: &Path) -> Result<String> {
    let text = path.to_str().ok_or_else(|| {
        anyhow::anyhow!(
            "xAI OAuth path {} contains invalid Unicode and cannot be compared safely",
            crate::quote_os_path(path)
        )
    })?;
    let without_device_prefix = text.strip_prefix(r"\\?\").unwrap_or(text);
    let normalized_prefix = without_device_prefix.strip_prefix("UNC\\").map_or_else(
        || without_device_prefix.to_string(),
        |rest| format!(r"\\{rest}"),
    );
    Ok(normalized_prefix
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase())
}

#[cfg(windows)]
fn secure_windows_owner_only_handle(file: &File, inherit_to_children: bool) -> Result<()> {
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::Security::Authorization::{
        EXPLICIT_ACCESS_W, SE_FILE_OBJECT, SET_ACCESS, SetEntriesInAclW, SetSecurityInfo,
        TRUSTEE_IS_SID, TRUSTEE_IS_USER, TRUSTEE_W,
    };
    use windows_sys::Win32::Security::{
        DACL_SECURITY_INFORMATION, NO_INHERITANCE, OWNER_SECURITY_INFORMATION,
        PROTECTED_DACL_SECURITY_INFORMATION, SUB_CONTAINERS_AND_OBJECTS_INHERIT,
    };
    use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;

    let user = CurrentWindowsUser::open()?;
    let entry = EXPLICIT_ACCESS_W {
        grfAccessPermissions: FILE_ALL_ACCESS,
        grfAccessMode: SET_ACCESS,
        grfInheritance: if inherit_to_children {
            SUB_CONTAINERS_AND_OBJECTS_INHERIT
        } else {
            NO_INHERITANCE
        },
        Trustee: TRUSTEE_W {
            pMultipleTrustee: std::ptr::null_mut(),
            MultipleTrusteeOperation: 0,
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_USER,
            ptstrName: user.sid().cast::<u16>(),
        },
    };
    let mut acl = std::ptr::null_mut();
    // SAFETY: `entry` and the returned ACL remain live through the following
    // handle-relative security update.
    let result = unsafe { SetEntriesInAclW(1, &raw const entry, std::ptr::null(), &mut acl) };
    if result != ERROR_SUCCESS {
        return Err(std::io::Error::from_raw_os_error(result as i32))
            .context("building a current-user-only DACL for Codewhale-owned xAI OAuth storage");
    }
    let _acl = WindowsLocalAllocation(acl.cast());
    // SAFETY: the file handle remains owned by `file`, and the ACL remains
    // allocated for the duration of the call. The owner and protected DACL are
    // committed together so the verifier never observes a half-secured file.
    let result = unsafe {
        SetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION
                | DACL_SECURITY_INFORMATION
                | PROTECTED_DACL_SECURITY_INFORMATION,
            user.sid(),
            std::ptr::null_mut(),
            acl,
            std::ptr::null(),
        )
    };
    if result != ERROR_SUCCESS {
        return Err(std::io::Error::from_raw_os_error(result as i32))
            .context("applying a current-user-only DACL to Codewhale-owned xAI OAuth storage");
    }
    Ok(())
}

#[cfg(windows)]
fn verify_windows_owner_only_handle(file: &File) -> Result<()> {
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::Security::Authorization::{
        EXPLICIT_ACCESS_W, GRANT_ACCESS, GetExplicitEntriesFromAclW, GetSecurityInfo,
        SE_FILE_OBJECT, SET_ACCESS, TRUSTEE_IS_SID,
    };
    use windows_sys::Win32::Security::{
        ACL, DACL_SECURITY_INFORMATION, EqualSid, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
        PSID,
    };
    use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;

    let user = CurrentWindowsUser::open()?;
    let mut owner: PSID = std::ptr::null_mut();
    let mut dacl: *mut ACL = std::ptr::null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    // SAFETY: the handle remains valid and all output pointers are writable.
    let result = unsafe {
        GetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            std::ptr::null_mut(),
            &mut dacl,
            std::ptr::null_mut(),
            &mut descriptor,
        )
    };
    if result != ERROR_SUCCESS {
        return Err(std::io::Error::from_raw_os_error(result as i32))
            .context("reading Codewhale-owned xAI OAuth security descriptor");
    }
    let _descriptor = WindowsLocalAllocation(descriptor.cast());
    anyhow::ensure!(
        !owner.is_null() && unsafe { EqualSid(owner, user.sid()) } != 0,
        "Codewhale-owned xAI OAuth storage owner is not the current user"
    );
    anyhow::ensure!(
        !dacl.is_null(),
        "Codewhale-owned xAI OAuth storage must have an owner-only DACL"
    );
    let mut count = 0;
    let mut entries: *mut EXPLICIT_ACCESS_W = std::ptr::null_mut();
    // SAFETY: `dacl` belongs to the live descriptor; Windows allocates the
    // returned entry array, released by the guard below.
    let result = unsafe { GetExplicitEntriesFromAclW(dacl, &mut count, &mut entries) };
    if result != ERROR_SUCCESS {
        return Err(std::io::Error::from_raw_os_error(result as i32))
            .context("reading Codewhale-owned xAI OAuth DACL entries");
    }
    let _entries = WindowsLocalAllocation(entries.cast());
    anyhow::ensure!(
        count == 1 && !entries.is_null(),
        "Codewhale-owned xAI OAuth DACL must grant only one user"
    );
    // SAFETY: `count == 1` proves the first returned entry is initialized.
    let entry = unsafe { &*entries };
    let trustee_sid: PSID = entry.Trustee.ptstrName.cast();
    anyhow::ensure!(
        entry.Trustee.TrusteeForm == TRUSTEE_IS_SID
            && !trustee_sid.is_null()
            && unsafe { EqualSid(trustee_sid, user.sid()) } != 0
            && matches!(entry.grfAccessMode, SET_ACCESS | GRANT_ACCESS)
            && entry.grfAccessPermissions == FILE_ALL_ACCESS,
        "Codewhale-owned xAI OAuth DACL is not current-user-only"
    );
    Ok(())
}

#[cfg(windows)]
struct CurrentWindowsUser {
    token: windows_sys::Win32::Foundation::HANDLE,
    token_info: Vec<usize>,
}

#[cfg(windows)]
impl CurrentWindowsUser {
    fn open() -> Result<Self> {
        use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE};
        use windows_sys::Win32::Security::{
            GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser,
        };
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

        let mut token: HANDLE = std::ptr::null_mut();
        // SAFETY: the pseudo-process handle is valid and `token` is writable.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return Err(std::io::Error::last_os_error())
                .context("opening current Windows user token");
        }
        let mut needed = 0;
        // SAFETY: a null buffer/zero length asks for the required size.
        let _ =
            unsafe { GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut needed) };
        if needed == 0 {
            let error = std::io::Error::from_raw_os_error(unsafe { GetLastError() } as i32);
            // SAFETY: the token is owned on this error path.
            unsafe { CloseHandle(token) };
            return Err(error).context("sizing current Windows user token information");
        }
        let words = (needed as usize).div_ceil(std::mem::size_of::<usize>());
        let mut token_info = vec![0usize; words];
        // SAFETY: the aligned buffer contains at least `needed` writable bytes.
        if unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                token_info.as_mut_ptr().cast(),
                needed,
                &mut needed,
            )
        } == 0
        {
            let error = std::io::Error::last_os_error();
            // SAFETY: the token is owned on this error path.
            unsafe { CloseHandle(token) };
            return Err(error).context("reading current Windows user token information");
        }
        let user = unsafe { &*token_info.as_ptr().cast::<TOKEN_USER>() };
        if user.User.Sid.is_null() {
            // SAFETY: the token is owned on this error path.
            unsafe { CloseHandle(token) };
            bail!("current Windows user token has no SID");
        }
        Ok(Self { token, token_info })
    }

    fn sid(&self) -> windows_sys::Win32::Security::PSID {
        use windows_sys::Win32::Security::TOKEN_USER;
        // SAFETY: the aligned token buffer remains owned by `self`.
        unsafe { (*self.token_info.as_ptr().cast::<TOKEN_USER>()).User.Sid }
    }
}

#[cfg(windows)]
impl Drop for CurrentWindowsUser {
    fn drop(&mut self) {
        // SAFETY: `token` is owned by this guard and closed exactly once.
        unsafe { windows_sys::Win32::Foundation::CloseHandle(self.token) };
    }
}

#[cfg(windows)]
struct WindowsLocalAllocation(*mut core::ffi::c_void);

#[cfg(windows)]
impl Drop for WindowsLocalAllocation {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: Windows allocated this block for a LocalFree caller.
            unsafe { windows_sys::Win32::Foundation::LocalFree(self.0) };
        }
    }
}

#[cfg(not(any(unix, windows)))]
fn open_owned_credentials_directory(directory: &Path) -> Result<XaiOAuthCredentialStore> {
    fs::create_dir_all(directory)?;
    let metadata = fs::symlink_metadata(directory)?;
    anyhow::ensure!(
        metadata.is_dir(),
        "Codewhale credentials path must be a directory"
    );
    Ok(XaiOAuthCredentialStore {
        directory: directory.to_path_buf(),
    })
}

#[cfg(not(any(unix, windows)))]
impl XaiOAuthCredentialStore {
    fn open_owned_file_for_read(&self, name: &str) -> Result<Option<File>> {
        validate_private_basename(name)?;
        match File::open(self.directory.join(name)) {
            Ok(file) => Ok(Some(file)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    fn open_internal_file(&self, name: &str) -> Result<File> {
        validate_private_basename(name)?;
        Ok(fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(self.directory.join(name))?)
    }

    fn write_owned_file(&self, name: &str, bytes: &[u8], allow_replace: bool) -> Result<()> {
        let path = self.directory.join(name);
        anyhow::ensure!(
            allow_replace || !path.exists(),
            "refusing to replace xAI OAuth generation"
        );
        crate::persistence::atomic_write(&path, bytes)
    }

    fn remove_raw(&self, name: &str) -> Result<bool> {
        validate_private_basename(name)?;
        match fs::remove_file(self.directory.join(name)) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.into()),
        }
    }

    fn rename_raw(&self, from: &str, to: &str) -> Result<()> {
        validate_private_basename(from)?;
        validate_private_basename(to)?;
        fs::rename(self.directory.join(from), self.directory.join(to))?;
        Ok(())
    }
}

#[cfg(not(any(unix, windows)))]
fn validate_owned_file_handle(file: &File, _path: &Path) -> Result<fs::Metadata> {
    let metadata = file.metadata()?;
    anyhow::ensure!(metadata.is_file(), "xAI OAuth path must be a regular file");
    Ok(metadata)
}

/// Delete one superseded generation after its replacement pointer committed.
/// The basename is validated before any filesystem access.
pub fn remove_xai_oauth_generation(generation: &str) -> Result<bool> {
    let generation = validate_xai_oauth_generation(generation)?;
    with_xai_oauth_lifecycle_lock(|store| store.remove(generation))
}

/// Explicit logout policy: remove the legacy Codewhale-owned file and every
/// valid generated xAI OAuth file. Unknown files in the credentials directory
/// are never touched.
pub fn clear_all_xai_oauth_credentials() -> Result<usize> {
    with_xai_oauth_lifecycle_lock(XaiOAuthCredentialStore::clear_all)
}

pub fn clear_all_chatgpt_oauth_credentials() -> Result<usize> {
    with_xai_oauth_lifecycle_lock(XaiOAuthCredentialStore::clear_chatgpt)
}

/// Clears every Codewhale-owned ChatGPT OAuth file without re-entering the
/// lifecycle lock. Only for code already running inside
/// [`with_xai_oauth_lifecycle_lock`] or [`with_xai_oauth_revocation_transaction`];
/// everyone else must call [`clear_all_chatgpt_oauth_credentials`], which
/// takes the lock. The lifecycle lock is a non-reentrant in-process mutex, so
/// a nested call would deadlock the logout path (seen as CI hangs on the
/// `logout_*` tests).
pub fn clear_all_chatgpt_oauth_credentials_locked() -> Result<usize> {
    let store = XaiOAuthCredentialStore::open()?;
    XaiOAuthCredentialStore::clear_chatgpt(&store)
}

pub fn remove_chatgpt_oauth_generation(generation: &str) -> Result<bool> {
    let generation = validate_chatgpt_oauth_generation(generation)?;
    with_xai_oauth_lifecycle_lock(|store| store.remove(generation))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_directory_preserves_lexical_identity() {
        let directory = tempfile::tempdir().expect("temp dir");
        let lexical = directory.path().join("missing").join("credentials");
        assert_eq!(
            lexical_absolute_path(&lexical).expect("preserve lexical path"),
            lexical
        );
        assert!(!lexical.exists());
        assert!(
            lexical_absolute_path(&directory.path().join("missing/../escape")).is_err(),
            "owned credential roots must reject traversal components"
        );
    }

    #[test]
    fn generation_names_are_strict_basenames() {
        let valid = "xai-auth-0123456789abcdef0123456789abcdef.json";
        assert!(is_valid_xai_oauth_generation(valid));
        for invalid in [
            "../xai-auth-0123456789abcdef0123456789abcdef.json",
            "/tmp/xai-auth-0123456789abcdef0123456789abcdef.json",
            "xai-auth-0123456789ABCDEF0123456789ABCDEF.json",
            "xai-auth-short.json",
            "xai-auth.json",
        ] {
            assert!(!is_valid_xai_oauth_generation(invalid), "{invalid}");
        }
        let chatgpt = "chatgpt-auth-0123456789abcdef0123456789abcdef.json";
        assert!(is_valid_chatgpt_oauth_generation(chatgpt));
        assert!(!is_valid_chatgpt_oauth_generation(valid));
        assert!(!is_valid_xai_oauth_generation(chatgpt));
        assert!(!is_valid_chatgpt_oauth_generation(
            "../chatgpt-auth-0123456789abcdef0123456789abcdef.json"
        ));
    }

    #[test]
    fn logout_cleanup_removes_only_owned_xai_files() {
        let directory = tempfile::tempdir().expect("temp dir");
        let directory = directory.path().canonicalize().expect("canonical temp dir");
        let store = open_owned_credentials_directory(&directory).expect("open store");
        let generation = "xai-auth-0123456789abcdef0123456789abcdef.json";
        store
            .write(generation, b"secret", false)
            .expect("generation");
        store
            .write(LEGACY_XAI_OAUTH_FILE_NAME, b"legacy", false)
            .expect("legacy");
        fs::write(directory.join("other-provider.json"), "keep").expect("other provider");

        assert_eq!(store.clear_all().expect("clear"), 2);
        assert!(directory.join("other-provider.json").exists());
        assert!(!directory.join(generation).exists());
        assert!(!directory.join("xai-auth.json").exists());
    }

    #[test]
    fn logout_cleanup_removes_chatgpt_files_without_touching_xai() {
        let directory = tempfile::tempdir().expect("temp dir");
        let directory = directory.path().canonicalize().expect("canonical temp dir");
        let store = open_owned_credentials_directory(&directory).expect("open store");
        let chatgpt = "chatgpt-auth-0123456789abcdef0123456789abcdef.json";
        let xai = "xai-auth-0123456789abcdef0123456789abcdef.json";
        store.write(chatgpt, b"chatgpt", false).expect("chatgpt");
        store.write(xai, b"xai", false).expect("xai");
        store
            .write(LEGACY_CHATGPT_OAUTH_FILE_NAME, b"legacy", false)
            .expect("legacy chatgpt");

        assert_eq!(store.clear_chatgpt().expect("clear chatgpt"), 2);
        assert!(directory.join(xai).exists());
        assert!(!directory.join(chatgpt).exists());
        assert!(!directory.join(LEGACY_CHATGPT_OAUTH_FILE_NAME).exists());
    }

    #[cfg(unix)]
    #[test]
    fn unix_store_pins_directory_identity_across_lexical_path_swap() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().expect("temp dir");
        let root = root.path().canonicalize().expect("canonical temp root");
        let credentials = root.join("credentials");
        fs::create_dir(&credentials).expect("credentials");
        let store = open_owned_credentials_directory(&credentials).expect("open pinned store");
        let parked = root.join("parked-credentials");
        let external = root.join("external-owner");
        fs::create_dir(&external).expect("external directory");
        fs::rename(&credentials, &parked).expect("park credentials");
        symlink(&external, &credentials).expect("replace lexical path with symlink");

        let generation = "xai-auth-0123456789abcdef0123456789abcdef.json";
        store
            .write(generation, b"pinned bytes", false)
            .expect("write through pinned directory handle");

        assert_eq!(fs::read(parked.join(generation)).unwrap(), b"pinned bytes");
        assert!(!external.join(generation).exists());
        assert_eq!(
            store.read_to_string(generation).unwrap().as_deref(),
            Some("pinned bytes")
        );
    }

    #[cfg(unix)]
    #[test]
    fn unix_store_rejects_symlinked_root_component_and_hardlinked_leaf() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().expect("temp dir");
        let root = root.path().canonicalize().expect("canonical temp root");
        let real = root.join("real-home");
        fs::create_dir(&real).expect("real home");
        let linked = root.join("linked-home");
        symlink(&real, &linked).expect("home symlink");
        assert!(
            open_owned_credentials_directory(&linked.join("credentials")).is_err(),
            "owned roots must reject every symlink component"
        );

        let credentials = real.join("credentials");
        let store = open_owned_credentials_directory(&credentials).expect("safe store");
        let generation = "xai-auth-fedcba9876543210fedcba9876543210.json";
        store
            .write(generation, b"secret", false)
            .expect("seed generation");
        fs::hard_link(
            credentials.join(generation),
            credentials.join("attacker-hardlink"),
        )
        .expect("hardlink fixture");
        assert!(
            store.read_to_string(generation).is_err(),
            "owned reads must reject multiply-linked credential files"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_owned_path_identity_is_case_insensitive_and_lossless() {
        use std::ffi::OsString;
        use std::os::windows::ffi::OsStringExt as _;

        assert_eq!(
            normalize_windows_path_for_comparison(Path::new(r"C:\Users\Alice\Credentials"))
                .unwrap(),
            normalize_windows_path_for_comparison(Path::new(r"\\?\c:\users\ALICE\credentials"))
                .unwrap()
        );
        let invalid = PathBuf::from(OsString::from_wide(&[
            b'C' as u16,
            b':' as u16,
            b'\\' as u16,
            0xd800,
        ]));
        assert!(normalize_windows_path_for_comparison(&invalid).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn windows_post_persist_failure_exact_deletes_new_and_replacement_generations() {
        fn assert_directory_empty(path: &Path) {
            let entries = fs::read_dir(path)
                .expect("read credentials directory")
                .collect::<std::io::Result<Vec<_>>>()
                .expect("read credential entries");
            assert!(
                entries.is_empty(),
                "rejected credential bytes must leave no durable file: {entries:?}"
            );
        }

        let root = tempfile::tempdir().expect("temp dir");
        let root = root.path().canonicalize().expect("canonical temp root");
        let credentials = root.join("new-credentials");
        let store = open_owned_credentials_directory(&credentials).expect("open secure store");
        let generation = "xai-auth-0123456789abcdef0123456789abcdef.json";
        let generation_path = credentials.join(generation);
        fail_next_windows_post_persist_validation(&generation_path);
        let error = store
            .write(generation, b"new credential bytes", false)
            .expect_err("post-persist validation must fail");
        assert!(error.to_string().contains("injected post-persistence"));
        assert_directory_empty(&credentials);

        let replacement_credentials = root.join("replacement-credentials");
        let replacement_store = open_owned_credentials_directory(&replacement_credentials)
            .expect("open replacement store");
        let replacement_path = replacement_credentials.join(generation);
        replacement_store
            .write(generation, b"prior credential bytes", false)
            .expect("seed prior generation");
        fail_next_windows_post_persist_validation(&replacement_path);
        let error = replacement_store
            .write(generation, b"replacement credential bytes", true)
            .expect_err("replacement validation must fail");
        assert!(error.to_string().contains("injected post-persistence"));
        assert!(
            !replacement_path.exists(),
            "a rejected replacement deliberately fails closed instead of restoring by path"
        );
        assert_directory_empty(&replacement_credentials);
    }

    #[cfg(windows)]
    #[test]
    fn windows_store_secures_every_new_owned_object_for_the_current_user() {
        let root = tempfile::tempdir().expect("temp dir");
        let root = root.path().canonicalize().expect("canonical temp root");
        let credentials = root.join("credentials");
        let store = open_owned_credentials_directory(&credentials).expect("open secure store");

        let directory = store
            ._component_handles
            .last()
            .expect("final credentials directory handle");
        verify_windows_owner_only_handle(directory).expect("current-user-only directory");

        let lock = store.open_lock_file().expect("create lifecycle lock");
        validate_owned_file_handle(&lock, &credentials.join(XAI_OAUTH_LIFECYCLE_LOCK_FILE_NAME))
            .expect("current-user-only lifecycle lock");

        let generation = "xai-auth-0123456789abcdef0123456789abcdef.json";
        store
            .write(generation, b"credential bytes", false)
            .expect("write secure generation");
        let generation_file = store
            .open_owned_file_for_read(generation)
            .expect("open generation")
            .expect("generation exists");
        validate_owned_file_handle(&generation_file, &credentials.join(generation))
            .expect("current-user-only generation");
    }

    #[cfg(windows)]
    #[test]
    fn windows_store_rejects_reparse_component_and_hardlinked_leaf() {
        let root = tempfile::tempdir().expect("temp dir");
        let root = root.path().canonicalize().expect("canonical temp root");
        let real = root.join("real-home");
        fs::create_dir(&real).expect("real home");
        let linked = root.join("linked-home");
        if std::os::windows::fs::symlink_dir(&real, &linked).is_ok() {
            assert!(
                open_owned_credentials_directory(&linked.join("credentials")).is_err(),
                "owned roots must reject junctions and directory reparse points"
            );
        }

        let credentials = real.join("credentials");
        let store = open_owned_credentials_directory(&credentials).expect("safe store");
        let generation = "xai-auth-fedcba9876543210fedcba9876543210.json";
        store
            .write(generation, b"secret", false)
            .expect("seed generation");
        fs::hard_link(
            credentials.join(generation),
            credentials.join("attacker-hardlink"),
        )
        .expect("hardlink fixture");
        assert!(
            store.read_to_string(generation).is_err(),
            "owned reads must reject multiply-linked credential files"
        );
    }
}
