use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use codewhale_config::cloud_facts::{CloudFactsState, KeyStatus, TrustedKey, overlay};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpListener;

use super::*;

/// Cross-language fixture signed with the TEST-ONLY key.
const FIXTURE_V7: &str = include_str!("../../../docs/cloud-facts/fixtures/envelope-stable-v7.json");
const FIXTURE_FUTURE_V8: &str =
    include_str!("../../../docs/cloud-facts/fixtures/envelope-future-only-v8.json");

fn test_keys() -> &'static [TrustedKey] {
    static KEYS: OnceLock<Vec<TrustedKey>> = OnceLock::new();
    KEYS.get_or_init(|| {
        vec![TrustedKey {
            key_id: "cwf-test-only",
            public_key: [
                243, 225, 75, 13, 110, 14, 162, 181, 4, 77, 69, 100, 179, 72, 105, 64, 8, 185, 46,
                62, 48, 131, 121, 35, 42, 55, 216, 23, 50, 219, 39, 181,
            ],
            status: KeyStatus::Active,
        }]
    })
}

/// The overlay/status are process-wide; serialize tests that touch them.
fn lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}

fn settings(dir: &tempfile::TempDir, url: Option<String>) -> Settings {
    Settings {
        enabled: true,
        channel: "stable".into(),
        url,
        ttl_secs: 3600,
        cache_path: Some(dir.path().join("facts").join(CACHE_FILE)),
        local_path: None,
    }
}

/// One canned HTTP response per connection; records the request line/headers.
struct MockServer {
    url: String,
    requests: Arc<Mutex<Vec<String>>>,
}

/// `(status, headers, body)` canned HTTP response.
type CannedResponse = (u16, Vec<(&'static str, String)>, String);

async fn mock_server(responses: Vec<CannedResponse>) -> MockServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&requests);
    tokio::spawn(async move {
        let mut responses = responses.into_iter();
        while let Ok((mut stream, _)) = listener.accept().await {
            let mut buf = vec![0u8; 8192];
            let n = stream.read(&mut buf).await.unwrap_or(0);
            let head = String::from_utf8_lossy(&buf[..n]).into_owned();
            seen.lock().unwrap().push(head);
            let (status, headers, body) =
                responses
                    .next()
                    .unwrap_or((500, vec![], "no more canned responses".into()));
            let reason = match status {
                200 => "OK",
                304 => "Not Modified",
                404 => "Not Found",
                _ => "Error",
            };
            let mut out = format!("HTTP/1.1 {status} {reason}\r\nConnection: close\r\n");
            for (k, v) in headers {
                out.push_str(&format!("{k}: {v}\r\n"));
            }
            out.push_str(&format!("Content-Length: {}\r\n\r\n{}", body.len(), body));
            let _ = stream.write_all(out.as_bytes()).await;
            let _ = stream.shutdown().await;
        }
    });
    MockServer {
        url: format!("http://{addr}/api/facts/v1/{{channel}}"),
        requests,
    }
}

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
}

#[test]
fn flag_off_means_no_client_no_file_and_off_status() {
    let _lock = lock();
    overlay::clear();
    let dir = tempfile::tempdir().unwrap();
    let mut s = settings(&dir, Some("http://127.0.0.1:1/{channel}".into()));
    s.enabled = false;
    assert_eq!(maybe_load_persisted_cache_with_keys(&s, test_keys()), None);
    assert_eq!(status().state, CloudFactsState::Off);
    let err = rt()
        .block_on(refresh_with_keys(&s, true, test_keys()))
        .unwrap_err();
    assert_eq!(err, RefreshError::Disabled);
    assert!(
        !dir.path().join("facts").exists(),
        "flag off must write nothing"
    );
    assert!(overlay::overlay().is_none());
}

#[test]
fn enabled_with_no_active_key_is_inert_and_never_fetches() {
    let _lock = lock();
    overlay::clear();
    let dir = tempfile::tempdir().unwrap();
    let s = settings(&dir, Some("http://127.0.0.1:1/{channel}".into()));
    assert_eq!(maybe_load_persisted_cache_with_keys(&s, &[]), None);
    assert_eq!(status().state, CloudFactsState::Inert);
    let err = rt().block_on(refresh_with_keys(&s, true, &[])).unwrap_err();
    assert_eq!(err, RefreshError::Inert);
    assert!(!dir.path().join("facts").exists());
}

