//! One RFC 8628 device-authorization polling loop, shared by every Codewhale
//! device-code flow (xAI/Grok device login, Codewhale account login).
//!
//! Ported from pi (<https://github.com/badlogic/pi-mono>), MIT licensed,
//! Copyright (c) 2025 Mario Zechner — see
//! `packages/ai/src/auth/oauth/device-code.ts` for the original
//! `pollOAuthDeviceCodeFlow`. The accumulated behaviours carried over from it:
//!
//! * the RFC 8628 §3.2 default of 5 seconds when the server omits `interval`;
//! * `slow_down` handling that **prefers a server-supplied interval** over the
//!   client-tracked one. Trusting only the client-tracked value lets WSL/VM
//!   clock drift poll early forever; RFC 8628 §3.5's +5s step is the fallback;
//! * a hard deadline derived from `expires_in`, never slept past even after
//!   `slow_down` backoff;
//! * a distinct timeout message when at least one `slow_down` was seen, so the
//!   clock-drift case is diagnosable instead of looking like a plain timeout.
//!
//! The loop is generic over the poll result and does no I/O of its own: the
//! caller supplies the poll and the sleep. Nothing here ever holds, formats, or
//! logs a token — `T` is opaque to this module and is never `Debug`-printed.

use std::time::{Duration, Instant};

use anyhow::{Result, bail};

/// RFC 8628 §3.2: when the authorization server omits `interval`, clients must
/// poll no faster than every 5 seconds.
pub const DEFAULT_POLL_INTERVAL_SECS: u64 = 5;
/// RFC 8628 §3.5: `slow_down` increases the polling interval by 5 seconds.
pub const SLOW_DOWN_STEP_SECS: u64 = 5;
/// Never poll faster than once a second, whatever the server asks for.
const MINIMUM_INTERVAL: Duration = Duration::from_secs(1);

/// What one poll of the token endpoint told us.
///
/// A terminal failure is reported by returning `Err` from the poll closure, so
/// each provider keeps its own error text.
pub enum DevicePollOutcome<T> {
    /// The user approved; `T` is the provider's parsed token material.
    Complete(T),
    /// `authorization_pending` — keep the current interval.
    Pending,
    /// `slow_down` — back off. `interval_seconds` is the server's new minimum
    /// when it supplied one (preferred over the client-tracked interval).
    SlowDown { interval_seconds: Option<u64> },
}

/// A configured device-code polling run. Build one, then [`DeviceCodePoll::run`].
pub struct DeviceCodePoll {
    interval: Duration,
    max_interval: Option<Duration>,
    lifetime: Duration,
    wait_before_first_poll: bool,
    timeout_message: String,
    slow_down_timeout_message: Option<String>,
}

impl DeviceCodePoll {
    /// Start a run that gives up after `lifetime` with `timeout_message`.
    ///
    /// The interval starts at the RFC 8628 default of 5 seconds; callers pass
    /// the server's `interval` through [`DeviceCodePoll::interval_seconds`].
    #[must_use]
    pub fn new(lifetime: Duration, timeout_message: impl Into<String>) -> Self {
        Self {
            interval: Duration::from_secs(DEFAULT_POLL_INTERVAL_SECS),
            max_interval: None,
            lifetime,
            wait_before_first_poll: false,
            timeout_message: timeout_message.into(),
            slow_down_timeout_message: None,
        }
    }

    /// Apply the server-advertised `interval`. `None` (or a zero/absent value,
    /// which RFC 8628 permits) keeps the 5-second default.
    #[must_use]
    pub fn interval_seconds(mut self, seconds: Option<u64>) -> Self {
        if let Some(seconds) = seconds.filter(|seconds| *seconds > 0) {
            self.interval = self.clamp_interval(Duration::from_secs(seconds));
        }
        self
    }

    /// Cap the interval, including after `slow_down` backoff.
    #[must_use]
    pub fn max_interval_seconds(mut self, seconds: u64) -> Self {
        self.max_interval = Some(Duration::from_secs(seconds.max(1)));
        self.interval = self.clamp_interval(self.interval);
        self
    }

