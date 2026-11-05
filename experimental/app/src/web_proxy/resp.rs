//! The answer of the proxy and of the bundle server, as the webview protocol
//! takes it.

/// Applied to everything that comes from the instance. `sandbox` (without
/// `allow-same-origin` or `allow-scripts`) gives a document opened from such a
/// response an opaque origin without script, whatever the server claimed.
pub(super) const DATA_CSP: &str = "sandbox; default-src 'none'";

pub struct Resp {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Resp {
    pub(super) fn plain(status: u16, text: &str) -> Resp {
        Resp {
            status,
            headers: vec![
                ("content-type".into(), "text/plain; charset=utf-8".into()),
                ("x-content-type-options".into(), "nosniff".into()),
                ("cache-control".into(), "no-store".into()),
            ],
            body: text.as_bytes().to_vec(),
        }
    }
    /// The answer as a document of the page may meet it: whatever produced
    /// it, a response under `/api/` carries the sandbox policy and no sniffing,
    /// so no branch (an error, a redirect notice, a new content type) can hand
    /// the privileged page of a server a document that runs script.
    pub fn sealed(mut self) -> Resp {
        self.headers.retain(|(name, _)| {
            name != "content-security-policy" && name != "x-content-type-options"
        });
        self.headers
            .push(("content-security-policy".into(), DATA_CSP.into()));
        self.headers
            .push(("x-content-type-options".into(), "nosniff".into()));
        self
    }
    /// The server's error envelope, so the app's error handling applies.
    pub(super) fn error(status: u16, code: &str, message: &str) -> Resp {
        Resp {
            status,
            headers: vec![
                ("content-type".into(), "application/json".into()),
                ("x-content-type-options".into(), "nosniff".into()),
                ("cache-control".into(), "no-store".into()),
            ],
            body: serde_json::json!({"error": {"code": code, "message": message}})
                .to_string()
                .into_bytes(),
        }
    }
}
