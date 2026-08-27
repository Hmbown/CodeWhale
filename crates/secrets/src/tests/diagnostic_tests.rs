use super::{EnvVarGuard, clear_known_envs, env_lock};
use crate::{
    SECRET_BACKEND_ENV, SecretBackendDiagnosticKind, SecretBackendInspection,
    SecretBackendPresence, diagnose_secret_backend,
};

#[test]
fn retired_keychain_env_diagnoses_the_file_store() {
    let _lock = env_lock();
    clear_known_envs();
    let tmp = tempfile::tempdir().unwrap();
    let _home = EnvVarGuard::set("CODEWHALE_HOME", tmp.path());
    let _backend = EnvVarGuard::set(SECRET_BACKEND_ENV, "system");

    let diagnostic = diagnose_secret_backend();

    assert_eq!(diagnostic.backend, SecretBackendDiagnosticKind::File);
    assert_eq!(diagnostic.inspection, SecretBackendInspection::MetadataOnly);
    assert_ne!(diagnostic.path, None);
    assert!(!format!("{diagnostic:?}").contains("Keychain"));
}

#[test]
fn file_backend_diagnostic_reads_metadata_not_secret_contents() {
    let _lock = env_lock();
    clear_known_envs();
    let tmp = tempfile::tempdir().unwrap();
    let codewhale_home = std::fs::canonicalize(tmp.path())
        .unwrap()
        .join("isolated-home");
    let path = codewhale_home.join("secrets").join("secrets.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let sentinel = "diagnostic-must-not-emit-this-secret";
    let invalid_secret_blob = format!("not-json:{sentinel}");
    std::fs::write(&path, &invalid_secret_blob).unwrap();
    let _home = EnvVarGuard::set("CODEWHALE_HOME", &codewhale_home);
    let _backend = EnvVarGuard::set(SECRET_BACKEND_ENV, "file");

    let diagnostic = diagnose_secret_backend();
    let serialized = serde_json::to_string(&diagnostic).unwrap();

    assert_eq!(diagnostic.backend, SecretBackendDiagnosticKind::File);
    assert_eq!(diagnostic.inspection, SecretBackendInspection::MetadataOnly);
    assert_eq!(diagnostic.path.as_deref(), Some(path.as_path()));
    assert_eq!(diagnostic.presence, SecretBackendPresence::Present);
    assert_eq!(diagnostic.legacy_path, None);
    assert!(!serialized.contains(sentinel));
    assert_eq!(std::fs::read_to_string(path).unwrap(), invalid_secret_blob);
}

#[test]
fn absent_file_backend_diagnostic_creates_no_state() {
    let _lock = env_lock();
    clear_known_envs();
    let tmp = tempfile::tempdir().unwrap();
    let codewhale_home = tmp.path().join("never-created-home");
    let path = codewhale_home.join("secrets").join("secrets.json");
    let _home = EnvVarGuard::set("CODEWHALE_HOME", &codewhale_home);
    let _backend = EnvVarGuard::set(SECRET_BACKEND_ENV, "file");

    let diagnostic = diagnose_secret_backend();

    assert_eq!(diagnostic.path.as_deref(), Some(path.as_path()));
    assert_eq!(diagnostic.presence, SecretBackendPresence::Absent);
    assert!(!codewhale_home.exists());
}

#[cfg(unix)]
#[test]
fn file_backend_diagnostic_rejects_a_symlinked_parent() {
    use std::os::unix::fs::symlink;

    let _lock = env_lock();
    clear_known_envs();
    let tmp = tempfile::tempdir().unwrap();
    let real_home = tmp.path().join("real-home");
    let linked_home = tmp.path().join("linked-home");
    let path = real_home.join("secrets").join("secrets.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "parent-symlink-secret-sentinel").unwrap();
    symlink(&real_home, &linked_home).unwrap();
    let _home = EnvVarGuard::set("CODEWHALE_HOME", &linked_home);
    let _backend = EnvVarGuard::set(SECRET_BACKEND_ENV, "file");

    let diagnostic = diagnose_secret_backend();

    assert_eq!(diagnostic.presence, SecretBackendPresence::Unknown);
}
