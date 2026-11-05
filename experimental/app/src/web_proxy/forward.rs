//! `/api/` of the selected instance, forwarded with that instance's own
//! cookies. The page names a path; everything else about the request is the
//! host's.
use super::{
    jar::Jar,
    limits::{Limits, SMALL_BODY},
    now,
    resp::Resp,
    rules::{FORWARDED_REQUEST, FORWARDED_RESPONSE, clamp_range, inline_safe, target},
};
use reqwest::Client;
use std::time::Duration;
use url::Url;

/// The selected instance.
pub struct Instance {
    pub origin: Url,
    pub client: Client,
    pub jar: Jar,
    pub limits: Limits,
}

/// How long the server may stay silent while the proxy waits for response
/// headers (GET/HEAD) or for the next piece of any answer. It is not a client
/// timeout: reqwest's read timeout is armed when the request starts and keeps
/// running while the body uploads, so it cannot serve here.
pub(super) const IDLE_TIMEOUT: Duration = Duration::from_secs(120);

/// Base time for a request that carries a body, on top of the allowance below.
pub(super) const UPLOAD_BASE: Duration = Duration::from_secs(600);

/// Slowest uplink an upload is still expected to finish on, in bytes per second.
const UPLOAD_MIN_RATE: u64 = 64 << 10;

/// Total time for a request with a body: the old 600 s plus what the body
/// needs at the minimum rate, so a large file on a home uplink can finish.
pub(super) fn upload_timeout(body_len: usize) -> Duration {
    UPLOAD_BASE + Duration::from_secs(body_len as u64 / UPLOAD_MIN_RATE)
}

pub async fn forward(
    instance: &Instance,
    method: &str,
    path_and_query: &str,
    headers: &[(String, String)],
    body: Vec<u8>,
) -> Resp {
    forward_unsealed(instance, method, path_and_query, headers, body)
        .await
        .sealed()
}

fn too_many_transfers() -> Resp {
    Resp::error(
        503,
        "UNAVAILABLE",
        "too many large transfers at once, try again",
    )
}

fn unreachable() -> Resp {
    Resp::error(502, "UNAVAILABLE", "the server could not be reached")
}

/// The methods the proxy forwards.
fn parse_method(method: &str) -> Option<reqwest::Method> {
    Some(match method {
        "GET" => reqwest::Method::GET,
        "HEAD" => reqwest::Method::HEAD,
        "POST" => reqwest::Method::POST,
        "PUT" => reqwest::Method::PUT,
        "PATCH" => reqwest::Method::PATCH,
        "DELETE" => reqwest::Method::DELETE,
        _ => return None,
    })
}

/// The request to the instance: the few headers of the page that may pass, the
/// origin and the cookie of the host's own making.
fn build_request(
    instance: &Instance,
    method: reqwest::Method,
    url: Url,
    headers: &[(String, String)],
    body: Vec<u8>,
) -> reqwest::RequestBuilder {
    let has_body = method != reqwest::Method::GET && method != reqwest::Method::HEAD;
    let mut request = instance.client.request(method, url);
    for (name, value) in headers {
        let lower = name.to_ascii_lowercase();
        if lower == "range" {
            request = request.header("range", clamp_range(value));
        } else if FORWARDED_REQUEST.contains(&lower.as_str()) {
            request = request.header(name.as_str(), value.as_str());
        }
    }
    // The page cannot choose these: the host speaks as the browser would on
    // the instance's own origin.
    request = request.header("origin", instance.origin.origin().ascii_serialization());
    if let Some(cookie) = instance.jar.header(now()) {
        request = request.header("cookie", cookie);
    }
    if has_body {
        // The server answers after it has read the body, so waiting for the
        // headers is bounded by the upload, not by the idle limit.
        request = request.timeout(upload_timeout(body.len())).body(body);
    }
    request
}

/// Sends the request and waits for the headers of the answer.
async fn send(request: reqwest::RequestBuilder, has_body: bool) -> Result<reqwest::Response, ()> {
    if has_body {
        request.send().await.map_err(|_| ())
    } else {
        match tokio::time::timeout(IDLE_TIMEOUT, request.send()).await {
            Ok(result) => result.map_err(|_| ()),
            Err(_) => Err(()),
        }
    }
}

/// Keeps the cookies the instance set (to rest on a blocking thread).
async fn keep_cookies(instance: &Instance, response: &reqwest::Response) {
    let set_cookie: Vec<String> = response
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok().map(str::to_owned))
        .collect();
    instance
        .jar
        .store_async(&set_cookie, instance.origin.scheme() == "https", now())
        .await;
}