    /// Sleep one interval before the first poll.
    ///
    /// Device-code endpoints that answer `authorization_pending` (xAI) want
    /// this; endpoints whose first response is already meaningful (the
    /// Codewhale account service, which returns HTTP 202 while pending) poll
    /// immediately and sleep afterwards.
    #[must_use]
    pub fn wait_before_first_poll(mut self, wait: bool) -> Self {
        self.wait_before_first_poll = wait;
        self
    }

    /// Message used instead of the plain timeout message when the run saw at
    /// least one `slow_down`. This is the WSL/VM clock-drift tell.
    #[must_use]
    pub fn slow_down_timeout_message(mut self, message: impl Into<String>) -> Self {
        self.slow_down_timeout_message = Some(message.into());
        self
    }

    fn clamp_interval(&self, interval: Duration) -> Duration {
        let interval = interval.max(MINIMUM_INTERVAL);
        match self.max_interval {
            Some(max) => interval.min(max),
            None => interval,
        }
    }

    /// Poll until the flow completes, fails, or the deadline passes.
    ///
    /// `sleep` is injected so tests never wait in real time. `poll` returns
    /// `Err` for any terminal failure (denied, expired, transport error).
    pub fn run<T, S, P>(self, mut sleep: S, mut poll: P) -> Result<T>
    where
        S: FnMut(Duration),
        P: FnMut() -> Result<DevicePollOutcome<T>>,
    {
        let deadline = Instant::now() + self.lifetime;
        let mut interval = self.interval;
        let mut saw_slow_down = false;

        if self.wait_before_first_poll {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(self.timed_out(saw_slow_down));
            }
            sleep(interval.min(remaining));
        }

        while Instant::now() < deadline {
            match poll()? {
                DevicePollOutcome::Complete(value) => return Ok(value),
                DevicePollOutcome::Pending => {}
                DevicePollOutcome::SlowDown { interval_seconds } => {
                    saw_slow_down = true;
                    // Prefer the server's new minimum when it gave one: a
                    // purely client-tracked interval polls early forever when
                    // the clock drifts (WSL, suspended VMs).
                    interval = match interval_seconds.filter(|seconds| *seconds > 0) {
                        Some(seconds) => self.clamp_interval(Duration::from_secs(seconds)),
                        None => {
                            self.clamp_interval(interval + Duration::from_secs(SLOW_DOWN_STEP_SECS))
                        }
                    };
                }
            }

            // Never sleep past the code's expiry, even after slow_down backoff.
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            sleep(interval.min(remaining));
        }

        Err(self.timed_out(saw_slow_down))
    }

    fn timed_out(&self, saw_slow_down: bool) -> anyhow::Error {
        match (saw_slow_down, self.slow_down_timeout_message.as_deref()) {
            (true, Some(message)) => anyhow::anyhow!("{message}"),
            _ => anyhow::anyhow!("{}", self.timeout_message),
        }
    }
}

/// Reject a device-code verification URI that must not be handed to a browser
/// opener.
///
/// Ported from pi's `validateVerificationUri`
/// (`packages/ai/src/auth/oauth/xai.ts`, MIT, Copyright (c) 2025 Mario
/// Zechner): the URI comes straight off the wire and is passed to the platform
/// "open this" call, so a malicious or compromised response could otherwise
/// launch `file:`, a custom app scheme, or a helper with attacker-chosen
/// arguments. pi requires `https:`; Codewhale additionally allows `http:` on a
/// loopback host, which is what self-hosted issuers and the device-code tests
/// use — matching the loopback allowance the account login already makes.
///
/// Embedded credentials are rejected in every case.
pub fn validate_browser_verification_uri(raw: &str, context: &str) -> Result<String> {
    let trimmed = raw.trim();
    let Ok(url) = url_scheme_and_host(trimmed) else {
        bail!("{context} returned an unusable verification URI");
    };
    let (scheme, host, has_credentials) = url;
    if has_credentials {
        bail!("{context} returned a verification URI with embedded credentials");
    }
    let allowed = scheme == "https" || (scheme == "http" && is_loopback_host(&host));
    if !allowed {
        bail!("{context} returned an untrusted verification URI");
    }
    Ok(trimmed.to_string())
}

