//! The address a person types or pastes: the origin of a server, and the invite
//! code a pasted link carries.
use super::app_url;
use url::Url;

fn loopback(url: &Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(name)) => name.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

/// One layer of quotes or angle brackets, as a link arrives from a chat message.
fn unwrap_paste(input: &str) -> &str {
    let input = input.trim();
    let bytes = input.as_bytes();
    if bytes.len() >= 2 {
        let (open, close) = (bytes[0], bytes[bytes.len() - 1]);
        let wrapped = (open == b'<' && close == b'>')
            || (open == b'"' && close == b'"')
            || (open == b'\'' && close == b'\'');
        if wrapped {
            return input[1..input.len() - 1].trim();
        }
    }
    input
}

/// The authority of what follows `://`: up to the first `/`, `?` or `#`. An `@` in the path or
/// query (a `/@name` profile link, an e-mail in `?q=`) is page content, not credentials.
fn authority_of(rest: &str) -> &str {
    rest.split(['/', '?', '#']).next().unwrap_or(rest)
}

/// The pasted text as a URL, before the page (path, query, fragment) is removed.
fn parsed(input: &str) -> Result<Url, String> {
    if input.len() > 2048 || input.chars().any(char::is_control) {
        return Err("Enter an HTTPS instance address.".into());
    }
    let input = unwrap_paste(input);
    let raw = if input.contains("://") {
        input.to_owned()
    } else {
        format!("https://{input}")
    };
    if raw.contains('\\') || raw.chars().any(char::is_whitespace) {
        return Err("Invalid instance address.".into());
    }
    let url = Url::parse(&raw).map_err(|_| "Invalid instance address.")?;
    let credentials = !url.username().is_empty()
        || url.password().is_some()
        || raw
            .split_once("://")
            .is_some_and(|(_, rest)| authority_of(rest).contains('@'));
    if url.host_str().is_none() {
        return Err("Invalid instance address.".into());
    }
    if credentials || (url.scheme() != "https" && url.scheme() != "http") {
        return Err("Use a web address (https://…) without a user name or password.".into());
    }
    Ok(url)
}

pub(crate) fn address(input: &str) -> Result<Url, String> {
    let mut url = parsed(input)?;
    // A pasted invite link keeps the host and an explicit port. The path,
    // query and fragment belong to the page. Plain http elsewhere becomes
    // https; a server on this machine stays http.
    if url.scheme() == "http" && !loopback(&url) {
        url.set_scheme("https")
            .map_err(|_| "Invalid instance address.".to_string())?;
    }
    url.set_path("/");
    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}

fn invite_token(value: &str) -> Option<String> {
    let value = value.trim().trim_end_matches(['.', ',', ';', ')']);
    crate::local_route::invite_segment(value).then(|| value.to_owned())
}

pub(super) fn path_invite(path: &str) -> Option<String> {
    let rest = path.strip_prefix("/invite/")?;
    let code = rest.strip_suffix('/').unwrap_or(rest);
    if code.contains('/') {
        return None;
    }
    invite_token(code)
}

fn query_invite(query: Option<&str>) -> Option<String> {
    let query = query?;
    url::form_urlencoded::parse(query.as_bytes())
        .find(|(key, _)| key == "invite")
        .and_then(|(_, value)| invite_token(&value))
}

fn fragment_invite(fragment: Option<&str>) -> Option<String> {
    let query = fragment?.split_once('?')?.1;
    query_invite(Some(query))
}

/// The invite code in a pasted link: `/invite/{code}`, `?invite=`, or the same
/// pair in the fragment. Anything else is not an invite, and connecting still
/// works. The code is the web app's, so the join screen can read it.
pub(crate) fn invite_code(input: &str) -> Option<String> {
    let url = parsed(input).ok()?;
    path_invite(url.path())
        .or_else(|| query_invite(url.query()))
        .or_else(|| fragment_invite(url.fragment()))
}

/// Where a new server page opens. A recognised invite opens the join screen;
/// a bare address opens the app. A code that is not an invite stays at the root.
pub(crate) fn join_page(code: Option<&str>) -> Url {
    let mut url = app_url();
    if let Some(code) = code.and_then(invite_token) {
        url.set_path(&format!("/invite/{code}"));
    }
    url
}
