// Modified by the PdfCraft Overlay fork (sefr-14/pdfcraft); changes are recorded in its Git history.
//! Asks GitHub for this fork's latest available Windows build (Help ▸ Check for updates, OV8).
//!
//! The builds are artifacts of the manually run `overlay-windows.yml` workflow on `Main`, so a
//! new commit doesn't mean a new build, and a successful run doesn't mean its artifact can
//! still be downloaded. The check reads the five most recent successful runs and their
//! artifacts (public data: no account or token) and picks the newest artifact that hasn't
//! expired. It never falls back to upstream's releases and never downloads anything.

use std::time::{Duration, Instant};

use pdfcraft_ui_egui::updates::{Build, Builds, Utc};

const API: &str = "https://api.github.com/repos/sefr-14/pdfcraft";
const REPOSITORY: &str = "sefr-14/pdfcraft";
const WORKFLOW_PATH: &str = ".github/workflows/overlay-windows.yml";
const BRANCH: &str = "Main";
/// How many of the most recent successful runs are looked at (one request each, at most).
const RUNS_CHECKED: usize = 5;
/// The artifact names `overlay-windows.yml` gives its Windows x64 portable builds:
/// `pdfcraft-overlay-<workspace version>-<7-character commit>-windows-x64-portable`.
const NAME_PREFIX: &str = "pdfcraft-overlay-";
const NAME_SUFFIX: &str = "-windows-x64-portable";
const MAX_NAME: usize = 128;
/// Each answer is capped (five runs come to about 50 KB).
const MAX_BODY: u64 = 1 << 20;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// The whole check gives up after this, however many requests are left.
const CHECK_BUDGET: Duration = Duration::from_secs(30);
const UNEXPECTED: &str = "GitHub gave an answer PdfCraft didn't expect";

/// What came back for one request: its status, the rate-limit headers and (for a success) the
/// body.
#[derive(Clone, Debug, Default)]
struct Answer {
    status: u16,
    remaining: Option<String>,
    reset: Option<String>,
    retry_after: Option<String>,
    body: String,
}

/// Why a request got no answer at all.
#[derive(Clone, Debug)]
enum Failure {
    Timeout,
    Offline(String),
    Tls(String),
    Other(String),
}

/// The latest available build (see the module documentation).
pub fn latest_build() -> Result<Builds, String> {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(REQUEST_TIMEOUT))
        .tls_config(ureq::tls::TlsConfig::builder().root_certs(os_roots()?).build())
        .build()
        .new_agent();
    let started = Instant::now();
    let mut get = |url: &str| {
        if started.elapsed() > CHECK_BUDGET {
            return Err(Failure::Timeout);
        }
        request(&agent, url)
    };
    find_build(&mut get, now_unix())
}

fn request(agent: &ureq::Agent, url: &str) -> Result<Answer, Failure> {
    let mut response = agent
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .header("User-Agent", concat!("PdfCraft/", env!("CARGO_PKG_VERSION")))
        // Read error statuses ourselves: a 403 or 429 usually means the anonymous rate limit
        // ran out, and when it refills rides along in the headers.
        .config()
        .http_status_as_error(false)
        .build()
        .call()
        .map_err(failure)?;
    let status = response.status().as_u16();
    let header = |name: &str| response.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_string);
    let mut answer = Answer {
        status,
        remaining: header("x-ratelimit-remaining"),
        reset: header("x-ratelimit-reset"),
        retry_after: header("retry-after"),
        body: String::new(),
    };
    if (200..300).contains(&status) {
        answer.body = response.body_mut().with_config().limit(MAX_BODY).read_to_string().map_err(failure)?;
    }
    Ok(answer)
}

fn failure(e: ureq::Error) -> Failure {
    let text = e.to_string();
    match e {
        ureq::Error::Timeout(_) => Failure::Timeout,
        ureq::Error::Io(ref io) if io.kind() == std::io::ErrorKind::TimedOut => Failure::Timeout,
        ureq::Error::HostNotFound | ureq::Error::ConnectionFailed | ureq::Error::Io(_) => Failure::Offline(text),
        ureq::Error::Tls(_) | ureq::Error::Rustls(_) => Failure::Tls(text),
        _ => Failure::Other(text),
    }
}