/// Minimal scheme/host/credential split, so this module stays free of a URL
/// dependency (`codewhale-config` deliberately has no `reqwest`/`url`).
fn url_scheme_and_host(raw: &str) -> Result<(String, String, bool), ()> {
    let (scheme, rest) = raw.split_once("://").ok_or(())?;
    if scheme.is_empty()
        || !scheme
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'-' || b == b'.')
    {
        return Err(());
    }
    let authority = rest
        .split(['/', '?', '#'])
        .next()
        .filter(|authority| !authority.is_empty())
        .ok_or(())?;
    let (credentials, hostport) = match authority.rsplit_once('@') {
        Some((credentials, hostport)) => (!credentials.is_empty(), hostport),
        None => (false, authority),
    };
    let host = match hostport.strip_prefix('[') {
        // IPv6 literal: [::1]:8080
        Some(rest) => rest.split_once(']').ok_or(())?.0.to_string(),
        None => hostport.split(':').next().ok_or(())?.to_string(),
    };
    if host.is_empty() {
        return Err(());
    }
    Ok((
        scheme.to_ascii_lowercase(),
        host.to_ascii_lowercase(),
        credentials,
    ))
}

fn is_loopback_host(host: &str) -> bool {
    if host == "localhost" || host == "::1" {
        return true;
    }
    host.parse::<std::net::IpAddr>()
        .is_ok_and(|address| address.is_loopback())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn recording_sleep(log: &RefCell<Vec<Duration>>) -> impl FnMut(Duration) + '_ {
        move |duration| log.borrow_mut().push(duration)
    }

    #[test]
    fn completes_on_first_poll_without_waiting() {
        let slept = RefCell::new(Vec::new());
        let value = DeviceCodePoll::new(Duration::from_secs(60), "timed out")
            .run(recording_sleep(&slept), || {
                Ok(DevicePollOutcome::Complete("token"))
            })
            .expect("first poll completes");
        assert_eq!(value, "token");
        assert!(slept.borrow().is_empty(), "no sleep before the first poll");
    }

    #[test]
    fn waits_one_interval_before_the_first_poll_when_asked() {
        let slept = RefCell::new(Vec::new());
        DeviceCodePoll::new(Duration::from_secs(60), "timed out")
            .interval_seconds(Some(3))
            .wait_before_first_poll(true)
            .run(recording_sleep(&slept), || {
                Ok(DevicePollOutcome::Complete(()))
            })
            .expect("completes after the initial wait");
        assert_eq!(slept.borrow().as_slice(), [Duration::from_secs(3)]);
    }

    #[test]
    fn omitted_interval_uses_the_rfc_default_of_five_seconds() {
        let slept = RefCell::new(Vec::new());
        let mut polls = 0;
        DeviceCodePoll::new(Duration::from_secs(600), "timed out")
            .interval_seconds(None)
            .run(recording_sleep(&slept), || {
                polls += 1;
                if polls == 1 {
                    Ok(DevicePollOutcome::Pending)
                } else {
                    Ok(DevicePollOutcome::Complete(()))
                }
            })
            .expect("completes");
        assert_eq!(slept.borrow().as_slice(), [Duration::from_secs(5)]);
    }

    #[test]
    fn slow_down_without_an_interval_adds_five_seconds() {
        let slept = RefCell::new(Vec::new());
        let mut polls = 0;
        DeviceCodePoll::new(Duration::from_secs(600), "timed out")
            .interval_seconds(Some(2))
            .run(recording_sleep(&slept), || {
                polls += 1;
                match polls {
                    1 => Ok(DevicePollOutcome::Pending),
                    2 => Ok(DevicePollOutcome::SlowDown {
                        interval_seconds: None,
                    }),
                    _ => Ok(DevicePollOutcome::Complete(())),
                }
            })
            .expect("completes");
        assert_eq!(
            slept.borrow().as_slice(),
            [Duration::from_secs(2), Duration::from_secs(7)]
        );
    }

    #[test]
    fn slow_down_prefers_a_server_supplied_interval() {
        // The clock-drift fix: the server's new minimum wins over the
        // client-tracked interval, in both directions.
        let slept = RefCell::new(Vec::new());
        let mut polls = 0;
        DeviceCodePoll::new(Duration::from_secs(600), "timed out")
            .interval_seconds(Some(2))
            .run(recording_sleep(&slept), || {
                polls += 1;
                match polls {
                    1 => Ok(DevicePollOutcome::SlowDown {
                        interval_seconds: Some(30),
                    }),
                    _ => Ok(DevicePollOutcome::Complete(())),
                }
            })
            .expect("completes");
        assert_eq!(slept.borrow().as_slice(), [Duration::from_secs(30)]);
    }

    #[test]
    fn interval_never_drops_below_one_second_or_exceeds_the_cap() {
        let slept = RefCell::new(Vec::new());
        let mut polls = 0;
        DeviceCodePoll::new(Duration::from_secs(600), "timed out")
            .interval_seconds(Some(0))
            .max_interval_seconds(10)
            .run(recording_sleep(&slept), || {
                polls += 1;
                match polls {
                    1 => Ok(DevicePollOutcome::SlowDown {
                        interval_seconds: Some(99),
                    }),
                    _ => Ok(DevicePollOutcome::Complete(())),
                }
            })
            .expect("completes");
        // interval 0 falls back to the RFC default (5s), capped at 10s.
        assert_eq!(slept.borrow().as_slice(), [Duration::from_secs(10)]);
    }

    #[test]
    fn never_sleeps_past_the_deadline() {
        let slept = RefCell::new(Vec::new());
        let error = DeviceCodePoll::new(Duration::from_millis(30), "timed out")
            .interval_seconds(Some(600))
            .run(
                |duration| {
                    slept.borrow_mut().push(duration);
                    std::thread::sleep(duration);
                },
                || Ok(DevicePollOutcome::<()>::Pending),
            )
            .expect_err("deadline stops the loop");
        assert_eq!(error.to_string(), "timed out");
        for duration in slept.borrow().iter() {
            assert!(
                *duration <= Duration::from_millis(30),
                "slept {duration:?} past a 30ms deadline"
            );
        }
    }

    #[test]
    fn a_terminal_poll_error_stops_immediately() {
        let slept = RefCell::new(Vec::new());
        let error = DeviceCodePoll::new(Duration::from_secs(600), "timed out")
            .run(recording_sleep(&slept), || {
                Err::<DevicePollOutcome<()>, _>(anyhow::anyhow!("access_denied"))
            })
            .expect_err("terminal errors propagate");
        assert_eq!(error.to_string(), "access_denied");
        assert!(slept.borrow().is_empty());
    }

    #[test]
    fn timing_out_after_slow_down_reports_the_clock_drift_message() {
        let error = DeviceCodePoll::new(Duration::from_millis(5), "plain timeout")
            .interval_seconds(Some(1))
            .slow_down_timeout_message("clock drift timeout")
            .run(std::thread::sleep, || {
                Ok(DevicePollOutcome::<()>::SlowDown {
                    interval_seconds: None,
                })
            })
            .expect_err("deadline stops the loop");
        assert_eq!(error.to_string(), "clock drift timeout");
    }

    #[test]
    fn timing_out_without_slow_down_reports_the_plain_message() {
        let error = DeviceCodePoll::new(Duration::from_millis(5), "plain timeout")
            .interval_seconds(Some(1))
            .slow_down_timeout_message("clock drift timeout")
            .run(std::thread::sleep, || Ok(DevicePollOutcome::<()>::Pending))
            .expect_err("deadline stops the loop");
        assert_eq!(error.to_string(), "plain timeout");
    }

    #[test]
    fn verification_uri_must_be_https_or_loopback_http() {
        assert_eq!(
            validate_browser_verification_uri("https://accounts.x.ai/device", "xAI").unwrap(),
            "https://accounts.x.ai/device"
        );
        assert!(validate_browser_verification_uri("http://127.0.0.1:8080/verify", "xAI").is_ok());
        assert!(validate_browser_verification_uri("http://localhost/verify", "xAI").is_ok());
        assert!(validate_browser_verification_uri("http://[::1]:9/verify", "xAI").is_ok());

        for hostile in [
            "http://accounts.x.ai/device",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "vscode://attacker/run",
            "data:text/html,<script>",
            "https://",
            "not a url",
            "",
        ] {
            assert!(
                validate_browser_verification_uri(hostile, "xAI").is_err(),
                "accepted {hostile}"
            );
        }
    }

    #[test]
    fn verification_uri_rejects_embedded_credentials() {
        let error =
            validate_browser_verification_uri("https://user:pass@accounts.x.ai/device", "xAI")
                .expect_err("credentials must be rejected");
        assert!(error.to_string().contains("embedded credentials"));
    }
}
