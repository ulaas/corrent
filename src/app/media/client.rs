//! The media UI's connection to a server (in-process or remote): typed API v1 calls.

use std::sync::{Arc, Mutex};

use serde::Serialize;
use serde::de::DeserializeOwned;

#[derive(Clone)]
pub struct MediaClient {
    http: reqwest::Client,
    /// For long downloads: no overall deadline, only a stall timeout.
    dl: reqwest::Client,
    base: String,
    token: String,
    /// The server's CA as a file, for mpv (`tls-ca-file`).
    ca_file: Option<std::path::PathBuf>,
}

impl MediaClient {
    /// `ca_pem`: the server's own CA (remote servers with a self-signed certificate).
    pub fn new(base: &str, token: &str, ca_pem: Option<&str>) -> Result<MediaClient, String> {
        let mut b = reqwest::Client::builder().timeout(std::time::Duration::from_secs(60));
        let mut d = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(20))
            .read_timeout(std::time::Duration::from_secs(120));
        if let Some(pem) = ca_pem {
            let ca = reqwest::Certificate::from_pem(pem.as_bytes()).map_err(|e| e.to_string())?;
            b = b.tls_certs_only([ca.clone()]);
            d = d.tls_certs_only([ca]);
        }
        Ok(MediaClient {
            http: b.build().map_err(|e| e.to_string())?,
            dl: d.build().map_err(|e| e.to_string())?,
            base: base.trim_end_matches('/').to_string(),
            token: token.to_string(),
            ca_file: None,
        })
    }

    /// The same server as another user (profile switch).
    pub fn with_token(&self, token: &str) -> MediaClient {
        MediaClient { token: token.to_string(), ..self.clone() }
    }

    pub fn with_ca_file(mut self, path: Option<std::path::PathBuf>) -> MediaClient {
        self.ca_file = path;
        self
    }

    pub fn ca_file(&self) -> Option<&std::path::Path> {
        self.ca_file.as_deref()
    }

    /// Absolute URL of an API path (for mpv and image requests).
    pub fn url(&self, path: &str) -> String {
        if path.starts_with("http://") || path.starts_with("https://") {
            path.to_string()
        } else {
            format!("{}{path}", self.base)
        }
    }

    /// The header mpv sends with every request (`--http-header-fields`).
    pub fn auth_header(&self) -> String {
        format!("Authorization: Bearer {}", self.token)
    }

    async fn decode<T: DeserializeOwned>(res: reqwest::Response) -> Result<T, String> {
        let status = res.status();
        let bytes = res.bytes().await.map_err(|e| e.to_string())?;
        if !status.is_success() {
            let msg = serde_json::from_slice::<corrent_proto::ApiError>(&bytes)
                .map(|e| e.code.text().to_string())
                .unwrap_or_else(|_| status.to_string());
            return Err(msg);
        }
        serde_json::from_slice(&bytes).map_err(|e| e.to_string())
    }

    /// Like `decode`, keeping the server's reason: a code's general text would mislead where the
    /// same code means several things (casting). Permissions still read as permissions.
    async fn decode_reason<T: DeserializeOwned>(res: reqwest::Response) -> Result<T, String> {
        let status = res.status();
        let bytes = res.bytes().await.map_err(|e| e.to_string())?;
        if !status.is_success() {
            return Err(match serde_json::from_slice::<corrent_proto::ApiError>(&bytes) {
                Ok(e) if e.code == corrent_proto::ErrorCode::Forbidden => e.code.text().to_string(),
                Ok(e) => e.message,
                Err(_) => status.to_string(),
            });
        }
        serde_json::from_slice(&bytes).map_err(|e| e.to_string())
    }

    /// `get`, with the server's reason when it fails.
    pub async fn get_reason<T: DeserializeOwned>(&self, path: &str) -> Result<T, String> {
        let res = self.http.get(self.url(path)).bearer_auth(&self.token).send().await.map_err(|e| e.to_string())?;
        Self::decode_reason(res).await
    }

    /// `send`, with the server's reason when it fails.
    pub async fn send_reason<T: DeserializeOwned, B: Serialize>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: &B,
    ) -> Result<T, String> {
        let body = serde_json::to_vec(body).map_err(|e| e.to_string())?;
        let res = self
            .http
            .request(method, self.url(path))
            .bearer_auth(&self.token)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        Self::decode_reason(res).await
    }

    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, String> {
        let res = self.http.get(self.url(path)).bearer_auth(&self.token).send().await.map_err(|e| e.to_string())?;
        Self::decode(res).await
    }

    pub async fn send<T: DeserializeOwned, B: Serialize>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: &B,
    ) -> Result<T, String> {
        let body = serde_json::to_vec(body).map_err(|e| e.to_string())?;
        let res = self
            .http
            .request(method, self.url(path))
            .bearer_auth(&self.token)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        Self::decode(res).await
    }

    pub async fn multipart<T: DeserializeOwned>(
        &self,
        path: &str,
        form: reqwest::multipart::Form,
    ) -> Result<T, String> {
        let res = self
            .http
            .post(self.url(path))
            .bearer_auth(&self.token)
            .multipart(form)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        Self::decode(res).await
    }

    /// A download from byte `from` on (resuming); the caller reads the body as it arrives.
    pub async fn download(&self, path: &str, from: u64) -> Result<reqwest::Response, String> {
        let mut r = self.dl.get(self.url(path)).bearer_auth(&self.token);
        if from > 0 {
            r = r.header(reqwest::header::RANGE, format!("bytes={from}-"));
        }
        let res = r.send().await.map_err(|e| e.to_string())?;
        if !res.status().is_success() {
            return Err(res.status().to_string());
        }
        Ok(res)
    }

    /// A whole response with only a stall timeout (slow transcoded segments).
    pub async fn download_bytes(&self, path: &str) -> Result<Vec<u8>, String> {
        let res = self.download(path, 0).await?;
        Ok(res.bytes().await.map_err(|e| e.to_string())?.to_vec())
    }

    /// The server's address (what offline copies are filed under).
    pub fn base(&self) -> &str {
        &self.base
    }

    pub async fn text(&self, path: &str) -> Result<String, String> {
        let bytes = self.bytes(path).await?;
        String::from_utf8(bytes).map_err(|e| e.to_string())
    }

    pub async fn bytes(&self, path: &str) -> Result<Vec<u8>, String> {
        let res = self.http.get(self.url(path)).bearer_auth(&self.token).send().await.map_err(|e| e.to_string())?;
        if !res.status().is_success() {
            return Err(res.status().to_string());
        }
        Ok(res.bytes().await.map_err(|e| e.to_string())?.to_vec())
    }
}