#[test]
fn network_200_verifies_installs_caches_and_304_keeps_it() {
    let _lock = lock();
    overlay::clear();
    let rt = rt();
    let server = rt.block_on(mock_server(vec![
        (
            200,
            vec![
                ("ETag", "\"stable-v7-abc\"".into()),
                ("Content-Type", "application/json".into()),
            ],
            FIXTURE_V7.into(),
        ),
        (
            304,
            vec![("ETag", "\"stable-v7-abc\"".into())],
            String::new(),
        ),
    ]));
    let dir = tempfile::tempdir().unwrap();
    let s = settings(&dir, Some(server.url.clone()));

    let outcome = rt
        .block_on(refresh_with_keys(&s, true, test_keys()))
        .unwrap();
    assert_eq!(outcome, RefreshOutcome::Updated { facts_version: 7 });
    let st = status();
    assert!(
        matches!(
            st.state,
            CloudFactsState::Verified {
                facts_version: 7,
                origin: FactsOrigin::Network,
                patches: 5,
                defaults: 1,
                announcements: 1,
                ..
            }
        ),
        "{st:?}"
    );
    assert_eq!(st.etag.as_deref(), Some("\"stable-v7-abc\""));
    let overlay = overlay::overlay().expect("overlay installed");
    assert_eq!(overlay.facts_version, 7);
    assert_eq!(
        overlay::cloud_default_model("deepseek")
            .map(|(m, _)| m)
            .as_deref(),
        Some("deepseek-v4-pro")
    );

    // Cache file exists, is secret-free, and carries the envelope + etag.
    let cache = std::fs::read_to_string(s.cache_path.as_ref().unwrap()).unwrap();
    assert!(cache.contains("stable-v7-abc"));
    assert!(cache.contains("cwf-test-only"));
    for needle in ["api_key", "authorization", "bearer", "password"] {
        assert!(!cache.to_lowercase().contains(&format!("\"{needle}\"")));
    }

    // Second fetch sends If-None-Match and keeps the overlay on 304.
    let outcome = rt
        .block_on(refresh_with_keys(&s, true, test_keys()))
        .unwrap();
    assert_eq!(
        outcome,
        RefreshOutcome::NotModified {
            facts_version: Some(7)
        }
    );
    let requests = server.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(
        requests[1]
            .to_lowercase()
            .contains("if-none-match: \"stable-v7-abc\""),
        "{}",
        requests[1]
    );
    for req in requests.iter() {
        assert!(
            req.contains(&format!("User-Agent: {USER_AGENT}"))
                || req.to_lowercase().contains("user-agent: codewhale/")
        );
        assert!(!req.to_lowercase().contains("cookie"));
        assert!(
            req.lines()
                .next()
                .unwrap()
                .contains("/api/facts/v1/stable HTTP/1.1"),
            "{}",
            req.lines().next().unwrap()
        );
    }
    assert!(matches!(
        status().state,
        CloudFactsState::Verified {
            facts_version: 7,
            ..
        }
    ));
    overlay::clear();
}

#[test]
fn persisted_cache_round_trips_and_a_tampered_cache_is_rejected_and_deleted() {
    let _lock = lock();
    overlay::clear();
    let rt = rt();
    let server = rt.block_on(mock_server(vec![(200, vec![], FIXTURE_V7.into())]));
    let dir = tempfile::tempdir().unwrap();
    let s = settings(&dir, Some(server.url.clone()));
    rt.block_on(refresh_with_keys(&s, true, test_keys()))
        .unwrap();
    overlay::clear();

    // Fresh process: the cache seeds the overlay without a network call.
    assert_eq!(
        maybe_load_persisted_cache_with_keys(&s, test_keys()),
        Some(7)
    );
    assert!(matches!(
        status().state,
        CloudFactsState::Verified {
            origin: FactsOrigin::DiskCache,
            ..
        }
    ));
    assert!(overlay::overlay().is_some());
    overlay::clear();

    // Tamper one payload byte on disk.
    let path = s.cache_path.clone().unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    let mut cache: serde_json::Value = serde_json::from_str(&text).unwrap();
    let env = cache["envelope"]
        .as_str()
        .unwrap()
        .replace("\"facts_version\": 7", "\"facts_version\": 9");
    cache["envelope"] = serde_json::Value::String(env);
    std::fs::write(&path, serde_json::to_vec(&cache).unwrap()).unwrap();
    assert_eq!(maybe_load_persisted_cache_with_keys(&s, test_keys()), None);
    assert!(
        matches!(status().state, CloudFactsState::Rejected { .. }),
        "{:?}",
        status().state
    );
    assert!(!path.exists(), "tampered cache must be deleted");
    assert!(overlay::overlay().is_none());
}

