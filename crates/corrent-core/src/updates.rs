//! The update check against the project's GitHub releases. On by default: once a day Corrent
//! asks GitHub's public release API for the latest release (one anonymous HTTPS request, through
//! the user's proxy if one is set; it carries Corrent's version in the User-Agent and nothing that
//! identifies the user or the profile). A newer release is announced; nothing is downloaded
//! until the user asks for it (`self_update` does that part).

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct UpdateSettings {
    /// Ask GitHub for the latest release once a day.
    pub check_automatically: bool,
    pub last_check: i64,
    /// A version the user chose to skip.
    pub skip_version: String,
}

impl Default for UpdateSettings {
    fn default() -> Self {
        Self { check_automatically: true, last_check: 0, skip_version: String::new() }
    }
}

/// How long the automatic check waits after the last one.
pub const CHECK_EVERY_SECS: i64 = 24 * 3600;

impl UpdateSettings {
    /// Whether the automatic check is due at `now` (Unix seconds).
    pub fn due(&self, now: i64) -> bool {
        // A clock set back must not silence the check for ever.
        self.check_automatically && (now - self.last_check > CHECK_EVERY_SECS || self.last_check > now)
    }
}

/// A file attached to a release.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
    /// The SHA-256 GitHub computed for the file (lower-case hex), when it gives one.
    pub sha256: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub version: String,
    /// The release's page, for people.
    pub url: String,
    pub assets: Vec<Asset>,
}

/// Where releases are asked for and what their addresses must start with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    /// The "latest release" API address.
    pub api: String,
    /// Release pages start with this.
    pub pages: String,
    /// Downloads start with this; an asset anywhere else is not offered.
    pub downloads: String,
}

impl Source {
    /// The project's releases on GitHub.
    pub fn github() -> Source {
        Source {
            api: "https://api.github.com/repos/ulaas/corrent/releases/latest".into(),
            pages: "https://github.com/ulaas/corrent/".into(),
            downloads: "https://github.com/ulaas/corrent/releases/download/".into(),
        }
    }
}

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

/// `sha256:<64 hex digits>` → the digits in lower case.
fn sha256_digest(d: &str) -> Option<String> {
    let h = d.strip_prefix("sha256:")?;
    (h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit())).then(|| h.to_ascii_lowercase())
}

pub fn parse_release(json: &str) -> Result<Release, String> {
    parse_release_from(json, &Source::github())
}

/// `parse_release` for releases published at `source`.
pub fn parse_release_from(json: &str, source: &Source) -> Result<Release, String> {
    #[derive(Deserialize)]
    struct GhAsset {
        name: String,
        browser_download_url: String,
        #[serde(default)]
        size: u64,
        #[serde(default)]
        digest: Option<String>,
    }
    #[derive(Deserialize)]
    struct Gh {
        tag_name: String,
        html_url: String,
        #[serde(default)]
        draft: bool,
        #[serde(default)]
        prerelease: bool,
        #[serde(default)]
        assets: Vec<GhAsset>,
    }
    let g: Gh = serde_json::from_str(json).map_err(|e| t!("unexpected answer ({e})", e = e))?;
    if g.draft || g.prerelease {
        return Err("no stable release".into());
    }
    if !g.html_url.starts_with(&source.pages) {
        return Err("unexpected release address".into());
    }
    let version = g.tag_name.trim_start_matches(['v', 'V']).to_string();
    // The version becomes part of file names: digits, letters, dots and dashes only.
    let plain = !version.is_empty()
        && version.len() <= 40
        && version.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'));
    if !plain || parse_version(&version).is_none() {
        return Err("unexpected version".into());
    }
    let assets = g
        .assets
        .into_iter()
        .filter_map(|a| {
            // Only the project's own downloads, and a checksum GitHub gives must be one that can
            // be checked: an asset with another kind of digest is not offered at all.
            let sha256 = match a.digest.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
                Some(d) => Some(sha256_digest(d)?),
                None => None,
            };
            a.browser_download_url.starts_with(&source.downloads).then_some(Asset {
                name: a.name,
                url: a.browser_download_url,
                size: a.size,
                sha256,
            })
        })
        .collect();
    Ok(Release { version, url: g.html_url, assets })
}