/// A result that arrives later (filled by a task, read by the UI).
///
/// Held in an `Arc`: the media pages look at their data every frame, and a home screen or a
/// 500-episode show must not be copied that often. Read with [`ready_arc`].
pub type Slot<T> = Arc<Mutex<Option<Result<Arc<T>, String>>>>;

/// Runs `fut` on the runtime; the UI repaints when it finishes.
pub fn fetch<T: Send + Sync + 'static>(
    rt: &tokio::runtime::Handle,
    ctx: &egui::Context,
    fut: impl std::future::Future<Output = Result<T, String>> + Send + 'static,
) -> Slot<T> {
    let slot: Slot<T> = Arc::new(Mutex::new(None));
    let (s2, ctx) = (slot.clone(), ctx.clone());
    rt.spawn(async move {
        let r = fut.await.map(Arc::new);
        *s2.lock().unwrap_or_else(|e| e.into_inner()) = Some(r);
        ctx.request_repaint();
    });
    slot
}

/// Reads a slot, copying the value: `None` while loading. For small values and one-off reads;
/// what is drawn every frame uses [`ready_arc`].
pub fn ready<T: Clone>(slot: &Slot<T>) -> Option<Result<T, String>> {
    ready_arc(slot).map(|r| r.map(|v| (*v).clone()))
}

/// Reads a slot without copying its value: `None` while loading.
pub fn ready_arc<T>(slot: &Slot<T>) -> Option<Result<Arc<T>, String>> {
    slot.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Has the result arrived?
pub fn is_ready<T>(slot: &Slot<T>) -> bool {
    slot.lock().unwrap_or_else(|e| e.into_inner()).is_some()
}

/// Moves the result out of a slot that is done with.
pub fn take_ready<T: Clone>(slot: &Slot<T>) -> Option<Result<T, String>> {
    let r = slot.lock().unwrap_or_else(|e| e.into_inner()).take()?;
    Some(r.map(|v| Arc::try_unwrap(v).unwrap_or_else(|v| (*v).clone())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reading_a_slot_shares_the_value() {
        let slot: Slot<Vec<String>> = Arc::new(Mutex::new(None));
        assert!(!is_ready(&slot));
        assert!(ready_arc(&slot).is_none());
        *slot.lock().unwrap() = Some(Ok(Arc::new(vec!["a".to_string()])));
        assert!(is_ready(&slot));
        let (a, b) = (ready_arc(&slot).unwrap().unwrap(), ready_arc(&slot).unwrap().unwrap());
        assert!(Arc::ptr_eq(&a, &b), "the same value, not a copy");
        assert_eq!(ready(&slot).unwrap().unwrap(), vec!["a".to_string()]);
        drop((a, b));
        assert_eq!(take_ready(&slot).unwrap().unwrap(), vec!["a".to_string()]);
        assert!(!is_ready(&slot), "taken");
    }
}
