//! Search backend selection and the shared async adapter contract.

use std::time::{Duration, Instant};

use async_trait::async_trait;

use super::contract::{BackendId, BackendSearch, DegradedReason, QueryCapabilities, SearchQuery};
use super::contract::{CapabilityState as QueryCapabilityState, SearchResult};
use crate::client::ProviderNativeSearchRequest;
use crate::config::SearchProvider;
use crate::tools::spec::{ToolContext, ToolError};

#[async_trait]
pub(crate) trait SearchBackend: Send + Sync {
    fn id(&self) -> BackendId;
    fn capabilities(&self) -> QueryCapabilities;
    async fn search(
        &self,
        query: &SearchQuery,
        deadline: Instant,
    ) -> Result<BackendSearch, ToolError>;
}

#[derive(Clone, Copy)]
pub(crate) struct BackendContext<'a> {
    tool_context: &'a ToolContext,
}

pub(crate) enum ConfiguredSearchBackend<'a> {
    Bing(BackendContext<'a>),
    DuckDuckGo(BackendContext<'a>),
    Firecrawl(BackendContext<'a>),
    Tavily(BackendContext<'a>),
    Bocha(BackendContext<'a>),
    Metaso(BackendContext<'a>),
    Searxng(BackendContext<'a>),
    Baidu(BackendContext<'a>),
    Volcengine(BackendContext<'a>),
    Sofya(BackendContext<'a>),
}

#[derive(Clone, Copy)]
struct ProviderNativeSearchBackend<'a> {
    context: &'a ToolContext,
}

impl<'a> ConfiguredSearchBackend<'a> {
    #[must_use]
    pub(crate) fn from_provider(context: &'a ToolContext, provider: SearchProvider) -> Self {
        let backend = BackendContext {
            tool_context: context,
        };
        match provider {
            SearchProvider::Bing => Self::Bing(backend),
            SearchProvider::DuckDuckGo => Self::DuckDuckGo(backend),
            SearchProvider::Firecrawl => Self::Firecrawl(backend),
            SearchProvider::Tavily => Self::Tavily(backend),
            SearchProvider::Bocha => Self::Bocha(backend),
            SearchProvider::Metaso => Self::Metaso(backend),
            SearchProvider::Searxng => Self::Searxng(backend),
            SearchProvider::Baidu => Self::Baidu(backend),
            SearchProvider::Volcengine => Self::Volcengine(backend),
            SearchProvider::Sofya => Self::Sofya(backend),
        }
    }

    const fn provider(&self) -> SearchProvider {
        match self {
            Self::Bing(_) => SearchProvider::Bing,
            Self::DuckDuckGo(_) => SearchProvider::DuckDuckGo,
            Self::Firecrawl(_) => SearchProvider::Firecrawl,
            Self::Tavily(_) => SearchProvider::Tavily,
            Self::Bocha(_) => SearchProvider::Bocha,
            Self::Metaso(_) => SearchProvider::Metaso,
            Self::Searxng(_) => SearchProvider::Searxng,
            Self::Baidu(_) => SearchProvider::Baidu,
            Self::Volcengine(_) => SearchProvider::Volcengine,
            Self::Sofya(_) => SearchProvider::Sofya,
        }
    }

    const fn context(&self) -> &BackendContext<'a> {
        match self {
            Self::Bing(context)
            | Self::DuckDuckGo(context)
            | Self::Firecrawl(context)
            | Self::Tavily(context)
            | Self::Bocha(context)
            | Self::Metaso(context)
            | Self::Searxng(context)
            | Self::Baidu(context)
            | Self::Volcengine(context)
            | Self::Sofya(context) => context,
        }
    }
}

pub(crate) struct SearchBackendChain<'a> {
    backends: Vec<Box<dyn SearchBackend + 'a>>,
}

#[derive(Debug)]
pub(crate) struct ChainedSearch {
    pub(crate) raw: BackendSearch,
    pub(crate) capabilities: QueryCapabilities,
}

impl<'a> SearchBackendChain<'a> {
    #[must_use]
    pub(crate) fn from_context(context: &'a ToolContext) -> Self {
        let selected = context.search_provider;
        let mut backends: Vec<Box<dyn SearchBackend + 'a>> = Vec::new();
        if should_prepend_provider_native(context) {
            backends.push(Box::new(ProviderNativeSearchBackend { context }));
        }
        backends.push(Box::new(ConfiguredSearchBackend::from_provider(
            context, selected,
        )));
        if !matches!(selected, SearchProvider::Bing | SearchProvider::DuckDuckGo) {
            backends.push(Box::new(ConfiguredSearchBackend::from_provider(
                context,
                SearchProvider::DuckDuckGo,
            )));
        }
        Self { backends }
    }

    #[must_use]
    pub(crate) fn initial_backend(&self) -> BackendId {
        self.backends
            .first()
            .expect("a search chain always has a configured backend")
            .id()
    }

    pub(crate) async fn search(
        &self,
        query: &SearchQuery,
        deadline: Instant,
        first_attempt_budget: Option<Duration>,
    ) -> Result<ChainedSearch, ToolError> {
        let backends = self
            .backends
            .iter()
            .map(|backend| backend.as_ref())
            .collect::<Vec<_>>();
        run_backend_chain(&backends, query, deadline, first_attempt_budget).await
    }
}

fn should_prepend_provider_native(context: &ToolContext) -> bool {
    provider_native_is_available(
        context
            .route_capabilities
            .server_side_web_search
            .is_supported(),
        context.provider_native_search.is_some(),
    )
}

const fn provider_native_is_available(capability_supported: bool, client_present: bool) -> bool {
    capability_supported && client_present
}

async fn run_backend_chain(
    backends: &[&dyn SearchBackend],
    query: &SearchQuery,
    deadline: Instant,
    first_attempt_budget: Option<Duration>,
) -> Result<ChainedSearch, ToolError> {
    let mut degraded = Vec::new();
    let mut last_empty = None;
    let mut attempted = Vec::new();

    for (index, backend) in backends.iter().enumerate() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        let backend_id = backend.id();
        if let Some(previous) = attempted.last() {
            degraded.push(DegradedReason::BackendFallback {
                from: *previous,
                to: backend_id,
            });
        }
        attempted.push(backend_id);

        let attempts_left = u32::try_from(backends.len() - index).unwrap_or(u32::MAX);
        let fair_share = remaining / attempts_left;
        let attempt_budget = if index == 0 {
            first_attempt_budget
                .map(|budget| budget.min(remaining))
                .unwrap_or(fair_share)
        } else {
            fair_share
        }
        .max(Duration::from_millis(1));
        let attempt_deadline = Instant::now() + attempt_budget;

        let result = tokio::time::timeout(attempt_budget, backend.search(query, attempt_deadline))
            .await
            .map_err(|_| ToolError::Timeout {
                seconds: u64::try_from(attempt_budget.as_millis())
                    .unwrap_or(u64::MAX)
                    .div_ceil(1_000),
            })
            .and_then(std::convert::identity);

        match result {
            Ok(mut raw) if !raw.results.is_empty() => {
                degraded.append(&mut raw.degraded);
                raw.degraded = degraded;
                return Ok(ChainedSearch {
                    raw,
                    capabilities: backend.capabilities(),
                });
            }
            Ok(mut raw) => {
                degraded.push(DegradedReason::NoUsableResults {
                    backend: backend_id,
                });
                degraded.append(&mut raw.degraded);
                last_empty = Some((raw, backend.capabilities()));
            }
            Err(error) if is_fail_closed(&error) => return Err(error),
            Err(error) if backends.len() == 1 => return Err(error),
            Err(_) => degraded.push(DegradedReason::BackendUnavailable {
                backend: backend_id,
            }),
        }
    }

    if let Some((mut raw, capabilities)) = last_empty {
        raw.degraded = degraded;
        return Ok(ChainedSearch { raw, capabilities });
    }

    if attempted.is_empty() {
        return Err(ToolError::Timeout { seconds: 1 });
    }

    let backend_ids = attempted
        .into_iter()
        .map(BackendId::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    Err(ToolError::not_available(format!(
        "web search backends unavailable: {backend_ids}"
    )))
}

const fn is_fail_closed(error: &ToolError) -> bool {
    matches!(
        error,
        ToolError::InvalidInput { .. }
            | ToolError::MissingField { .. }
            | ToolError::PathEscape { .. }
            | ToolError::Cancelled { .. }
            | ToolError::PermissionDenied { .. }
    )
}

#[async_trait]
impl SearchBackend for ConfiguredSearchBackend<'_> {
    fn id(&self) -> BackendId {
        match self.provider() {
            SearchProvider::Bing => BackendId::Bing,
            SearchProvider::DuckDuckGo => BackendId::DuckDuckGo,
            SearchProvider::Firecrawl => BackendId::Firecrawl,
            SearchProvider::Tavily => BackendId::Tavily,
            SearchProvider::Bocha => BackendId::Bocha,
            SearchProvider::Metaso => BackendId::Metaso,
            SearchProvider::Searxng => BackendId::Searxng,
            SearchProvider::Baidu => BackendId::Baidu,
            SearchProvider::Volcengine => BackendId::Volcengine,
            SearchProvider::Sofya => BackendId::Sofya,
        }
    }

    fn capabilities(&self) -> QueryCapabilities {
        // All current adapters enforce result count. Other knobs are either
        // post-filtered by the shared harness or reported as not honored.
        QueryCapabilities::count_only()
    }

    async fn search(
        &self,
        query: &SearchQuery,
        deadline: Instant,
    ) -> Result<BackendSearch, ToolError> {
        crate::tools::web_search::run_backend_search(
            self.provider(),
            query,
            deadline,
            self.context().tool_context,
        )
        .await
    }
}

#[async_trait]
impl SearchBackend for ProviderNativeSearchBackend<'_> {
    fn id(&self) -> BackendId {
        BackendId::ProviderNative
    }

    fn capabilities(&self) -> QueryCapabilities {
        QueryCapabilities {
            max_results: QueryCapabilityState::Supported,
            recency: QueryCapabilityState::Unsupported,
            domains: QueryCapabilityState::Supported,
            locale: QueryCapabilityState::Unsupported,
            published_date: QueryCapabilityState::Unknown,
        }
    }

    async fn search(
        &self,
        query: &SearchQuery,
        _deadline: Instant,
    ) -> Result<BackendSearch, ToolError> {
        if !self
            .context
            .route_capabilities
            .server_side_web_search
            .is_supported()
        {
            return Err(ToolError::not_available(
                "active route does not report provider-native web search",
            ));
        }
        let client = self
            .context
            .provider_native_search
            .as_ref()
            .ok_or_else(|| ToolError::not_available("provider-native search client unavailable"))?;
        if let Some(maximum) = client.maximum_domain_count()
            && query.domains.len() > maximum
        {
            return Err(ToolError::invalid_input(format!(
                "{} native web search accepts at most {maximum} domains",
                client.provider().as_str()
            )));
        }
        let host = client.host().ok_or_else(|| {
            ToolError::execution_failed("provider-native search endpoint has no valid host")
        })?;
        crate::tools::web_search::check_policy(
            self.context.network_policy.as_ref(),
            host.as_str(),
        )?;
        let response = client
            .search(&ProviderNativeSearchRequest {
                query: query.query.clone(),
                max_results: query.max_results,
                domains: query.domains.clone(),
            })
            .await
            .map_err(|error| {
                ToolError::execution_failed(format!(
                    "{} provider-native web search failed: {error}",
                    client.provider().as_str()
                ))
            })?;
        let results = response
            .citations
            .into_iter()
            .enumerate()
            .map(|(index, citation)| {
                SearchResult::new(
                    index + 1,
                    citation.title,
                    citation.url,
                    citation.snippet,
                    citation.published,
                )
            })
            .collect();
        Ok(BackendSearch {
            backend: BackendId::ProviderNative,
            source: format!(
                "provider-native/{}/{}",
                client.provider().as_str(),
                client.model()
            ),
            backend_detail: Some(host),
            results,
            degraded: Vec::new(),
            note: response.answer,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    struct FakeBackend {
        id: BackendId,
        result: Result<Vec<super::super::contract::SearchResult>, ToolError>,
    }

    #[async_trait]
    impl SearchBackend for FakeBackend {
        fn id(&self) -> BackendId {
            self.id
        }

        fn capabilities(&self) -> QueryCapabilities {
            QueryCapabilities::count_only()
        }

        async fn search(
            &self,
            _query: &SearchQuery,
            _deadline: Instant,
        ) -> Result<BackendSearch, ToolError> {
            Ok(BackendSearch {
                backend: self.id,
                source: self.id.as_str().to_string(),
                backend_detail: None,
                results: self.result.clone()?,
                degraded: Vec::new(),
                note: None,
            })
        }
    }

    fn query() -> SearchQuery {
        SearchQuery::new("bounded chain".to_string(), 5, None, Vec::new(), None)
    }

    fn result() -> super::super::contract::SearchResult {
        super::super::contract::SearchResult::new(
            1,
            "Fallback result".to_string(),
            "https://example.com/result".to_string(),
            None,
            None,
        )
    }

    #[test]
    fn every_configured_provider_maps_to_one_explicit_backend_adapter() {
        let cases = [
            (SearchProvider::Bing, BackendId::Bing),
            (SearchProvider::DuckDuckGo, BackendId::DuckDuckGo),
            (SearchProvider::Firecrawl, BackendId::Firecrawl),
            (SearchProvider::Tavily, BackendId::Tavily),
            (SearchProvider::Bocha, BackendId::Bocha),
            (SearchProvider::Metaso, BackendId::Metaso),
            (SearchProvider::Searxng, BackendId::Searxng),
            (SearchProvider::Baidu, BackendId::Baidu),
            (SearchProvider::Volcengine, BackendId::Volcengine),
            (SearchProvider::Sofya, BackendId::Sofya),
        ];

        for (provider, expected) in cases {
            let mut context = ToolContext::new(std::path::PathBuf::from("."));
            context.search_provider = provider;
            let backend = ConfiguredSearchBackend::from_provider(&context, provider);
            assert_eq!(backend.id(), expected);
            assert_eq!(
                backend.capabilities().max_results,
                super::super::contract::CapabilityState::Supported
            );
        }
    }

    #[test]
    fn provider_native_is_fail_closed_without_both_fact_and_client() {
        assert!(!provider_native_is_available(false, false));
        assert!(!provider_native_is_available(true, false));
        assert!(!provider_native_is_available(false, true));
        assert!(provider_native_is_available(true, true));
    }

    #[tokio::test]
    async fn unavailable_api_falls_back_with_explicit_receipts() {
        let api = FakeBackend {
            id: BackendId::Tavily,
            result: Err(ToolError::execution_failed(
                "provider detail must stay private",
            )),
        };
        let scrape = FakeBackend {
            id: BackendId::DuckDuckGo,
            result: Ok(vec![result()]),
        };
        let response = run_backend_chain(
            &[&api, &scrape],
            &query(),
            Instant::now() + Duration::from_secs(1),
            None,
        )
        .await
        .expect("fallback should succeed");

        assert_eq!(response.raw.backend, BackendId::DuckDuckGo);
        assert_eq!(
            response.raw.degraded,
            vec![
                DegradedReason::BackendUnavailable {
                    backend: BackendId::Tavily,
                },
                DegradedReason::BackendFallback {
                    from: BackendId::Tavily,
                    to: BackendId::DuckDuckGo,
                },
            ]
        );
    }

    #[tokio::test]
    async fn provider_native_to_api_to_scrape_records_every_transition() {
        let native = FakeBackend {
            id: BackendId::ProviderNative,
            result: Err(ToolError::execution_failed("native unavailable")),
        };
        let api = FakeBackend {
            id: BackendId::Tavily,
            result: Err(ToolError::execution_failed("API unavailable")),
        };
        let scrape = FakeBackend {
            id: BackendId::DuckDuckGo,
            result: Ok(vec![result()]),
        };

        let response = run_backend_chain(
            &[&native, &api, &scrape],
            &query(),
            Instant::now() + Duration::from_secs(1),
            None,
        )
        .await
        .expect("final scrape fallback should succeed");

        assert_eq!(response.raw.backend, BackendId::DuckDuckGo);
        assert_eq!(
            response.raw.degraded,
            vec![
                DegradedReason::BackendUnavailable {
                    backend: BackendId::ProviderNative,
                },
                DegradedReason::BackendFallback {
                    from: BackendId::ProviderNative,
                    to: BackendId::Tavily,
                },
                DegradedReason::BackendUnavailable {
                    backend: BackendId::Tavily,
                },
                DegradedReason::BackendFallback {
                    from: BackendId::Tavily,
                    to: BackendId::DuckDuckGo,
                },
            ]
        );
    }

    #[tokio::test]
    async fn first_attempt_budget_overrides_the_default_fair_share() {
        struct DeadlineBackend {
            observed_budget: Arc<Mutex<Option<Duration>>>,
        }

        #[async_trait]
        impl SearchBackend for DeadlineBackend {
            fn id(&self) -> BackendId {
                BackendId::Volcengine
            }

            fn capabilities(&self) -> QueryCapabilities {
                QueryCapabilities::count_only()
            }

            async fn search(
                &self,
                _query: &SearchQuery,
                deadline: Instant,
            ) -> Result<BackendSearch, ToolError> {
                *self.observed_budget.lock().expect("budget lock") =
                    Some(deadline.saturating_duration_since(Instant::now()));
                Ok(BackendSearch {
                    backend: BackendId::Volcengine,
                    source: "volcengine".to_string(),
                    backend_detail: None,
                    results: vec![result()],
                    degraded: Vec::new(),
                    note: None,
                })
            }
        }

        let observed_budget = Arc::new(Mutex::new(None));
        let volcengine = DeadlineBackend {
            observed_budget: Arc::clone(&observed_budget),
        };
        let fallback = FakeBackend {
            id: BackendId::DuckDuckGo,
            result: Ok(vec![result()]),
        };
        let first_attempt_budget = Duration::from_millis(1_500);
        let response = run_backend_chain(
            &[&volcengine, &fallback],
            &query(),
            Instant::now() + Duration::from_secs(2),
            Some(first_attempt_budget),
        )
        .await
        .expect("the first backend should complete inside its dedicated budget");

        assert_eq!(response.raw.backend, BackendId::Volcengine);
        let observed = observed_budget
            .lock()
            .expect("budget lock")
            .expect("first backend must observe a deadline");
        assert!(
            observed > Duration::from_millis(1_250),
            "dedicated first-attempt budget should exceed the default one-second fair share: {observed:?}"
        );
        assert!(observed <= first_attempt_budget);
    }

    #[tokio::test]
    async fn all_unavailable_returns_typed_error_with_backend_ids_only() {
        let private_error = "secret provider response";
        let api = FakeBackend {
            id: BackendId::Bocha,
            result: Err(ToolError::execution_failed(private_error)),
        };
        let scrape = FakeBackend {
            id: BackendId::DuckDuckGo,
            result: Err(ToolError::execution_failed("different private response")),
        };
        let error = run_backend_chain(
            &[&api, &scrape],
            &query(),
            Instant::now() + Duration::from_secs(1),
            None,
        )
        .await
        .expect_err("all-down chain must fail");
        let message = error.to_string();

        assert!(matches!(error, ToolError::NotAvailable { .. }));
        assert!(message.contains("bocha, duckduckgo"));
        assert!(!message.contains(private_error));
        assert!(!message.contains("different private response"));
    }

    #[tokio::test]
    async fn policy_failure_does_not_leak_query_to_fallback() {
        struct CountingBackend {
            calls: Arc<std::sync::atomic::AtomicUsize>,
        }
        #[async_trait]
        impl SearchBackend for CountingBackend {
            fn id(&self) -> BackendId {
                BackendId::DuckDuckGo
            }

            fn capabilities(&self) -> QueryCapabilities {
                QueryCapabilities::count_only()
            }

            async fn search(
                &self,
                _query: &SearchQuery,
                _deadline: Instant,
            ) -> Result<BackendSearch, ToolError> {
                self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Err(ToolError::execution_failed("unexpected fallback"))
            }
        }

        let api = FakeBackend {
            id: BackendId::Searxng,
            result: Err(ToolError::permission_denied("policy blocked")),
        };
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let scrape = CountingBackend {
            calls: Arc::clone(&calls),
        };
        let error = run_backend_chain(
            &[&api, &scrape],
            &query(),
            Instant::now() + Duration::from_secs(1),
            None,
        )
        .await
        .expect_err("policy error must fail closed");

        assert!(matches!(error, ToolError::PermissionDenied { .. }));
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn empty_api_falls_back_and_records_no_usable_results() {
        let api = FakeBackend {
            id: BackendId::Metaso,
            result: Ok(Vec::new()),
        };
        let scrape = FakeBackend {
            id: BackendId::DuckDuckGo,
            result: Ok(vec![result()]),
        };
        let response = run_backend_chain(
            &[&api, &scrape],
            &query(),
            Instant::now() + Duration::from_secs(1),
            None,
        )
        .await
        .expect("empty API response should fall back");

        assert_eq!(
            response.raw.degraded,
            vec![
                DegradedReason::NoUsableResults {
                    backend: BackendId::Metaso,
                },
                DegradedReason::BackendFallback {
                    from: BackendId::Metaso,
                    to: BackendId::DuckDuckGo,
                },
            ]
        );
    }
}
