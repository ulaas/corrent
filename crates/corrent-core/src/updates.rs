//! Opt-in update check against the project's GitHub releases. Never downloads or installs
//! anything: a newer release is shown as a toast with a link.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct UpdateSettings {
    /// Check once a day at startup.
    pub check_automatically: bool,
    pub last_check: i64,
    /// A version the user chose to skip.
    pub skip_version: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub version: String,
    pub url: String,
}

const RELEASES_API: &str = "https://api.github.com/repos/ulaas/corrent/releases/latest";

/// `v1.2.3` / `1.2.3-beta` → (1, 2, 3); missing parts are 0.
fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let core = v.trim().trim_start_matches(['v', 'V']).split(['-', '+']).next()?;
    let mut it = core.split('.').map(|p| p.parse::<u64>());
    let major = it.next()?.ok()?;
    let minor = it.next().unwrap_or(Ok(0)).ok()?;
    let patch = it.next().unwrap_or(Ok(0)).ok()?;
    Some((major, minor, patch))
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

pub fn parse_release(json: &str) -> Result<Release, String> {
    #[derive(Deserialize)]
    struct Gh {
        tag_name: String,
        html_url: String,
        #[serde(default)]
        draft: bool,
        #[serde(default)]
        prerelease: bool,
    }
    let g: Gh = serde_json::from_str(json).map_err(|e| t!("unexpected answer ({e})", e = e))?;
    if g.draft || g.prerelease {
        return Err("no stable release".into());
    }
    if !g.html_url.starts_with("https://github.com/") {
        return Err("unexpected release address".into());
    }
    Ok(Release { version: g.tag_name.trim_start_matches(['v', 'V']).to_string(), url: g.html_url })
}

/// The newest release if it is newer than this build.
pub async fn check(proxy: Option<String>) -> Result<Option<Release>, String> {
    check_at(&crate::rss::feed::client(proxy), RELEASES_API).await
}

/// `check` against `url` (tests point it elsewhere).
pub async fn check_at(client: &reqwest::Client, url: &str) -> Result<Option<Release>, String> {
    let mut resp = client
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| e.without_url().to_string())?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None); // no releases published yet
    }
    if !resp.status().is_success() {
        return Err(t!("GitHub answered {status}", status = resp.status()));
    }
    // GitHub's answer is a few KiB: more is not GitHub.
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| e.without_url().to_string())? {
        body.extend_from_slice(&chunk);
        if body.len() > 1 << 20 {
            return Err(t!("the answer is too large").into());
        }
    }
    let r = parse_release(&String::from_utf8_lossy(&body))?;
    Ok(is_newer(&r.version, env!("CARGO_PKG_VERSION")).then_some(r))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare() {
        assert!(is_newer("v0.2.0", "0.1.0"));
        assert!(is_newer("1.0", "0.9.9"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0-beta", "0.1.0"));
        assert!(!is_newer("garbage", "0.1.0"));
    }

    #[test]
    fn parses_github_release() {
        let r =
            parse_release(r#"{"tag_name":"v0.3.1","html_url":"https://github.com/ulaas/corrent/releases/tag/v0.3.1"}"#)
                .unwrap();
        assert_eq!(r.version, "0.3.1");
        assert!(parse_release(r#"{"tag_name":"v9","html_url":"https://evil.example/x"}"#).is_err());
        assert!(parse_release(r#"{"tag_name":"v9","html_url":"https://github.com/x","prerelease":true}"#).is_err());
    }
}