/// The certificate authorities the operating system trusts.
fn os_roots() -> Result<ureq::tls::RootCerts, String> {
    let found = rustls_native_certs::load_native_certs();
    let certs: Vec<ureq::tls::Certificate<'static>> = found.certs.iter().map(|c| ureq::tls::Certificate::from_der(c.as_ref()).to_owned()).collect();
    if certs.is_empty() {
        return Err("no trusted certificates found on this system".into());
    }
    Ok(ureq::tls::RootCerts::new_with_certs(&certs))
}

/// The current Unix time in seconds (0 when the clock gives nothing usable).
fn now_unix() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// What to tell the user when GitHub's anonymous quota (60 requests an hour, shared by everyone
/// behind the same address) ran out. `retry_after` is the `retry-after` header, a wait in
/// seconds; `reset` the `x-ratelimit-reset` header, the Unix time the quota refills; `now` the
/// current Unix time. Anything surprising (no header, not a number, already past) falls back
/// to asking for patience.
fn rate_limited(retry_after: Option<&str>, reset: Option<&str>, now: u64) -> String {
    const FALLBACK: &str = "GitHub is limiting update checks for now (shared hourly quota); try again later";
    let number = |s: Option<&str>| s.and_then(|s| s.trim().parse::<u64>().ok());
    let wait = number(retry_after).or_else(|| number(reset).and_then(|reset| reset.checked_sub(now))).filter(|&wait| wait > 0);
    match wait {
        Some(wait) => {
            let minutes = wait.div_ceil(60).max(1);
            let plural = if minutes == 1 { "" } else { "s" };
            format!("GitHub is limiting update checks for now (shared hourly quota); try again in about {minutes} minute{plural}")
        }
        None => FALLBACK.into(),
    }
}

fn describe(f: Failure) -> String {
    match f {
        Failure::Timeout => "GitHub took too long to answer".into(),
        Failure::Offline(e) => format!("couldn't reach GitHub; check the network connection ({e})"),
        Failure::Tls(e) => format!("the secure connection to GitHub failed ({e})"),
        Failure::Other(e) => format!("couldn't reach GitHub ({e})"),
    }
}

/// One request's JSON, or why there is none. Every refusal is an error, never "no build".
fn fetch(get: &mut dyn FnMut(&str) -> Result<Answer, Failure>, url: &str, now: u64) -> Result<serde_json::Value, String> {
    let a = get(url).map_err(describe)?;
    let limited = || rate_limited(a.retry_after.as_deref(), a.reset.as_deref(), now);
    match a.status {
        200..=299 => serde_json::from_str(&a.body).map_err(|_| UNEXPECTED.to_string()),
        429 => Err(limited()),
        403 if a.remaining.as_deref().map(str::trim) == Some("0") || a.retry_after.is_some() => Err(limited()),
        401 | 403 | 404 => Err(format!("GitHub refused the request (http status: {})", a.status)),
        500..=599 => Err(format!("GitHub isn't answering right now (http status: {})", a.status)),
        s => Err(format!("couldn't reach GitHub (http status: {s})")),
    }
}

/// A successful run of the Windows workflow on `Main` in this repository.
struct Run {
    id: u64,
    created: Utc,
}

fn run(v: &serde_json::Value) -> Option<Run> {
    let s = |key: &str| v[key].as_str();
    let ours = |key: &str| v[key]["full_name"].as_str() == Some(REPOSITORY);
    let eligible = s("path") == Some(WORKFLOW_PATH)
        && s("head_branch") == Some(BRANCH)
        && s("status") == Some("completed")
        && s("conclusion") == Some("success")
        && ours("repository")
        && ours("head_repository");
    eligible.then_some(())?;
    Some(Run { id: v["id"].as_u64()?, created: Utc::parse(s("created_at")?)? })
}

