//! Tests of the trusted client, by the module they cover.
use super::*;
use super::{
    address::*, commands::*, dialog::*, discover::*, links::*, requests::*, rounds::*, run::*,
    trusted::*,
};
use crate::instances::{Phase, Session};
use crate::web_bundle::{ServerInfo, Store, Version};
use crate::web_client::trusted::Trusted;
use crate::web_proxy::{Resp, now};
use std::{sync::Arc, time::Duration};
use url::Url;

mod discover;
mod links;
mod origin;
mod requests;
mod run;
mod trusted;
mod vectors;

fn url(raw: &str) -> Url {
    Url::parse(raw).unwrap()
}

fn app(path: &str) -> Url {
    app_url().join(path).unwrap()
}

/// 2026-11-01T00:00:00Z
const NOW: u64 = 1_793_491_200;

/// Starts a server on the list with a running session on `held`.
fn run_server(trusted: &Trusted, host: &str, held: Held) -> (String, Arc<Session>) {
    run_server_at(
        trusted,
        Url::parse(&format!("https://{host}/")).unwrap(),
        held,
    )
}

fn run_server_at(trusted: &Trusted, origin: Url, held: Held) -> (String, Arc<Session>) {
    use crate::web_proxy::{Instance, Jar, Limits, client};
    let mut registry = trusted.registry.lock().unwrap();
    let id = registry.add(origin.clone()).unwrap();
    let session = Arc::new(Session {
        id: id.clone(),
        instance: Arc::new(Instance {
            client: client().unwrap(),
            jar: Jar::new(None, 0),
            origin,
            limits: Limits::default(),
        }),
        held,
        community: None,
        sockets: Arc::new(crate::web_ws::Sockets::default()),
    });
    registry.slot_mut(&id).unwrap().phase = Phase::Ready(Arc::clone(&session));
    (id, session)
}

fn held_of(version: &'static str, seed: u8) -> Held {
    use crate::web_bundle::fixture::{Spec, bundle, trusted as keys};
    let mut spec = Spec::new(version);
    spec.seed = seed;
    Store::load(None, Some(&bundle(&spec)), &keys(seed))
        .select(&ServerInfo {
            version: Version::parse(version),
            web_client_api: 1,
        })
        .unwrap()
}
