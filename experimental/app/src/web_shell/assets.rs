//! The files of the rail page and of the dialog it opens, served by the host.
use crate::web_proxy::Resp;

const INDEX: &str = include_str!("../../rail-ui/index.html");
const STYLE: &str = include_str!("../../rail-ui/rail.css");
const SCRIPT_TEXT: &str = include_str!("../../rail-ui/rail-text.js");
const SCRIPT_STATE: &str = include_str!("../../rail-ui/rail-state.js");
const SCRIPT_RENDER: &str = include_str!("../../rail-ui/rail-render.js");
const SCRIPT_COMMANDS: &str = include_str!("../../rail-ui/rail-commands.js");
const SCRIPT_MENU: &str = include_str!("../../rail-ui/rail-menu.js");
const SCRIPT_DRAG: &str = include_str!("../../rail-ui/rail-drag.js");
const SCRIPT: &str = include_str!("../../rail-ui/rail.js");
const DIALOG_INDEX: &str = include_str!("../../rail-ui/dialog.html");
const DIALOG_SCRIPT: &str = include_str!("../../rail-ui/dialog.js");
const DIALOG_STYLE: &str = include_str!("../../rail-ui/dialog.css");

/// What the rail page may do: load itself and talk to the host.
pub const RAIL_CSP: &str = concat!(
    "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; ",
    "connect-src ipc: http://ipc.localhost https://ipc.localhost; ",
    "base-uri 'none'; form-action 'none'; frame-ancestors 'none'"
);

/// The files of the rail page and of the dialog it opens.
pub fn rail_asset(method: &str, path: &str) -> Resp {
    let (kind, body) = match path {
        "/" | "/index.html" => ("text/html; charset=utf-8", INDEX),
        "/rail.css" => ("text/css; charset=utf-8", STYLE),
        "/rail-text.js" => ("text/javascript; charset=utf-8", SCRIPT_TEXT),
        "/rail-state.js" => ("text/javascript; charset=utf-8", SCRIPT_STATE),
        "/rail-render.js" => ("text/javascript; charset=utf-8", SCRIPT_RENDER),
        "/rail-commands.js" => ("text/javascript; charset=utf-8", SCRIPT_COMMANDS),
        "/rail-menu.js" => ("text/javascript; charset=utf-8", SCRIPT_MENU),
        "/rail-drag.js" => ("text/javascript; charset=utf-8", SCRIPT_DRAG),
        "/rail.js" => ("text/javascript; charset=utf-8", SCRIPT),
        "/dialog.html" => ("text/html; charset=utf-8", DIALOG_INDEX),
        "/dialog.css" => ("text/css; charset=utf-8", DIALOG_STYLE),
        "/dialog.js" => ("text/javascript; charset=utf-8", DIALOG_SCRIPT),
        _ => return text(404, "Not found."),
    };
    if method != "GET" && method != "HEAD" {
        return text(405, "Method not allowed.");
    }
    Resp {
        status: 200,
        headers: vec![
            ("content-type".into(), kind.into()),
            ("content-security-policy".into(), RAIL_CSP.into()),
            ("x-content-type-options".into(), "nosniff".into()),
            ("cache-control".into(), "no-store".into()),
        ],
        body: if method == "HEAD" {
            Vec::new()
        } else {
            body.as_bytes().to_vec()
        },
    }
}

fn text(status: u16, body: &str) -> Resp {
    Resp {
        status,
        headers: vec![
            ("content-type".into(), "text/plain; charset=utf-8".into()),
            ("x-content-type-options".into(), "nosniff".into()),
        ],
        body: body.as_bytes().to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rail_page_is_served_with_its_own_strict_policy_and_nothing_else() {
        for path in [
            "/",
            "/index.html",
            "/rail.css",
            "/rail-text.js",
            "/rail-state.js",
            "/rail-render.js",
            "/rail-commands.js",
            "/rail-menu.js",
            "/rail-drag.js",
            "/rail.js",
            "/dialog.html",
            "/dialog.css",
            "/dialog.js",
        ] {
            let resp = rail_asset("GET", path);
            assert_eq!(resp.status, 200, "{path}");
            let csp = resp
                .headers
                .iter()
                .find(|(n, _)| n == "content-security-policy")
                .map(|(_, v)| v.as_str());
            assert_eq!(csp, Some(RAIL_CSP));
            assert!(!resp.body.is_empty());
        }
        assert!(!RAIL_CSP.contains("unsafe-inline") && !RAIL_CSP.contains("unsafe-eval"));
        assert!(RAIL_CSP.contains("default-src 'none'"));
        assert_eq!(rail_asset("HEAD", "/rail.js").body.len(), 0);
        assert_eq!(rail_asset("POST", "/rail.js").status, 405);
        for path in [
            "/api/health",
            "/c/1",
            "/rail.js/",
            "/dialog",
            "/dialog.html/",
            "/icons/x.png",
            "/index.html?x",
            "/../x",
            "",
        ] {
            assert_eq!(rail_asset("GET", path).status, 404, "{path}");
        }
    }
}