/// What an artifact of run `run` is.
enum Artifact {
    /// A Windows portable build that can be downloaded.
    Usable(Build),
    /// A Windows portable build that has expired (or is about to be deleted).
    Expired,
    /// Something else: another kind of artifact, a malformed entry, or an empty upload.
    Other,
}

fn artifact(v: &serde_json::Value, run: u64, now: u64) -> Artifact {
    let Some(name) = v["name"].as_str() else { return Artifact::Other };
    let named = name.len() <= MAX_NAME
        && name.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
        && name.starts_with(NAME_PREFIX)
        && name.ends_with(NAME_SUFFIX);
    let from_run = v["workflow_run"]["id"].as_u64() == Some(run) && v["workflow_run"]["head_branch"].as_str() == Some(BRANCH);
    if !named || !from_run {
        return Artifact::Other;
    }
    let expires = v["expires_at"].as_str().and_then(Utc::parse).map(Utc::unix);
    let now = i64::try_from(now).unwrap_or(i64::MAX);
    if v["expired"].as_bool() != Some(false) || expires.is_none_or(|t| t <= now) {
        return Artifact::Expired;
    }
    match (v["created_at"].as_str().and_then(Utc::parse), v["size_in_bytes"].as_u64()) {
        (Some(built), Some(size)) if size > 0 => Artifact::Usable(Build { version: version(name), artifact: name.to_string(), built, run_id: run }),
        _ => Artifact::Other,
    }
}

/// The version in an artifact name: what lies between the prefix and `-<commit>` before the
/// suffix, if it reads as `x.y.z` (with an optional `-pre` part). Otherwise `None`.
fn version(name: &str) -> Option<String> {
    let middle = name.strip_prefix(NAME_PREFIX)?.strip_suffix(NAME_SUFFIX)?;
    let (version, commit) = middle.rsplit_once('-')?;
    let hex = (7..=40).contains(&commit.len()) && commit.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c));
    let (core, pre) = version.split_once('-').map_or((version, None), |(c, p)| (c, Some(p)));
    let numbers = core.split('.').collect::<Vec<_>>();
    let core_ok = numbers.len() == 3 && numbers.iter().all(|n| !n.is_empty() && n.len() <= 9 && n.bytes().all(|c| c.is_ascii_digit()));
    let pre_ok = pre.is_none_or(|p| !p.is_empty() && p.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'.'));
    (hex && core_ok && pre_ok).then(|| version.to_string())
}

