//! The image proxy against a fake fetcher, and the real fetcher against a
//! local server.

use std::collections::HashMap;
use std::sync::Mutex;

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode, header};
use domain::NewTeam;
use http_body_util::BodyExt;
use store::users::User;
use tower::ServiceExt;

use super::*;
use crate::test_util::{ALICE, reconciler, t0, temp_db};

const PNG: &[u8] = b"\x89PNG\r\n\x1a\n-fake-png-1";
const PNG2: &[u8] = b"\x89PNG\r\n\x1a\n-fake-png-2";

/// Answers per URL; counts calls.
#[derive(Default)]
struct FakeFetcher {
    answers: Mutex<HashMap<String, Result<Vec<u8>, FetchError>>>,
    calls: Mutex<Vec<String>>,
}

impl FakeFetcher {
    fn answer(&self, url: &str, answer: Result<&[u8], FetchError>) {
        self.answers
            .lock()
            .unwrap()
            .insert(url.to_owned(), answer.map(<[u8]>::to_vec));
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait]
impl ImageFetcher for FakeFetcher {
    async fn fetch(&self, url: &Url) -> Result<Vec<u8>, FetchError> {
        self.calls.lock().unwrap().push(url.to_string());
        self.answers
            .lock()
            .unwrap()
            .get(url.as_str())
            .cloned()
            .unwrap_or(Err(FetchError::Status(404)))
    }
}

struct ImgApp {
    app: Router,
    pool: store::SqlitePool,
    fetcher: Arc<FakeFetcher>,
    cache: tempfile::TempDir,
    _db: tempfile::TempDir,
}

async fn img_app(max_age: Duration) -> ImgApp {
    let (pool, db) = temp_db().await;
    let cache = tempfile::tempdir().unwrap();
    let fetcher = Arc::new(FakeFetcher::default());
    let proxy = ImageProxy::new(cache.path().join("img"), fetcher.clone()).with_max_age(max_age);
    let state = AppState::new(reconciler(pool.clone(), &[]), Arc::new(domain::SystemClock))
        .with_images(Arc::new(proxy));
    ImgApp {
        app: crate::router(state, Path::new("/nonexistent")),
        pool,
        fetcher,
        cache,
        _db: db,
    }
}

async fn get(app: &Router, uri: &str) -> (StatusCode, HeaderMap, Vec<u8>) {
    let res = app
        .clone()
        .oneshot(Request::get(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let headers = res.headers().clone();
    (
        status,
        headers,
        res.into_body().collect().await.unwrap().to_bytes().to_vec(),
    )
}

async fn user(pool: &store::SqlitePool, id: &str, avatar: Option<&str>) {
    store::users::upsert_user(
        pool,
        &User {
            discord_user_id: id.to_owned(),
            username: "u".to_owned(),
            global_name: None,
            avatar_hash: avatar.map(str::to_owned),
            updated_at: t0(),
        },
    )
    .await
    .unwrap();
}

async fn team(pool: &store::SqlitePool, logo: Option<&str>) -> i64 {
    store::teams::insert(
        pool,
        &NewTeam {
            name: "Vitality".to_owned(),
            acronym: None,
            logo_url: logo.map(str::to_owned),
            country: None,
        },
    )
    .await
    .unwrap()
    .0
}

fn cached_files(dir: &Path) -> usize {
    std::fs::read_dir(dir.join("img")).map_or(0, |d| d.count())
}

#[tokio::test]
async fn avatars_are_fetched_once_then_served_from_disk() {
    let a = img_app(DEFAULT_MAX_AGE).await;
    user(&a.pool, ALICE, Some("abc123")).await;
    let upstream = format!("https://cdn.discordapp.com/avatars/{ALICE}/abc123.png?size=128");
    a.fetcher.answer(&upstream, Ok(PNG));

    for _ in 0..2 {
        let (status, headers, body) =
            get(&a.app, &format!("/api/img/avatar/{ALICE}?v=abc123")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, PNG);
        assert_eq!(headers[header::CONTENT_TYPE], "image/png");
        assert_eq!(headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
        assert_eq!(headers[header::CACHE_CONTROL], "public, max-age=86400");
        assert!(
            !headers.contains_key(header::ETAG),
            "images skip the JSON ETag layer"
        );
    }
    assert_eq!(a.fetcher.calls(), [upstream]);
    assert_eq!(cached_files(a.cache.path()), 1);
}

#[tokio::test]
async fn users_without_an_avatar_get_the_default_one() {
    let a = img_app(DEFAULT_MAX_AGE).await;
    user(&a.pool, ALICE, None).await;
    let index = (ALICE.parse::<u64>().unwrap() >> 22) % 6;
    let upstream = format!("https://cdn.discordapp.com/embed/avatars/{index}.png");
    a.fetcher.answer(&upstream, Ok(PNG));
    let (status, _, body) = get(&a.app, &format!("/api/img/avatar/{ALICE}")).await;
    assert_eq!((status, body.as_slice()), (StatusCode::OK, PNG));
    assert_eq!(a.fetcher.calls(), [upstream]);

    assert_eq!(
        discord_avatar(&(5u64 << 22).to_string(), None),
        "https://cdn.discordapp.com/embed/avatars/5.png"
    );
    assert_eq!(
        discord_avatar(&((6u64 << 22) | 123).to_string(), Some("../x")),
        "https://cdn.discordapp.com/embed/avatars/0.png",
        "an odd hash falls back to the default avatar"
    );
}

#[tokio::test]
async fn team_logos_follow_the_allow_list() {
    let a = img_app(DEFAULT_MAX_AGE).await;
    let logo = "https://cdn-api.pandascore.co/images/team/image/1/vitality.png";
    a.fetcher.answer(logo, Ok(PNG));
    let ok = team(&a.pool, Some(logo)).await;
    let (status, _, body) = get(&a.app, &format!("/api/img/team/{ok}")).await;
    assert_eq!((status, body.as_slice()), (StatusCode::OK, PNG));

    let evil = team(&a.pool, Some("https://evil.example/x.png")).await;
    let plain_http = team(&a.pool, Some("http://cdn-api.pandascore.co/x.png")).await;
    let other_port = team(&a.pool, Some("https://cdn-api.pandascore.co:8443/x.png")).await;
    let no_logo = team(&a.pool, None).await;
    for uri in [
        format!("/api/img/team/{evil}"),
        format!("/api/img/team/{plain_http}"),
        format!("/api/img/team/{other_port}"),
        format!("/api/img/team/{no_logo}"),
        "/api/img/team/999".to_owned(),
        "/api/img/avatar/999".to_owned(),
        "/api/img/avatar/not-an-id".to_owned(),
    ] {
        let (status, _, _) = get(&a.app, &uri).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
    }
    assert_eq!(a.fetcher.calls(), [logo], "nothing else was fetched");
}

#[tokio::test]
async fn anything_but_a_raster_image_is_rejected() {
    let a = img_app(DEFAULT_MAX_AGE).await;
    let svg = "https://cdn-api.pandascore.co/a.svg";
    let html = "https://cdn-api.pandascore.co/b.png";
    a.fetcher.answer(
        svg,
        Ok(b"<svg xmlns='http://www.w3.org/2000/svg'><script/></svg>"),
    );
    a.fetcher
        .answer(html, Ok(b"<!doctype html><script>alert(1)</script>"));
    for logo in [svg, html] {
        let id = team(&a.pool, Some(logo)).await;
        let (status, headers, _) = get(&a.app, &format!("/api/img/team/{id}")).await;
        assert_eq!(status, StatusCode::BAD_GATEWAY, "{logo}");
        assert_eq!(headers[header::CONTENT_TYPE], "application/json");
    }
    assert_eq!(cached_files(a.cache.path()), 0);
}

#[tokio::test]
async fn stale_copies_are_served_when_the_upstream_fails() {
    // Max age zero: every request refetches.
    let a = img_app(Duration::ZERO).await;
    let logo = "https://cdn-api.pandascore.co/images/team/image/1/v.png";
    let id = team(&a.pool, Some(logo)).await;
    let uri = format!("/api/img/team/{id}");

    a.fetcher.answer(logo, Ok(PNG));
    assert_eq!(get(&a.app, &uri).await.2, PNG);

    a.fetcher.answer(logo, Err(FetchError::Status(503)));
    let (status, _, body) = get(&a.app, &uri).await;
    assert_eq!(
        (status, body.as_slice()),
        (StatusCode::OK, PNG),
        "stale copy"
    );

    a.fetcher.answer(logo, Ok(PNG2));
    assert_eq!(get(&a.app, &uri).await.2, PNG2, "refreshed");
    assert_eq!(a.fetcher.calls().len(), 3);
    assert_eq!(cached_files(a.cache.path()), 1, "no temp files left");

    // Without any cached copy, a failure is a 502.
    let fresh = team(&a.pool, Some("https://cdn-api.pandascore.co/never.png")).await;
    let (status, _, _) = get(&a.app, &format!("/api/img/team/{fresh}")).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
}

#[test]
fn image_formats_are_sniffed_by_magic_bytes() {
    let mut webp = b"RIFF\0\0\0\0WEBPVP8 ".to_vec();
    webp.extend_from_slice(&[0; 8]);
    let cases: [(&[u8], Option<&str>); 9] = [
        (PNG, Some("image/png")),
        (&[0xFF, 0xD8, 0xFF, 0xE0], Some("image/jpeg")),
        (b"GIF89a....", Some("image/gif")),
        (b"GIF87a....", Some("image/gif")),
        (&webp, Some("image/webp")),
        (b"\0\0\0\x1cftypavif\0\0", Some("image/avif")),
        (b"<svg></svg>", None),
        (b"RIFF\0\0\0\0WAVE", None),
        (b"", None),
    ];
    for (bytes, expected) in cases {
        assert_eq!(sniff(bytes), expected, "{bytes:?}");
    }
}

#[test]
fn allow_list_and_public_urls() {
    for (url, allowed) in [
        ("https://cdn.discordapp.com/avatars/1/a.png", true),
        ("https://cdn-api.pandascore.co/x.png", true),
        ("http://cdn.discordapp.com/x.png", false),
        ("https://cdn.discordapp.com:444/x.png", false),
        ("https://cdn.discordapp.com.evil.example/x.png", false),
        ("https://user@evil.example/x.png", false),
        ("file:///etc/passwd", false),
    ] {
        assert_eq!(is_allowed(&Url::parse(url).unwrap()), allowed, "{url}");
    }

    assert_eq!(avatar_url("7", Some("abc")), "/api/img/avatar/7?v=abc");
    assert_eq!(avatar_url("7", None), "/api/img/avatar/7?v=0");
    let logo = team_logo_url(
        3,
        Some("https://cdn-api.pandascore.co/images/team/image/1/x.png"),
    )
    .unwrap();
    assert!(logo.starts_with("/api/img/team/3?v="), "{logo}");
    assert!(
        !logo.contains("pandascore") && !logo.contains("/1/"),
        "no external ID"
    );
    assert_eq!(team_logo_url(3, None), None);
}

/// The real fetcher, against a local server: redirects only to allowed
/// URLs, a size cap, and HTTP errors.
#[tokio::test]
async fn http_fetcher_limits_redirects_and_size() {
    use axum::response::Redirect;
    use axum::routing::get as route_get;

    let big = vec![0u8; MAX_IMAGE_BYTES + 1];
    let app = Router::new()
        .route("/ok", route_get(|| async { PNG }))
        .route("/big", route_get(move || async move { big.clone() }))
        .route("/to-ok", route_get(|| async { Redirect::temporary("/ok") }))
        .route(
            "/to-outside",
            route_get(|| async { Redirect::temporary("http://localhost:9/x.png") }),
        )
        .route(
            "/loop",
            route_get(|| async { Redirect::temporary("/loop") }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    fn local(url: &Url) -> bool {
        url.host_str() == Some("127.0.0.1")
    }
    let fetcher = HttpFetcher::with_allow(local).unwrap();
    let url = |path: &str| Url::parse(&format!("http://{addr}{path}")).unwrap();

    assert_eq!(fetcher.fetch(&url("/ok")).await.unwrap(), PNG);
    assert_eq!(fetcher.fetch(&url("/to-ok")).await.unwrap(), PNG);
    assert_eq!(fetcher.fetch(&url("/big")).await, Err(FetchError::TooLarge));
    assert_eq!(
        fetcher.fetch(&url("/nope")).await,
        Err(FetchError::Status(404))
    );
    for path in ["/to-outside", "/loop"] {
        assert!(
            matches!(fetcher.fetch(&url(path)).await, Err(FetchError::Other(_))),
            "{path}"
        );
    }
}
