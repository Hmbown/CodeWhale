use super::*;

#[test]
fn retryability_distinguishes_transient_failures_from_durable_failures() {
    for error in [
        LlmError::RateLimited {
            message: "too many requests".into(),
            retry_after: None,
        },
        LlmError::ServerError {
            status: 500,
            message: "internal error".into(),
        },
        LlmError::NetworkError("connection refused".into()),
        LlmError::Timeout(Duration::from_secs(30)),
    ] {
        assert!(error.is_retryable(), "expected transient error: {error}");
    }
    for error in [
        LlmError::authentication_error("invalid key"),
        LlmError::AuthorizationError("blocked".into()),
        LlmError::InvalidRequest {
            status: 400,
            message: "bad json".into(),
        },
        LlmError::ContentPolicyError("unsafe content".into()),
        LlmError::ContextLengthError("too long".into()),
    ] {
        assert!(!error.is_retryable(), "expected durable error: {error}");
    }
}

#[test]
fn http_response_boundary_classifies_status_contract() {
    assert!(matches!(
        LlmError::from_http_response(429, "rate limit exceeded"),
        LlmError::RateLimited { .. }
    ));
    assert!(matches!(
        LlmError::from_http_response(401, "invalid api key"),
        LlmError::AuthenticationError(_)
    ));
    assert!(matches!(
        LlmError::from_http_response(403, "forbidden"),
        LlmError::AuthorizationError(_)
    ));
    assert!(matches!(
        LlmError::from_http_response(403, "invalid api key"),
        LlmError::AuthenticationError(_)
    ));
    let cancelled = LlmError::from_http_response(499, "upstream request cancelled");
    assert!(matches!(
        &cancelled,
        LlmError::ServerError { status: 499, .. }
    ));
    assert!(cancelled.is_retryable());
    assert!(matches!(
        LlmError::from_http_response(500, "internal server error"),
        LlmError::ServerError { status: 500, .. }
    ));
    assert!(matches!(
        LlmError::from_http_response(503, "service unavailable"),
        LlmError::ServerError { status: 503, .. }
    ));
    assert!(matches!(
        LlmError::from_http_response(400, "context_length_exceeded"),
        LlmError::ContextLengthError(_)
    ));
    assert!(matches!(
        LlmError::from_http_response(400, "content_policy_violation"),
        LlmError::ContentPolicyError(_)
    ));
    assert!(matches!(
        LlmError::from_http_response(400, "invalid json"),
        LlmError::InvalidRequest { status: 400, .. }
    ));
    // "Unsupported parameter: max_output_tokens" names a *token* field, which
    // the generic keyword rules misread as a context-window overflow. It is a
    // request-shape error, and retrying or compacting cannot fix it.
    assert!(matches!(
        LlmError::from_http_response(
            400,
            "{\"error\":{\"code\":\"unsupported_parameter\",\"message\":\"Unsupported parameter: max_output_tokens\"}}"
        ),
        LlmError::InvalidRequest { status: 400, .. }
    ));
    assert!(matches!(
        LlmError::from_http_response(
            400,
            "{\"error\":{\"type\":\"invalid_request_error\",\"message\":\"Unsupported parameter: temperature\"}}"
        ),
        LlmError::InvalidRequest { status: 400, .. }
    ));
}

#[test]
fn explicit_400_402_and_429_quota_responses_are_typed_and_non_retryable() {
    for (status, body) in [
        (
            400,
            r#"{"error":{"code":"insufficient_quota","message":"You exceeded your current quota"}}"#,
        ),
        (
            429,
            r#"{"error":{"type":"insufficient_quota","message":"Billing limit reached"}}"#,
        ),
        (
            402,
            r#"{"error":{"code":"billing_hard_limit_reached","message":"Payment required"}}"#,
        ),
        (
            429,
            "You exceeded your current quota. Please check your plan and billing details.",
        ),
        (429, "Account quota exhausted"),
    ] {
        let error = LlmError::from_http_response(status, body);
        assert!(matches!(error, LlmError::QuotaExhausted(_)));
        assert!(!error.is_retryable());
    }

    let raw = r#"{"error":{"code":"billing_hard_limit_reached","message":"Account unavailable"}}"#;
    let safe = sanitize_http_error_body(Some("fixture"), 429, raw);
    assert!(matches!(
        LlmError::from_http_response(429, &safe),
        LlmError::QuotaExhausted(_)
    ));
}

#[test]
fn generic_429_stays_rate_limited_and_retryable() {
    for body in [
        "Too Many Requests",
        "Rate limit on your API quota exceeded",
        "Requests per minute quota exceeded",
        "Quota rate limit exceeded; retry after 10 seconds",
    ] {
        let error = LlmError::from_http_response(429, body);
        assert!(
            matches!(error, LlmError::RateLimited { .. }),
            "expected transient rate limit for {body:?}, got {error:?}"
        );
        assert!(error.is_retryable());
    }

    let raw = r#"{"error":{"code":"RESOURCE_EXHAUSTED","message":"Rate limit on your API quota exceeded"}}"#;
    let safe = sanitize_http_error_body(Some("fixture"), 429, raw);
    let error = LlmError::from_http_response(429, &safe);
    assert!(matches!(error, LlmError::RateLimited { .. }));
    assert!(error.is_retryable());
}

#[tokio::test]
async fn retry_loop_stops_after_one_typed_quota_failure() {
    let mut calls = 0;
    let result: RetryResult<i32> = with_retry(
        &RetryConfig::default(),
        || {
            calls += 1;
            async {
                Err(LlmError::from_http_response(
                    429,
                    r#"{"error":{"code":"insufficient_quota"}}"#,
                ))
            }
        },
        None,
    )
    .await;
    assert_eq!(result.unwrap_err().attempts, 1);
    assert_eq!(calls, 1);
}

#[tokio::test]
async fn retry_loop_stops_after_one_authentication_failure() {
    let mut calls = 0;
    let result: RetryResult<i32> = with_retry(
        &RetryConfig::default(),
        || {
            calls += 1;
            async { Err(LlmError::authentication_error("bad key")) }
        },
        None,
    )
    .await;
    assert!(result.is_err());
    assert_eq!(calls, 1);
}
