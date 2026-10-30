//! The two downloads of an update (UPD.1): the signed channel envelope and the package. HTTPS
//! with the operating system's trusted roots; redirects only within the update host. Anything
//! else — Cloudflare Access's sign-in page, another host, an HTML answer — is reported as such
//! and never parsed as update data. No credentials are ever sent.

use std::io::Read;
use std::path::Path;
use std::time::Duration;

use pdfcraft_overlay_update::envelope::MAX_ENVELOPE;
use pdfcraft_overlay_update::manifest::Package;
use pdfcraft_ui_egui::updates::Progress;

/// At most this many redirects, each within the update host.
const MAX_REDIRECTS: usize = 3;
const SIGN_IN: &str = "The update server asked for sign-in or refused access";

/// Why a request got no usable answer.
#[derive(Clone, Debug)]
enum Failure {
    Timeout,
    Offline(String),
    Tls(String),
    Other(String),
}

fn describe(f: Failure) -> String {
    match f {
        Failure::Timeout => "the update server took too long to answer".into(),
        Failure::Offline(e) => format!("couldn't reach the update server; check the network connection ({e})"),
        Failure::Tls(e) => format!("the secure connection to the update server failed ({e})"),
        Failure::Other(e) => format!("couldn't reach the update server ({e})"),
    }
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

fn agent(body_timeout: Duration) -> Result<ureq::Agent, String> {
    Ok(ureq::Agent::config_builder()
        .https_only(true)
        // Redirects are followed by hand, only within the update host.
        .max_redirects(0)
        .max_redirects_will_error(false)
        .http_status_as_error(false)
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .timeout_recv_body(Some(body_timeout))
        .user_agent(concat!("PDF-Overlay-Updater/", env!("CARGO_PKG_VERSION")))
        .tls_config(ureq::tls::TlsConfig::builder().root_certs(os_roots()?).build())
        .build()
        .new_agent())
}

/// Where a redirect may go: an absolute path on the same host, or `https://<host>/…`.
pub(crate) fn redirect_target(host: &str, location: &str) -> Result<String, String> {
    if location.starts_with('/') && !location.starts_with("//") {
        return Ok(format!("https://{host}{location}"));
    }
    let rest = location.strip_prefix("https://").ok_or_else(|| format!("{SIGN_IN} (redirected away from HTTPS)"))?;
    let target_host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if target_host.eq_ignore_ascii_case(host) { Ok(location.to_string()) } else { Err(format!("{SIGN_IN} (redirected to {target_host})")) }
}

/// GET `url`, following redirects within `host`; the response once it is a 2xx with a non-HTML body.
fn get(agent: &ureq::Agent, host: &str, url: &str) -> Result<ureq::http::Response<ureq::Body>, String> {
    let mut url = url.to_string();
    for _ in 0..=MAX_REDIRECTS {
        let response = agent.get(&url).header("Cache-Control", "no-cache").call().map_err(|e| describe(failure(e)))?;
        let status = response.status().as_u16();
        match status {
            200..=299 => {
                let html = response
                    .headers()
                    .get("content-type")
                    .and_then(|v| v.to_str().ok())
                    .is_some_and(|t| t.to_ascii_lowercase().contains("text/html"));
                if html {
                    return Err(format!("{SIGN_IN} (the server answered with a web page)"));
                }
                return Ok(response);
            }
            300..=399 => {
                let location = response
                    .headers()
                    .get("location")
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(|| format!("{SIGN_IN} (redirect without a location)"))?;
                url = redirect_target(host, location)?;
            }
            401 | 403 => return Err(format!("{SIGN_IN} (http status {status})")),
            404 => return Err("no approved update is published right now (http status 404); try again later".into()),
            429 | 500..=599 => return Err(format!("the update server isn't answering right now (http status {status})")),
            s => return Err(format!("couldn't reach the update server (http status {s})")),
        }
    }
    Err(format!("{SIGN_IN} (too many redirects)"))
}

/// The signed channel envelope's bytes (at most [`MAX_ENVELOPE`]).
pub fn fetch_envelope(host: &str, url: &str) -> Result<Vec<u8>, String> {
    let agent = agent(Duration::from_secs(30))?;
    let mut response = get(&agent, host, url)?;
    let mut bytes = Vec::new();
    response
        .body_mut()
        .with_config()
        .limit(MAX_ENVELOPE as u64 + 1)
        .reader()
        .take(MAX_ENVELOPE as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| describe(Failure::Other(e.to_string())))?;
    if bytes.len() > MAX_ENVELOPE {
        return Err("the update information is larger than expected".into());
    }
    Ok(bytes)
}

/// Download `package` into `to` (created fresh), checking its signed size and hash while it
/// streams. A failure leaves a partial file for the caller to remove.
pub fn download(host: &str, package: &Package, to: &Path, progress: &Progress) -> Result<(), String> {
    use std::sync::atomic::Ordering;
    let agent = agent(Duration::from_secs(20 * 60))?;
    let mut response = get(&agent, host, &package.url)?;
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(to).map_err(|e| format!("couldn't create {}: {e}", to.display()))?;
    let mut reader = response.body_mut().with_config().limit(package.size.saturating_add(1)).reader();
    progress.total.store(package.size, Ordering::Relaxed);
    pdfcraft_overlay_update::hash::copy_verified(&mut reader, &mut file, package.size, &package.sha256, &mut |done| {
        progress.done.store(done, Ordering::Relaxed);
        !progress.cancel.load(Ordering::Relaxed)
    })
    .map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| format!("couldn't write {}: {e}", to.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirects_stay_on_the_update_host() {
        let host = "git.ferrive.me";
        assert_eq!(redirect_target(host, "/api/packages/x").unwrap(), "https://git.ferrive.me/api/packages/x");
        assert_eq!(redirect_target(host, "https://GIT.ferrive.me/a").unwrap(), "https://GIT.ferrive.me/a");
        for away in [
            "https://ferrive.cloudflareaccess.com/cdn-cgi/access/login",
            "http://git.ferrive.me/a",
            "//evil.example/a",
            "https://git.ferrive.me.evil.example/a",
            "https://evil.example?git.ferrive.me",
            "ftp://git.ferrive.me/a",
        ] {
            let e = redirect_target(host, away).unwrap_err();
            assert!(e.starts_with(SIGN_IN), "{away}: {e}");
        }
    }
}
