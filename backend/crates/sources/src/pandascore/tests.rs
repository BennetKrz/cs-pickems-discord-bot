//! HTTP behavior against a local mock server: auth, filters, batching,
//! pagination, quota headers, the hard cap and 429 handling. No real API
//! is called, except by the ignored `live_smoke` test.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::Request;
use axum::http::{Response, StatusCode};
use domain::{BracketSide, FeedOutcome};
use reqwest::Url;
use serde_json::{Value, json};

use super::*;
use crate::SrcFeed;

const TOKEN: &str = "test-token";

#[derive(Debug, Clone)]
struct Req {
    path: String,
    query: Vec<(String, String)>,
    auth: Option<String>,
}

impl Req {
    fn param(&self, key: &str) -> Option<&str> {
        self.query
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

type Responder = dyn Fn(&Req, usize) -> Response<Body> + Send + Sync;

struct Mock {
    base: String,
    log: Arc<Mutex<Vec<Req>>>,
}

impl Mock {
    async fn start(
        responder: impl Fn(&Req, usize) -> Response<Body> + Send + Sync + 'static,
    ) -> Self {
        let log: Arc<Mutex<Vec<Req>>> = Arc::default();
        let responder: Arc<Responder> = Arc::new(responder);
        let app = {
            let log = log.clone();
            axum::Router::new().fallback(move |req: Request| {
                let log = log.clone();
                let responder = responder.clone();
                async move {
                    let url = Url::parse(&format!("http://mock{}", req.uri())).unwrap();
                    let r = Req {
                        path: url.path().to_owned(),
                        query: url.query_pairs().into_owned().collect(),
                        auth: req
                            .headers()
                            .get("authorization")
                            .map(|v| v.to_str().unwrap().to_owned()),
                    };
                    let n = {
                        let mut log = log.lock().unwrap();
                        log.push(r.clone());
                        log.len() - 1
                    };
                    responder(&r, n)
                }
            })
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self {
            base: format!("http://{addr}"),
            log,
        }
    }

    fn requests(&self) -> Vec<Req> {
        self.log.lock().unwrap().clone()
    }

    fn adapter(&self) -> PandascoreAdapter {
        self.adapter_with(|_| {})
    }

    fn adapter_with(&self, tweak: impl FnOnce(&mut PandascoreConfig)) -> PandascoreAdapter {
        let mut config = PandascoreConfig {
            base_url: self.base.clone(),
            retry: RetryPolicy {
                max_retries: 3,
                base_delay: Duration::from_millis(10),
                max_delay: Duration::from_millis(80),
            },
            ..PandascoreConfig::new(TOKEN)
        };
        tweak(&mut config);
        PandascoreAdapter::new(config).unwrap()
    }
}

fn ok(body: &Value) -> Response<Body> {
    ok_with(body, &[])
}

fn ok_with(body: &Value, headers: &[(&str, &str)]) -> Response<Body> {
    let mut b = Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/json");
    for (k, v) in headers {
        b = b.header(*k, *v);
    }
    b.body(Body::from(body.to_string())).unwrap()
}

fn status(code: StatusCode, headers: &[(&str, &str)]) -> Response<Body> {
    let mut b = Response::builder().status(code);
    for (k, v) in headers {
        b = b.header(*k, *v);
    }
    b.body(Body::from(r#"{"error":"x"}"#)).unwrap()
}

fn fixture(text: &str) -> Value {
    serde_json::from_str(text).unwrap()
}

fn serie() -> Value {
    fixture(include_str!("../../fixtures/pandascore/serie.json"))
}

#[tokio::test]
async fn fetch_matches_sends_token_and_batched_id_filter() {
    let bo3 = fixture(include_str!("../../fixtures/pandascore/match_bo3.json"));
    let bo1 = fixture(include_str!("../../fixtures/pandascore/match_bo1.json"));
    let body = json!([bo3, bo1]);
    let mock = Mock::start(move |_, _| ok(&body)).await;

    let ids = ["1814195", "not-a-number", "1814117", "1814195"].map(String::from);
    let got = mock.adapter().fetch_matches(&ids).await.unwrap();

    let reqs = mock.requests();
    assert_eq!(reqs.len(), 1);
    let r = &reqs[0];
    assert_eq!(r.path, "/csgo/matches");
    assert_eq!(r.auth.as_deref(), Some("Bearer test-token"));
    assert_eq!(r.param("filter[id]"), Some("1814117,1814195"));
    assert_eq!(r.param("per_page"), Some("100"));

    let ids: Vec<_> = got.iter().map(|m| m.dto.ext_id.as_str()).collect();
    assert_eq!(ids, ["1814195", "1814117"]);
    assert_eq!(got[0].raw["id"], 1814195, "raw is the PandaScore object");
    assert_eq!(got[0].dto.bracket, None);
}

#[tokio::test]
async fn fetch_matches_batches_by_100() {
    let mock = Mock::start(|_, _| ok(&json!([]))).await;
    let ids: Vec<String> = (1..=150).map(|i| i.to_string()).collect();
    let adapter = mock.adapter();
    assert_eq!(adapter.cost(CallKind::FetchMatches, ids.len()), 2);
    adapter.fetch_matches(&ids).await.unwrap();

    let reqs = mock.requests();
    assert_eq!(reqs.len(), 2);
    assert_eq!(reqs[0].param("filter[id]").unwrap().split(',').count(), 100);
    assert_eq!(reqs[1].param("filter[id]").unwrap().split(',').count(), 50);

    // No IDs, no request.
    adapter.fetch_live(&[]).await.unwrap();
    assert_eq!(mock.requests().len(), 2);
    assert_eq!(adapter.cost(CallKind::FetchLive, 0), 0);
}

#[tokio::test]
async fn fetch_live_uses_running_route() {
    let running = fixture(include_str!("../../fixtures/pandascore/match_running.json"));
    let body = json!([running]);
    let mock = Mock::start(move |_, _| ok(&body)).await;
    let got = mock
        .adapter()
        .fetch_live(&["1814195".into()])
        .await
        .unwrap();
    assert_eq!(mock.requests()[0].path, "/csgo/matches/running");
    assert_eq!(got[0].dto.status, Some(domain::MatchStatus::Live));
}

#[tokio::test]
async fn event_matches_follow_pages_and_merge_bracket_links() {
    let all = fixture(include_str!("../../fixtures/pandascore/serie_matches.json"));
    let brackets = fixture(include_str!("../../fixtures/pandascore/brackets.json"));
    let all = all.as_array().unwrap().clone();
    let (page1, page2) = (json!(all[..5]), json!(all[5..]));
    let mock = Mock::start(move |r, _| match (r.path.as_str(), r.param("page")) {
        ("/csgo/matches", None) => ok_with(
            &page1,
            &[(
                "link",
                // PandaScore's absolute URL; the client re-bases it.
                "<https://api.pandascore.co/csgo/matches?filter%5Bserie_id%5D=11212&page=2&per_page=100&sort=id>; rel=\"next\", \
                 <https://api.pandascore.co/csgo/matches?filter%5Bserie_id%5D=11212&page=2&per_page=100&sort=id>; rel=\"last\"",
            )],
        ),
        ("/csgo/matches", Some("2")) => ok(&page2),
        ("/tournaments/22466/brackets", _) => ok(&brackets),
        _ => status(StatusCode::NOT_FOUND, &[]),
    })
    .await;

    let got = mock.adapter().fetch_event_matches("11212").await.unwrap();
    let reqs = mock.requests();
    let paths: Vec<_> = reqs.iter().map(|r| r.path.as_str()).collect();
    assert_eq!(
        paths,
        [
            "/csgo/matches",
            "/csgo/matches",
            "/tournaments/22466/brackets"
        ]
    );
    assert_eq!(reqs[0].param("filter[serie_id]"), Some("11212"));
    assert_eq!(reqs[1].param("page"), Some("2"));
    assert!(
        reqs.iter()
            .all(|r| r.auth.as_deref() == Some("Bearer test-token"))
    );

    assert_eq!(got.len(), 9);
    let gf = got.iter().find(|m| m.dto.ext_id == "1814198").unwrap();
    let b = gf.dto.bracket.as_ref().unwrap();
    assert_eq!(b.side, Some(BracketSide::Final));
    assert_eq!(
        b.feeds_a,
        Some(SrcFeed {
            match_ext_id: "1814196".into(),
            outcome: FeedOutcome::Winner
        })
    );
    assert_eq!(gf.raw["previous_matches"].as_array().unwrap().len(), 2);
    let qf = got.iter().find(|m| m.dto.ext_id == "1814192").unwrap();
    assert_eq!(qf.dto.bracket, Some(crate::SrcBracket::default()));
}

#[tokio::test]
async fn fetch_event_and_search() {
    let serie = serie();
    let search = fixture(include_str!("../../fixtures/pandascore/series_search.json"));
    let mock = Mock::start(move |r, _| match r.path.as_str() {
        "/series/11137" => ok(&serie),
        "/csgo/series" => ok(&search),
        _ => status(StatusCode::NOT_FOUND, &[]),
    })
    .await;
    let adapter = mock.adapter();

    let e = adapter.fetch_event("11137").await.unwrap();
    assert_eq!(e.dto.name, "BLAST Rivals Fall 2026");
    assert_eq!(e.dto.stages.len(), 3);

    let found = adapter.search_events("BLAST Rivals!").await.unwrap();
    assert_eq!(found.len(), 4);
    assert!(
        found
            .iter()
            .all(|e| e.dto.tier.is_some() && e.dto.begin_at.is_some())
    );
    let r = &mock.requests()[1];
    assert_eq!(r.param("search[slug]"), Some("blast-rivals"));
    assert_eq!(r.param("sort"), Some("-begin_at"));

    // Empty queries are not sent.
    assert!(adapter.search_events(" ?! ").await.unwrap().is_empty());
    assert_eq!(mock.requests().len(), 2);

    assert!(matches!(
        adapter.fetch_event("404").await,
        Err(SourceError::NotFound(_))
    ));
    assert!(matches!(
        adapter.fetch_event("abc").await,
        Err(SourceError::NotFound(_))
    ));
    assert_eq!(mock.requests().len(), 3, "a non-numeric ID is not sent");
}

#[tokio::test]
async fn non_cs_serie_is_not_found() {
    let mut serie = serie();
    serie["videogame"]["slug"] = "valorant".into();
    let mock = Mock::start(move |_, _| ok(&serie)).await;
    assert!(matches!(
        mock.adapter().fetch_event("11137").await,
        Err(SourceError::NotFound(_))
    ));
}

#[tokio::test]
async fn error_statuses_are_mapped() {
    let mock = Mock::start(|r, _| match r.path.as_str() {
        "/series/1" => status(StatusCode::UNAUTHORIZED, &[]),
        "/series/2" => status(StatusCode::FORBIDDEN, &[]),
        "/series/3" => status(StatusCode::INTERNAL_SERVER_ERROR, &[]),
        _ => ok_with(&json!({"not": "a serie"}), &[]),
    })
    .await;
    let a = mock.adapter();
    let err = |r: SourceResult<_>| match r {
        Err(SourceError::Http(msg)) => msg,
        other => panic!("expected Http error, got {other:?}"),
    };
    assert!(err(a.fetch_event("1").await).contains("PANDASCORE_TOKEN"));
    assert!(err(a.fetch_event("2").await).contains("plan"));
    assert!(err(a.fetch_event("3").await).contains("500"));
    assert!(matches!(
        a.fetch_event("4").await,
        Err(SourceError::Decode(_))
    ));
    // Errors are not retried.
    assert_eq!(mock.requests().len(), 4);
}

#[tokio::test]
async fn quota_is_read_from_headers() {
    let serie = serie();
    let mock = Mock::start(move |_, _| {
        ok_with(
            &serie,
            &[
                ("x-rate-limit-remaining", "42"),
                ("x-rate-limit-used", "958"),
            ],
        )
    })
    .await;
    let a = mock.adapter();
    assert_eq!(a.quota(), None);
    a.fetch_event("11137").await.unwrap();
    assert_eq!(
        a.quota(),
        Some(Quota {
            remaining: 42,
            used: Some(958)
        })
    );
}

#[tokio::test]
async fn remaining_zero_stops_further_requests() {
    let serie = serie();
    let mock = Mock::start(move |_, _| ok_with(&serie, &[("x-rate-limit-remaining", "0")])).await;
    let a = mock.adapter();
    a.fetch_event("11137").await.unwrap();
    match a.fetch_event("11137").await {
        Err(SourceError::RateLimited {
            retry_after: Some(d),
        }) => assert!(d <= Duration::from_secs(60)),
        other => panic!("expected RateLimited, got {other:?}"),
    }
    assert_eq!(mock.requests().len(), 1);
}

#[tokio::test]
async fn retries_429_with_backoff_then_succeeds() {
    let serie = serie();
    let mock = Mock::start(move |_, n| {
        if n < 2 {
            status(StatusCode::TOO_MANY_REQUESTS, &[])
        } else {
            ok(&serie)
        }
    })
    .await;
    let start = Instant::now();
    mock.adapter().fetch_event("11137").await.unwrap();
    assert_eq!(mock.requests().len(), 3);
    // Retry 0 waits ≥ 5 ms, retry 1 ≥ 10 ms (half of 10 and 20 ms).
    assert!(start.elapsed() >= Duration::from_millis(15));
}

#[tokio::test]
async fn gives_up_after_max_retries() {
    let mock = Mock::start(|_, _| status(StatusCode::TOO_MANY_REQUESTS, &[])).await;
    let r = mock.adapter().fetch_event("11137").await;
    assert!(matches!(
        r,
        Err(SourceError::RateLimited { retry_after: None })
    ));
    assert_eq!(mock.requests().len(), 4, "1 attempt + 3 retries");
}

#[tokio::test]
async fn honors_retry_after() {
    let serie = serie();
    let mock = Mock::start(move |r, n| match (r.path.as_str(), n) {
        ("/series/1", 0) => status(StatusCode::TOO_MANY_REQUESTS, &[("retry-after", "1")]),
        ("/series/2", _) => status(StatusCode::TOO_MANY_REQUESTS, &[("retry-after", "120")]),
        _ => ok(&serie),
    })
    .await;
    let a = mock.adapter_with(|c| c.retry.max_delay = Duration::from_secs(2));

    let start = Instant::now();
    a.fetch_event("1").await.unwrap();
    assert!(start.elapsed() >= Duration::from_secs(1));
    assert_eq!(mock.requests().len(), 2);

    // Longer than the policy allows: no waiting, the scheduler decides.
    let start = Instant::now();
    let r = a.fetch_event("2").await;
    assert!(matches!(
        r,
        Err(SourceError::RateLimited { retry_after: Some(d) }) if d == Duration::from_secs(120)
    ));
    assert!(start.elapsed() < Duration::from_secs(1));
    assert_eq!(mock.requests().len(), 3);
}

#[tokio::test]
async fn never_exceeds_the_per_minute_cap() {
    let serie = serie();
    let mock = Mock::start(move |_, _| ok(&serie)).await;
    let a = mock.adapter();
    assert_eq!(a.limits(), FREE_PLAN);
    for _ in 0..60 {
        a.fetch_event("11137").await.unwrap();
    }
    for _ in 0..5 {
        assert!(matches!(
            a.fetch_event("11137").await,
            Err(SourceError::RateLimited {
                retry_after: Some(_)
            })
        ));
    }
    assert_eq!(mock.requests().len(), 60);
}

#[tokio::test]
async fn never_exceeds_the_hourly_cap() {
    let serie = serie();
    let mock = Mock::start(move |_, _| ok(&serie)).await;
    let a = mock.adapter_with(|c| {
        c.limits = RateLimits {
            per_minute: 60,
            per_hour: 5,
        }
    });
    for _ in 0..5 {
        a.fetch_event("11137").await.unwrap();
    }
    match a.fetch_event("11137").await {
        Err(SourceError::RateLimited {
            retry_after: Some(d),
        }) => assert!(d > Duration::from_secs(3500)),
        other => panic!("expected RateLimited, got {other:?}"),
    }
    assert_eq!(mock.requests().len(), 5);
}

#[tokio::test]
async fn retries_count_against_the_cap() {
    let mock = Mock::start(|_, _| status(StatusCode::TOO_MANY_REQUESTS, &[])).await;
    let a = mock.adapter_with(|c| {
        c.limits = RateLimits {
            per_minute: 2,
            per_hour: 100,
        }
    });
    let r = a.fetch_event("11137").await;
    assert!(matches!(
        r,
        Err(SourceError::RateLimited {
            retry_after: Some(_)
        })
    ));
    assert_eq!(mock.requests().len(), 2);
}

#[test]
fn declared_capabilities_and_costs() {
    let a = PandascoreAdapter::new(PandascoreConfig::new("t")).unwrap();
    assert_eq!(a.id(), "pandascore");
    assert!(a.supports(Capability::Results));
    assert!(a.supports(Capability::Brackets));
    assert!(
        !a.supports(Capability::MapScores),
        "no round scores on the free plan"
    );
    assert!(
        !a.supports(Capability::MapNames),
        "no map names on the free plan"
    );
    assert_eq!(
        a.limits(),
        RateLimits {
            per_minute: 60,
            per_hour: 1000
        }
    );
    assert_eq!(a.cost(CallKind::SearchEvents, 0), 1);
    assert_eq!(a.cost(CallKind::FetchEvent, 0), 1);
    assert_eq!(a.cost(CallKind::FetchEventMatches, 0), 3);
    assert_eq!(a.cost(CallKind::FetchMatches, 1), 1);
    assert_eq!(a.cost(CallKind::FetchMatches, 101), 2);
}

#[test]
fn debug_output_hides_the_token() {
    let c = PandascoreConfig::new("super-secret");
    assert!(!format!("{c:?}").contains("super-secret"));
    let a = PandascoreAdapter::new(c).unwrap();
    assert!(!format!("{a:?}").contains("super-secret"));
}

/// Calls the real API (about 3 requests). Never runs in CI:
/// `PANDASCORE_TOKEN=… cargo test -p sources --features pandascore -- --ignored live_smoke`
#[tokio::test]
#[ignore = "calls the real PandaScore API"]
async fn live_smoke() {
    let token = std::env::var("PANDASCORE_TOKEN").expect("PANDASCORE_TOKEN");
    let a = PandascoreAdapter::new(PandascoreConfig::new(token)).unwrap();
    let found = a.search_events("blast").await.unwrap();
    assert!(!found.is_empty());
    let first = &found[0].dto;
    let event = a.fetch_event(&first.ext_id).await.unwrap();
    assert_eq!(event.dto.ext_id, first.ext_id);
    let matches = a.fetch_event_matches(&first.ext_id).await.unwrap();
    println!(
        "{}: {} stages, {} matches, quota {:?}",
        event.dto.name,
        event.dto.stages.len(),
        matches.len(),
        a.quota()
    );
}
