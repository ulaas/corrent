//! The media UI's connection to a server (in-process or remote): typed API v1 calls.

use std::sync::{Arc, Mutex};

use serde::Serialize;
use serde::de::DeserializeOwned;

#[derive(Clone)]
pub struct MediaClient {
    http: reqwest::Client,
    base: String,
    token: String,
    /// The server's CA as a file, for mpv (`tls-ca-file`).
    ca_file: Option<std::path::PathBuf>,
}

impl MediaClient {
    /// `ca_pem`: the server's own CA (remote servers with a self-signed certificate).
    pub fn new(base: &str, token: &str, ca_pem: Option<&str>) -> Result<MediaClient, String> {
        let mut b = reqwest::Client::builder().timeout(std::time::Duration::from_secs(60));
        if let Some(pem) = ca_pem {
            let ca = reqwest::Certificate::from_pem(pem.as_bytes()).map_err(|e| e.to_string())?;
            b = b.tls_certs_only([ca]);
        }
        Ok(MediaClient {
            http: b.build().map_err(|e| e.to_string())?,
            base: base.trim_end_matches('/').to_string(),
            token: token.to_string(),
            ca_file: None,
        })
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
pub type Slot<T> = Arc<Mutex<Option<Result<T, String>>>>;

/// Runs `fut` on the runtime; the UI repaints when it finishes.
pub fn fetch<T: Send + 'static>(
    rt: &tokio::runtime::Handle,
    ctx: &egui::Context,
    fut: impl std::future::Future<Output = Result<T, String>> + Send + 'static,
) -> Slot<T> {
    let slot: Slot<T> = Arc::new(Mutex::new(None));
    let (s2, ctx) = (slot.clone(), ctx.clone());
    rt.spawn(async move {
        let r = fut.await;
        *s2.lock().unwrap_or_else(|e| e.into_inner()) = Some(r);
        ctx.request_repaint();
    });
    slot
}

/// Reads a slot: `None` while loading.
pub fn ready<T: Clone>(slot: &Slot<T>) -> Option<Result<T, String>> {
    slot.lock().unwrap_or_else(|e| e.into_inner()).clone()
}