/// The newest usable artifact of the most recent successful runs, newest run first.
fn find_build(get: &mut dyn FnMut(&str) -> Result<Answer, Failure>, now: u64) -> Result<Builds, String> {
    let runs_url =
        format!("{API}/actions/workflows/overlay-windows.yml/runs?branch={BRANCH}&status=success&exclude_pull_requests=true&per_page={RUNS_CHECKED}");
    let runs = fetch(get, &runs_url, now)?;
    let total = runs["total_count"].as_u64().ok_or(UNEXPECTED)?;
    let mut runs: Vec<Run> = runs["workflow_runs"].as_array().ok_or(UNEXPECTED)?.iter().filter_map(run).collect();
    runs.sort_by(|a, b| b.created.cmp(&a.created).then(b.id.cmp(&a.id)));
    runs.truncate(RUNS_CHECKED);
    let mut expired = false;
    for r in &runs {
        let answer = fetch(get, &format!("{API}/actions/runs/{}/artifacts?per_page=10", r.id), now)?;
        let mut newest: Option<Build> = None;
        for a in answer["artifacts"].as_array().ok_or(UNEXPECTED)? {
            match artifact(a, r.id, now) {
                Artifact::Usable(b) if newest.as_ref().is_none_or(|n| b.built > n.built) => newest = Some(b),
                Artifact::Expired => expired = true,
                _ => {}
            }
        }
        if let Some(b) = newest {
            return Ok(Builds::Found(b));
        }
    }
    let checked = runs.len();
    Ok(if expired { Builds::Expired { checked, total } } else { Builds::NoneFound { checked, total } })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::collections::HashMap;

    /// 21 Oct 2026 13:00 UTC.
    const NOW: u64 = 1_792_587_600;
    const RUNS: &str = "https://api.github.com/repos/sefr-14/pdfcraft/actions/workflows/overlay-windows.yml/runs?branch=Main&status=success&exclude_pull_requests=true&per_page=5";

    fn artifacts_url(run: u64) -> String {
        format!("https://api.github.com/repos/sefr-14/pdfcraft/actions/runs/{run}/artifacts?per_page=10")
    }

    fn a_run(id: u64, created: &str) -> Value {
        json!({
            "id": id, "path": ".github/workflows/overlay-windows.yml", "head_branch": "Main", "event": "workflow_dispatch",
            "status": "completed", "conclusion": "success", "created_at": created,
            "repository": {"full_name": "sefr-14/pdfcraft"}, "head_repository": {"full_name": "sefr-14/pdfcraft"},
            "html_url": format!("https://github.com/sefr-14/pdfcraft/actions/runs/{id}"),
        })
    }

    fn an_artifact(run: u64, name: &str, created: &str, expires: &str) -> Value {
        json!({
            "id": run + 1, "name": name, "size_in_bytes": 44_754_296, "expired": false,
            "created_at": created, "expires_at": expires,
            "workflow_run": {"id": run, "head_branch": "Main", "head_sha": "534043a16cf419ae34e500bab68f6dce10d27958"},
        })
    }

    /// Answers by URL, counting the requests.
    struct Fake {
        answers: HashMap<String, Result<Answer, Failure>>,
        asked: Vec<String>,
    }

    impl Fake {
        fn new() -> Self {
            Fake { answers: HashMap::new(), asked: Vec::new() }
        }
        fn json(mut self, url: &str, body: Value) -> Self {
            self.answers.insert(url.into(), Ok(Answer { status: 200, body: body.to_string(), ..Answer::default() }));
            self
        }
        fn runs(self, runs: Vec<Value>, total: u64) -> Self {
            self.json(RUNS, json!({"total_count": total, "workflow_runs": runs}))
        }
        fn artifacts(self, run: u64, artifacts: Vec<Value>) -> Self {
            let n = artifacts.len();
            self.json(&artifacts_url(run), json!({"total_count": n, "artifacts": artifacts}))
        }
        fn answer(mut self, url: &str, answer: Result<Answer, Failure>) -> Self {
            self.answers.insert(url.into(), answer);
            self
        }
        fn check(&mut self) -> Result<Builds, String> {
            let answers = &self.answers;
            let asked = &mut self.asked;
            let mut get = |url: &str| {
                asked.push(url.to_string());
                answers.get(url).cloned().unwrap_or_else(|| Err(Failure::Other(format!("unexpected request {url}"))))
            };
            find_build(&mut get, NOW)
        }
    }

    fn found(r: Result<Builds, String>) -> Build {
        match r {
            Ok(Builds::Found(b)) => b,
            other => panic!("expected a build, got {other:?}"),
        }
    }

    #[test]
    fn the_newest_usable_artifact_of_main_wins() {
        let mut fake = Fake::new()
            .runs(vec![a_run(2, "2026-10-20T20:28:24Z"), a_run(3, "2026-10-21T11:55:41Z")], 2)
            .artifacts(
                3,
                vec![an_artifact(3, "pdfcraft-overlay-0.10.1-6740201-windows-x64-portable", "2026-10-21T12:17:14Z", "2027-01-19T11:55:42Z")],
            )
            .artifacts(
                2,
                vec![an_artifact(2, "pdfcraft-overlay-0.9.0-30c1c1b-windows-x64-portable", "2026-10-20T20:59:00Z", "2027-01-18T20:28:24Z")],
            );
        let b = found(fake.check());
        assert_eq!(b.version.as_deref(), Some("0.10.1"));
        assert_eq!(b.artifact, "pdfcraft-overlay-0.10.1-6740201-windows-x64-portable");
        assert_eq!(b.built, Utc::parse("2026-10-21T12:17:14Z").unwrap());
        assert_eq!(b.run_url(), "https://github.com/sefr-14/pdfcraft/actions/runs/3");
        // The newest run had one: the older run wasn't asked about.
        assert_eq!(fake.asked, [RUNS.to_string(), artifacts_url(3)]);
    }

    #[test]
    fn a_newer_run_without_a_usable_artifact_falls_back_to_an_older_one() {
        let mut fake = Fake::new()
            .runs(vec![a_run(9, "2026-10-21T11:00:00Z"), a_run(8, "2026-10-20T11:00:00Z"), a_run(7, "2026-10-19T11:00:00Z")], 3)
            // No artifact at all, then only someone else's.
            .artifacts(9, vec![])
            .artifacts(8, vec![an_artifact(8, "pdfcraft-windows-arm64", "2026-10-20T11:30:00Z", "2026-10-27T11:00:00Z")])
            .artifacts(
                7,
                vec![an_artifact(7, "pdfcraft-overlay-0.9.0-ed5e5b5-windows-x64-portable", "2026-10-19T11:30:00Z", "2027-01-17T11:00:00Z")],
            );
        let b = found(fake.check());
        assert_eq!((b.run_id, b.version.as_deref()), (7, Some("0.9.0")));
        assert_eq!(fake.asked.len(), 4);
    }

    #[test]
    fn expired_artifacts_are_never_offered() {
        let mut gone = an_artifact(5, "pdfcraft-overlay-0.9.0-534043a-windows-x64-portable", "2026-07-01T00:00:00Z", "2026-09-29T00:00:00Z");
        gone["expired"] = json!(true);
        // Flagged as current but past its expiry: GitHub deletes these a little later.
        let late = an_artifact(4, "pdfcraft-overlay-0.9.0-30c1c1b-windows-x64-portable", "2026-07-01T00:00:00Z", "2026-10-21T12:59:59Z");
        let mut fake = Fake::new()
            .runs(vec![a_run(5, "2026-07-01T00:00:00Z"), a_run(4, "2026-06-30T00:00:00Z")], 2)
            .artifacts(5, vec![gone])
            .artifacts(4, vec![late]);
        assert_eq!(fake.check(), Ok(Builds::Expired { checked: 2, total: 2 }));
        // An empty upload isn't a build, but it isn't an expired one either.
        let mut empty = an_artifact(5, "pdfcraft-overlay-0.9.0-534043a-windows-x64-portable", "2026-10-21T00:00:00Z", "2027-01-19T00:00:00Z");
        empty["size_in_bytes"] = json!(0);
        let mut fake = Fake::new().runs(vec![a_run(5, "2026-10-21T00:00:00Z")], 1).artifacts(5, vec![empty]);
        assert_eq!(fake.check(), Ok(Builds::NoneFound { checked: 1, total: 1 }));
    }

    #[test]
    fn only_successful_completed_runs_of_this_workflow_on_main_count() {
        let mut runs = Vec::new();
        for (id, key, value) in [
            (11, "conclusion", json!("failure")),
            (12, "conclusion", json!("cancelled")),
            (13, "status", json!("in_progress")),
            (14, "head_branch", json!("main")),
            (15, "head_branch", json!("feat/x")),
            (16, "path", json!(".github/workflows/windows-arm64.yml")),
            (17, "head_repository", json!({"full_name": "someone/pdfcraft"})),
            (18, "repository", json!({"full_name": "storytold/pdfcraft"})),
            (19, "created_at", json!("yesterday")),
        ] {
            let mut r = a_run(id, "2026-10-21T00:00:00Z");
            r[key] = value;
            runs.push(r);
        }
        let mut fake = Fake::new().runs(runs, 9);
        // None is asked about: each is filtered out before its artifacts are.
        assert_eq!(fake.check(), Ok(Builds::NoneFound { checked: 0, total: 9 }));
        assert_eq!(fake.asked, [RUNS.to_string()]);
    }

    #[test]
    fn artifact_names_and_their_runs_are_checked() {
        let other_run = an_artifact(99, "pdfcraft-overlay-0.9.0-534043a-windows-x64-portable", "2026-10-21T00:00:00Z", "2027-01-19T00:00:00Z");
        let mut fake = Fake::new().runs(vec![a_run(6, "2026-10-21T00:00:00Z")], 1).artifacts(
            6,
            vec![
                an_artifact(6, "pdfcraft-overlay-0.9.0-534043a-windows-arm64-portable", "2026-10-21T00:00:00Z", "2027-01-19T00:00:00Z"),
                an_artifact(6, "pdfcraft-0.9.0-windows-x64-portable", "2026-10-21T00:00:00Z", "2027-01-19T00:00:00Z"),
                an_artifact(6, "pdfcraft-overlay-<script>-windows-x64-portable", "2026-10-21T00:00:00Z", "2027-01-19T00:00:00Z"),
                // Listed under this run but made by another.
                other_run,
            ],
        );
        assert_eq!(fake.check(), Ok(Builds::NoneFound { checked: 1, total: 1 }));
    }

    #[test]
    fn a_name_without_a_readable_version_is_shown_without_one() {
        assert_eq!(version("pdfcraft-overlay-0.10.1-6740201-windows-x64-portable").as_deref(), Some("0.10.1"));
        assert_eq!(version("pdfcraft-overlay-1.2.3-rc.1-6740201-windows-x64-portable").as_deref(), Some("1.2.3-rc.1"));
        for no_version in [
            "pdfcraft-overlay-nightly-6740201-windows-x64-portable",
            "pdfcraft-overlay-0.10-6740201-windows-x64-portable",
            "pdfcraft-overlay-0.10.1-674020-windows-x64-portable",
            "pdfcraft-overlay-0.10.1-674020G-windows-x64-portable",
            "pdfcraft-overlay-windows-x64-portable",
        ] {
            assert_eq!(version(no_version), None, "{no_version}");
        }
        let mut fake = Fake::new().runs(vec![a_run(6, "2026-10-21T00:00:00Z")], 1).artifacts(
            6,
            vec![an_artifact(6, "pdfcraft-overlay-nightly-6740201-windows-x64-portable", "2026-10-21T00:10:00Z", "2027-01-19T00:00:00Z")],
        );
        let b = found(fake.check());
        assert_eq!((b.version, b.artifact.as_str()), (None, "pdfcraft-overlay-nightly-6740201-windows-x64-portable"));
    }

    #[test]
    fn failures_are_errors_never_no_build() {
        let status = |status: u16| Ok(Answer { status, ..Answer::default() });
        for (answer, expected) in [
            (Err(Failure::Timeout), "GitHub took too long to answer"),
            (Err(Failure::Offline("dns error".into())), "couldn't reach GitHub; check the network connection (dns error)"),
            (Err(Failure::Tls("bad certificate".into())), "the secure connection to GitHub failed (bad certificate)"),
            (status(503), "GitHub isn't answering right now (http status: 503)"),
            (status(404), "GitHub refused the request (http status: 404)"),
            (status(403), "GitHub refused the request (http status: 403)"),
            (Ok(Answer { status: 200, body: "<html>".into(), ..Answer::default() }), UNEXPECTED),
            (Ok(Answer { status: 200, body: r#"{"message":"Not Found"}"#.into(), ..Answer::default() }), UNEXPECTED),
        ] {
            assert_eq!(Fake::new().answer(RUNS, answer).check(), Err(expected.to_string()));
        }
        // A failure on a run's artifacts is an error too, not a skipped run.
        let mut fake = Fake::new().runs(vec![a_run(6, "2026-10-21T00:00:00Z")], 1).answer(&artifacts_url(6), Err(Failure::Timeout));
        assert_eq!(fake.check(), Err("GitHub took too long to answer".into()));
    }

    #[test]
    fn rate_limits_name_the_wait() {
        let limited = |remaining: Option<&str>, retry_after: Option<&str>, status: u16| {
            Fake::new()
                .answer(
                    RUNS,
                    Ok(Answer {
                        status,
                        remaining: remaining.map(Into::into),
                        reset: Some((NOW + 1200).to_string()),
                        retry_after: retry_after.map(Into::into),
                        body: String::new(),
                    }),
                )
                .check()
        };
        let in_20 = "GitHub is limiting update checks for now (shared hourly quota); try again in about 20 minutes".to_string();
        assert_eq!(limited(Some("0"), None, 403), Err(in_20.clone()));
        assert_eq!(limited(None, None, 429), Err(in_20));
        // A secondary limit says how long to wait itself.
        assert_eq!(
            limited(Some("12"), Some("60"), 403),
            Err("GitHub is limiting update checks for now (shared hourly quota); try again in about 1 minute".into())
        );
        assert_eq!(
            rate_limited(None, Some("1800000061"), 1800000000),
            "GitHub is limiting update checks for now (shared hourly quota); try again in about 2 minutes"
        );
        let later = "GitHub is limiting update checks for now (shared hourly quota); try again later";
        for bad in [None, Some(""), Some("soon"), Some("1799999999"), Some("1800000000")] {
            assert_eq!(rate_limited(None, bad, 1800000000), later);
        }
    }

    #[test]
    fn at_most_five_runs_and_six_requests() {
        let runs: Vec<Value> = (1..=8).map(|id| a_run(id, &format!("2026-10-{:02}T00:00:00Z", 10 + id))).collect();
        let mut fake = Fake::new().runs(runs, 40);
        for id in 1..=8 {
            fake = fake.artifacts(id, vec![]);
        }
        assert_eq!(fake.check(), Ok(Builds::NoneFound { checked: 5, total: 40 }));
        assert_eq!(fake.asked.len(), 6);
        // Newest first: runs 8 to 4.
        assert_eq!(fake.asked.get(1), Some(&artifacts_url(8)));
        assert_eq!(fake.asked.last(), Some(&artifacts_url(4)));
    }

    #[test]
    fn a_hostile_html_url_is_ignored() {
        let mut r = a_run(6, "2026-10-21T00:00:00Z");
        r["html_url"] = json!("https://example.com/pdfcraft.exe");
        let mut fake = Fake::new().runs(vec![r], 1).artifacts(
            6,
            vec![an_artifact(6, "pdfcraft-overlay-0.10.1-6740201-windows-x64-portable", "2026-10-21T00:10:00Z", "2027-01-19T00:00:00Z")],
        );
        assert_eq!(found(fake.check()).run_url(), "https://github.com/sefr-14/pdfcraft/actions/runs/6");
    }

    #[test]
    fn upstream_releases_are_never_asked_about() {
        // The code, not these tests (one of which feeds in a run from upstream to refuse it).
        let code = |src: &'static str| src.split("#[cfg(test)]").next().unwrap_or(src);
        for (file, src) in
            [("updates.rs", include_str!("updates.rs")), ("ui-egui updates.rs", include_str!("../../../crates/ui-egui/src/updates.rs"))]
        {
            assert!(!code(src).contains(concat!("story", "told")), "{file} names upstream");
            assert!(!code(src).contains("/releases"), "{file} asks for releases");
        }
        assert!(RUNS.starts_with("https://api.github.com/repos/sefr-14/pdfcraft/"));
    }

    /// Live: asks GitHub over TLS with the OS's roots (`cargo test -p pdfcraft -- --ignored`).
    #[test]
    #[ignore = "needs network access"]
    fn github_answers_with_the_latest_build() {
        let r = latest_build().unwrap();
        assert!(matches!(r, Builds::Found(_) | Builds::NoneFound { .. } | Builds::Expired { .. }), "{r:?}");
    }
}
