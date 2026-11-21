//! End-to-end tests against the real `app` binary (T01 acceptance criteria).
//!
//! Each test runs the binary with a cleared environment and a temp working
//! directory, so a developer's `.env` or `config.toml` cannot leak in.

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_app");

fn base_command(cwd: &Path) -> Command {
    let mut cmd = Command::new(BIN);
    cmd.env_clear()
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd
}

fn full_env(cmd: &mut Command, dir: &Path, port: u16) {
    cmd.env("DISCORD_CLIENT_ID", "123456789012345678")
        .env("DISCORD_CLIENT_SECRET", "test-secret")
        .env("DISCORD_BOT_TOKEN", "test-bot-token")
        .env("PANDASCORE_TOKEN", "test-pandascore-token")
        .env(
            "DATABASE_URL",
            format!("sqlite://{}", dir.join("data/test.db").display()),
        )
        .env("BIND_ADDR", format!("127.0.0.1:{port}"))
        .env("STATIC_DIR", dir.join("dist"))
        .env("RUST_LOG", "info");
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Minimal HTTP/1.1 GET; returns (status line, body).
fn http_get(port: u16, path: &str) -> std::io::Result<(String, String)> {
    let mut stream = TcpStream::connect(("127.0.0.1", port))?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    )?;
    let mut raw = String::new();
    stream.read_to_string(&mut raw)?;
    let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((&raw, ""));
    let status = head.lines().next().unwrap_or_default().to_owned();
    Ok((status, body.to_owned()))
}

fn wait_for_health(child: &mut Child, port: u16) -> (String, String) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            panic!("app exited early with {status}");
        }
        if let Ok(res) = http_get(port, "/healthz") {
            return res;
        }
        assert!(Instant::now() < deadline, "app did not become healthy");
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn wait_with_deadline(mut child: Child, limit: Duration) -> (Output, Duration) {
    let start = Instant::now();
    loop {
        if child.try_wait().unwrap().is_some() {
            return (child.wait_with_output().unwrap(), start.elapsed());
        }
        if start.elapsed() > limit {
            let _ = child.kill();
            let out = child.wait_with_output().unwrap();
            panic!(
                "app did not exit within {limit:?}\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn missing_env_exits_1_and_names_variables() {
    let dir = tempfile::tempdir().unwrap();
    let out = base_command(dir.path()).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    for var in [
        "DISCORD_CLIENT_ID",
        "DISCORD_CLIENT_SECRET",
        "DISCORD_BOT_TOKEN",
        "DATABASE_URL",
        "PANDASCORE_TOKEN",
    ] {
        assert!(stderr.contains(var), "{var} not named in: {stderr}");
    }
}

#[test]
fn single_missing_env_is_named() {
    let dir = tempfile::tempdir().unwrap();
    let mut cmd = base_command(dir.path());
    full_env(&mut cmd, dir.path(), free_port());
    cmd.env_remove("DISCORD_BOT_TOKEN");
    let out = cmd.output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        stderr.trim(),
        "error: missing required environment variable(s): DISCORD_BOT_TOKEN"
    );
}

#[test]
fn serves_health_and_frontend_then_stops_cleanly_on_sigterm() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("dist")).unwrap();
    std::fs::write(
        dir.path().join("dist/index.html"),
        "<!doctype html><title>pickem test</title>",
    )
    .unwrap();

    let port = free_port();
    let mut cmd = base_command(dir.path());
    full_env(&mut cmd, dir.path(), port);
    let mut child = cmd.spawn().unwrap();

    // AC1: /healthz is 200 with version, git_sha and db: "ok".
    let (status, body) = wait_for_health(&mut child, port);
    assert!(status.contains(" 200 "), "status: {status}, body: {body}");
    for field in [r#""version":"#, r#""git_sha":"#, r#""db":"ok""#] {
        assert!(body.contains(field), "{field} missing in {body}");
    }

    // AC4: the built frontend is served at /.
    let (status, body) = http_get(port, "/").unwrap();
    assert!(status.contains(" 200 "), "status: {status}");
    assert!(body.contains("pickem test"));

    // T16: `app healthcheck` (the Docker HEALTHCHECK) succeeds with nothing
    // but BIND_ADDR, also on a wildcard bind address.
    let probe = base_command(dir.path())
        .arg("healthcheck")
        .env("BIND_ADDR", format!("0.0.0.0:{port}"))
        .output()
        .unwrap();
    assert!(
        probe.status.success(),
        "healthcheck failed: {}",
        String::from_utf8_lossy(&probe.stderr)
    );
    assert!(String::from_utf8_lossy(&probe.stdout).contains("200"));

    // AC3: SIGTERM ends the process within 10 s without errors.
    let killed = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(killed.success());
    let (out, elapsed) = wait_with_deadline(child, Duration::from_secs(10));
    let log = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "exit {:?}, log:\n{log}", out.status);
    assert!(elapsed < Duration::from_secs(10));
    assert!(!log.contains("ERROR"), "errors in log:\n{log}");
    assert!(log.contains("shutdown complete"), "log:\n{log}");
    // Secrets never reach the log.
    for secret in ["test-secret", "test-bot-token", "test-pandascore-token"] {
        assert!(!log.contains(secret), "{secret} leaked:\n{log}");
    }
}