#[test]
fn local_path_loads_without_network_and_scope_rejection_is_reported() {
    let _lock = lock();
    overlay::clear();
    let dir = tempfile::tempdir().unwrap();
    let local = dir.path().join("envelope.json");
    std::fs::write(&local, FIXTURE_V7).unwrap();
    let mut s = settings(&dir, Some("http://127.0.0.1:1/{channel}".into()));
    s.local_path = Some(local.clone());
    let outcome = rt()
        .block_on(refresh_with_keys(&s, true, test_keys()))
        .unwrap();
    assert_eq!(outcome, RefreshOutcome::Updated { facts_version: 7 });
    assert!(matches!(
        status().state,
        CloudFactsState::Verified {
            origin: FactsOrigin::LocalFile,
            ..
        }
    ));

    std::fs::write(&local, FIXTURE_FUTURE_V8).unwrap();
    let err = rt()
        .block_on(refresh_with_keys(&s, true, test_keys()))
        .unwrap_err();
    assert!(matches!(
        err,
        RefreshError::Rejected(FactsRejection::NotApplicable { .. })
    ));
    assert!(matches!(
        status().state,
        CloudFactsState::NotApplicable { .. }
    ));
    overlay::clear();
}

#[test]
fn server_errors_keep_prior_facts_and_persist_backoff() {
    let _lock = lock();
    overlay::clear();
    let rt = rt();
    let server = rt.block_on(mock_server(vec![
        (200, vec![], FIXTURE_V7.into()),
        (500, vec![], "boom".into()),
        (200, vec![], "x".repeat(MAX_BODY_BYTES + 1)),
    ]));
    let dir = tempfile::tempdir().unwrap();
    let s = settings(&dir, Some(server.url.clone()));
    rt.block_on(refresh_with_keys(&s, true, test_keys()))
        .unwrap();

    let err = rt
        .block_on(refresh_with_keys(&s, true, test_keys()))
        .unwrap_err();
    assert_eq!(err, RefreshError::HttpStatus(500));
    assert!(
        matches!(
            status().state,
            CloudFactsState::Failed {
                keeping: Some(7),
                ..
            }
        ),
        "{:?}",
        status().state
    );
    assert!(
        overlay::overlay().is_some(),
        "prior verified facts survive a failure"
    );

    // Backoff is persisted and honoured by non-forced refreshes.
    let cache: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(s.cache_path.as_ref().unwrap()).unwrap())
            .unwrap();
    assert!(cache["backoff_until"].as_u64().unwrap() > now_unix());
    let err = rt
        .block_on(refresh_with_keys(&s, false, test_keys()))
        .unwrap_err();
    assert!(matches!(err, RefreshError::BackingOff { .. }));

    // Oversized body is refused before verification.
    let err = rt
        .block_on(refresh_with_keys(&s, true, test_keys()))
        .unwrap_err();
    assert!(matches!(err, RefreshError::TooLarge(_)));
    assert!(overlay::overlay().is_some());
    overlay::clear();
}

#[test]
fn not_found_means_no_facts_not_failure() {
    let _lock = lock();
    overlay::clear();
    let rt = rt();
    let server = rt.block_on(mock_server(vec![(
        404,
        vec![],
        "{\"error\":\"no-facts\"}".into(),
    )]));
    let dir = tempfile::tempdir().unwrap();
    let s = settings(&dir, Some(server.url.clone()));
    let outcome = rt
        .block_on(refresh_with_keys(&s, true, test_keys()))
        .unwrap();
    assert_eq!(outcome, RefreshOutcome::NoFacts);
    assert_eq!(status().state, CloudFactsState::BundledOnly);
    assert!(overlay::overlay().is_none());
}

#[test]
fn settings_resolve_url_template_and_channel_validation() {
    let s = Settings {
        channel: "beta".into(),
        ..Settings::default()
    };
    assert_eq!(s.url(), "https://codewhale.net/api/facts/v1/beta");
    assert!(valid_channel("stable"));
    assert!(valid_channel("beta-2"));
    assert!(!valid_channel("-bad"));
    assert!(!valid_channel("Stable"));
    assert!(!valid_channel(""));
    assert_eq!(
        Settings::default().cache_path.as_deref(),
        None::<&std::path::Path>,
        "default settings resolve the cache under CODEWHALE_HOME"
    );
    let _ = PathBuf::new();
}