/// The newest release if it is newer than this build.
pub async fn check(proxy: Option<String>) -> Result<Option<Release>, String> {
    check_at(&crate::rss::feed::client(proxy), &Source::github()).await
}

/// `check` against `source` (tests point it elsewhere).
pub async fn check_at(client: &reqwest::Client, source: &Source) -> Result<Option<Release>, String> {
    let mut resp = client
        .get(&source.api)
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
    // GitHub's answer is some tens of KiB (it lists every file of the release): more is not GitHub.
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| e.without_url().to_string())? {
        body.extend_from_slice(&chunk);
        if body.len() > 1 << 20 {
            return Err(t!("the answer is too large").into());
        }
    }
    let r = parse_release_from(&String::from_utf8_lossy(&body), source)?;
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
        assert!(r.assets.is_empty());
        assert!(parse_release(r#"{"tag_name":"v9","html_url":"https://evil.example/x"}"#).is_err());
        assert!(parse_release(r#"{"tag_name":"v9","html_url":"https://github.com/someone/else"}"#).is_err());
        assert!(
            parse_release(r#"{"tag_name":"v9","html_url":"https://github.com/ulaas/corrent/x","prerelease":true}"#)
                .is_err()
        );
        // The version goes into file names.
        assert!(parse_release(r#"{"tag_name":"v9/../x","html_url":"https://github.com/ulaas/corrent/x"}"#).is_err());
    }

    #[test]
    fn only_the_projects_own_downloads_are_offered_with_their_checksums() {
        let d = "https://github.com/ulaas/corrent/releases/download/v9.0.0/";
        let json = format!(
            r#"{{"tag_name":"v9.0.0","html_url":"https://github.com/ulaas/corrent/releases/tag/v9.0.0","assets":[
                {{"name":"a.zip","browser_download_url":"{d}a.zip","size":12,"digest":"sha256:{h}"}},
                {{"name":"b.zip","browser_download_url":"{d}b.zip","size":3,"digest":null}},
                {{"name":"c.zip","browser_download_url":"https://example.com/c.zip","size":3}},
                {{"name":"d.zip","browser_download_url":"http://github.com/ulaas/corrent/releases/download/d.zip"}},
                {{"name":"e.zip","browser_download_url":"{d}e.zip","digest":"md5:abc"}}]}}"#,
            h = "AB".repeat(32)
        );
        let r = parse_release(&json).unwrap();
        let names: Vec<&str> = r.assets.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["a.zip", "b.zip"], "elsewhere, plain http and an unknown digest are left out");
        assert_eq!(r.assets[0].sha256.as_deref(), Some("ab".repeat(32).as_str()));
        assert_eq!((r.assets[0].size, r.assets[1].sha256.clone()), (12, None));
    }

    #[test]
    fn the_check_is_on_unless_the_file_says_otherwise() {
        assert!(UpdateSettings::default().check_automatically);
        let s: crate::config::Settings = toml::from_str("[behavior]\nskip_add_dialog = true\n").unwrap();
        assert!(s.updates.check_automatically, "a file without the section");
        let s: crate::config::Settings = toml::from_str("[updates]\nlast_check = 5\n").unwrap();
        assert!(s.updates.check_automatically, "a section without the switch");
        let s: crate::config::Settings = toml::from_str("[updates]\ncheck_automatically = false\n").unwrap();
        assert!(!s.updates.check_automatically, "switched off stays off");
    }

    #[test]
    fn the_check_is_due_once_a_day() {
        let mut u = UpdateSettings { last_check: 1_000_000, ..Default::default() };
        assert!(!u.due(1_000_000 + 3600));
        assert!(u.due(1_000_000 + CHECK_EVERY_SECS + 1));
        assert!(u.due(999_000), "the clock went back");
        u.check_automatically = false;
        assert!(!u.due(1_000_000 + CHECK_EVERY_SECS + 1));
    }
}