/// T16: `app healthcheck` fails (exit 1) when nothing answers, without
/// needing any of the app's required configuration.
#[test]
fn healthcheck_fails_without_a_server() {
    let dir = tempfile::tempdir().unwrap();
    let out = base_command(dir.path())
        .args([
            "healthcheck",
            "--addr",
            &format!("127.0.0.1:{}", free_port()),
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("cannot connect"), "{stderr}");
    assert!(!stderr.contains("DISCORD_"), "{stderr}");
}

/// T03: `app reconcile --replay` rebuilds from `source_records` and exits,
/// without starting the server or calling any API.
#[test]
fn reconcile_replay_runs_and_exits() {
    let dir = tempfile::tempdir().unwrap();
    let mut cmd = base_command(dir.path());
    full_env(&mut cmd, dir.path(), free_port());
    let (out, _) = wait_with_deadline(
        cmd.args(["reconcile", "--replay"]).spawn().unwrap(),
        Duration::from_secs(10),
    );
    let log =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "exit {:?}, log:\n{log}", out.status);
    assert!(log.contains("replay finished"), "{log}");
    assert!(!log.contains("HTTP listening"), "{log}");
    assert!(dir.path().join("data/test.db").exists());

    // `reconcile` alone is a usage error.
    let mut cmd = base_command(dir.path());
    full_env(&mut cmd, dir.path(), free_port());
    let out = cmd.arg("reconcile").output().unwrap();
    assert_eq!(out.status.code(), Some(2), "clap usage error");
    assert!(String::from_utf8_lossy(&out.stderr).contains("--replay"));
}

/// T11: `app rules show|set` and `app rescore --all` run and exit without
/// starting the server.
#[test]
fn rules_and_rescore_run_and_exit() {
    let dir = tempfile::tempdir().unwrap();
    let guild = "100000000000000001";
    let run = |args: &[&str]| {
        let mut cmd = base_command(dir.path());
        full_env(&mut cmd, dir.path(), free_port());
        let (out, _) = wait_with_deadline(cmd.args(args).spawn().unwrap(), Duration::from_secs(10));
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let log = stdout.clone() + &String::from_utf8_lossy(&out.stderr);
        assert!(!log.contains("HTTP listening"), "{log}");
        (out.status, stdout, log)
    };

    let (status, stdout, log) = run(&["rules", "show", "--guild", guild]);
    assert!(status.success(), "{log}");
    assert!(stdout.contains("\"version\": 1"), "{stdout}");
    assert!(stdout.contains("\"map_score\": 2"), "{stdout}");

    let file = dir.path().join("rules.json");
    std::fs::write(&file, r#"{ "winner": 5 }"#).unwrap();
    let file = file.to_str().unwrap();
    let (status, stdout, log) = run(&["rules", "set", "--guild", guild, file]);
    assert!(status.success(), "{log}");
    assert!(stdout.contains("version 2"), "{stdout}");

    let (_, stdout, _) = run(&["rules", "show", "--guild", guild]);
    assert!(stdout.contains("\"version\": 2"), "{stdout}");
    assert!(stdout.contains("\"winner\": 5"), "{stdout}");

    // A typo in the file is rejected, and nothing changes.
    std::fs::write(dir.path().join("bad.json"), r#"{ "winer": 5 }"#).unwrap();
    let bad = dir.path().join("bad.json");
    let (status, _, log) = run(&["rules", "set", "--guild", guild, bad.to_str().unwrap()]);
    assert!(!status.success());
    assert!(log.contains("not a valid rules file"), "{log}");

    let (status, _, log) = run(&["rescore", "--all"]);
    assert!(status.success(), "{log}");
    assert!(log.contains("rescore finished"), "{log}");
}

/// T05: production builds have no dev endpoints, and refuse the fixture
/// source instead of silently running without it.
#[cfg(not(feature = "dev"))]
#[test]
fn production_build_has_no_dev_endpoints_or_fixture_source() {
    let dir = tempfile::tempdir().unwrap();
    let port = free_port();
    let mut cmd = base_command(dir.path());
    full_env(&mut cmd, dir.path(), port);
    let mut child = cmd.spawn().unwrap();
    wait_for_health(&mut child, port);
    let (status, body) = http_get(port, "/api/dev/clock").unwrap();
    let _ = child.kill();
    let _ = child.wait();
    assert!(status.contains(" 404 "), "status: {status}");
    assert!(body.contains(r#""error":"not_found""#), "{body}");

    let mut cmd = base_command(dir.path());
    full_env(&mut cmd, dir.path(), free_port());
    cmd.env("ENABLED_SOURCES", "fixture");
    let (out, _) = wait_with_deadline(cmd.spawn().unwrap(), Duration::from_secs(10));
    let log =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{log}");
    assert!(log.contains("--features dev"), "{log}");
    assert!(!log.contains("HTTP listening"), "{log}");
}

/// T05: a dev build with the fixture source loads the scenario and serves
/// the dev clock.
#[cfg(feature = "dev")]
#[test]
fn dev_build_serves_the_fixture_clock() {
    let dir = tempfile::tempdir().unwrap();
    let port = free_port();
    let mut cmd = base_command(dir.path());
    full_env(&mut cmd, dir.path(), port);
    cmd.env("ENABLED_SOURCES", "fixture");
    let mut child = cmd.spawn().unwrap();
    wait_for_health(&mut child, port);
    let (status, body) = http_get(port, "/api/dev/clock").unwrap();
    let _ = child.kill();
    let _ = child.wait();
    assert!(status.contains(" 200 "), "status: {status}");
    assert!(body.contains(r#""anchor":"#), "{body}");
}