/// Reads the body of the answer, each piece within the idle limit and the
/// whole within `read_cap`.
async fn read_body(
    response: &mut reqwest::Response,
    announced: Option<u64>,
    read_cap: usize,
) -> Result<Vec<u8>, Resp> {
    let mut out = Vec::with_capacity(announced.map_or(0, |n| (n as usize).min(read_cap)));
    loop {
        match tokio::time::timeout(IDLE_TIMEOUT, response.chunk()).await {
            Ok(Ok(Some(chunk))) if chunk.len() <= read_cap - out.len() => {
                out.extend_from_slice(&chunk)
            }
            Ok(Ok(None)) => return Ok(out),
            _ => {
                return Err(Resp::error(
                    502,
                    "UNAVAILABLE",
                    "the server's answer was cut off or too large",
                ));
            }
        }
    }
}

/// Takes the upload limit out of the answer to `GET /api/permissions`.
fn announce_upload_limit(instance: &Instance, body: &[u8]) {
    if let Some(mb) = serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("max_upload_mb")?.as_u64())
    {
        instance.limits.announce_upload_mb(mb);
    }
}

/// The answer to the page: only the headers it may see, and a type that cannot
/// run as a document in the app's origin.
fn page_answer(
    method: &reqwest::Method,
    response: &reqwest::Response,
    status: u16,
    out: Vec<u8>,
) -> Resp {
    let mut headers: Vec<(String, String)> = response
        .headers()
        .iter()
        .filter(|(name, _)| FORWARDED_RESPONSE.contains(&name.as_str()))
        .filter_map(|(name, value)| {
            Some((name.as_str().to_owned(), value.to_str().ok()?.to_owned()))
        })
        .collect();
    let declared = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream");
    if inline_safe(declared) {
        headers.push(("content-type".into(), declared.to_owned()));
    } else {
        headers.push(("content-type".into(), "application/octet-stream".into()));
        if !headers
            .iter()
            .any(|(name, _)| name == "content-disposition")
        {
            headers.push(("content-disposition".into(), "attachment".into()));
        }
    }
    headers.push(("cross-origin-resource-policy".into(), "same-origin".into()));
    Resp {
        status,
        headers,
        body: if *method == reqwest::Method::HEAD {
            Vec::new()
        } else {
            out
        },
    }
}

async fn forward_unsealed(
    instance: &Instance,
    method: &str,
    path_and_query: &str,
    headers: &[(String, String)],
    body: Vec<u8>,
) -> Resp {
    let Some(url) = target(&instance.origin, path_and_query) else {
        return Resp::error(400, "INVALID_INPUT", "unsupported path");
    };
    let Some(method) = parse_method(method) else {
        return Resp::error(405, "INVALID_INPUT", "method not allowed");
    };
    let cap = instance.limits.request_cap(url.path());
    if body.len() > cap {
        return instance.limits.too_large(url.path(), cap);
    }
    // Held until the answer is complete: the body stays in memory until the
    // instance has read it.
    let Ok(_request_memory) = instance.limits.lend(body.len()).await else {
        return too_many_transfers();
    };
    let announces_limit = url.path() == "/api/permissions" && method == reqwest::Method::GET;
    let has_body = method != reqwest::Method::GET && method != reqwest::Method::HEAD;
    let request = build_request(instance, method.clone(), url, headers, body);
    let Ok(mut response) = send(request, has_body).await else {
        return unreachable();
    };
    let status = response.status().as_u16();
    keep_cookies(instance, &response).await;
    // 304 answers the conditional request the proxy passes on; it is no redirect.
    if response.status().is_redirection() && status != 304 {
        return Resp::error(502, "UNAVAILABLE", "the server answered with a redirect");
    }
    let response_cap = instance.limits.response_cap();
    // A HEAD answer announces the length of a body it does not carry: nothing
    // to size, borrow memory for or allocate.
    let announced = if method == reqwest::Method::HEAD {
        None
    } else {
        response.content_length()
    };
    if announced.is_some_and(|n| n > response_cap as u64) {
        return Resp::error(
            502,
            "UNAVAILABLE",
            "the server's answer is too large for this client",
        );
    }
    // A large answer borrows its memory first; one of unknown length is held
    // to the small-body size.
    let (reserve, read_cap) = match announced {
        Some(n) => (n as usize, response_cap),
        None => (0, SMALL_BODY),
    };
    let Ok(_answer_memory) = instance.limits.lend(reserve).await else {
        return too_many_transfers();
    };
    let out = match read_body(&mut response, announced, read_cap).await {
        Ok(out) => out,
        Err(resp) => return resp,
    };
    if announces_limit && status == 200 {
        announce_upload_limit(instance, &out);
    }
    page_answer(&method, &response, status, out)
}
