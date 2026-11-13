//! Test-only: a server on the list with a running session.
use super::{Phase, Registry, Session};
use crate::web_bundle::{Held, ServerInfo, Store, Version, fixture};
use crate::web_proxy::{Instance, Jar, Limits, client};
use std::sync::Arc;
use url::Url;

fn held() -> Held {
    let store = Store::load(
        None,
        Some(&fixture::bundle(&fixture::Spec::new("1.1.0"))),
        &fixture::trusted(7),
    );
    store
        .select(&ServerInfo {
            version: Some(Version(1, 1, 0)),
            web_client_api: 1,
        })
        .unwrap()
}

/// Adds `https://<host>/` to `registry` and starts it: its id and session.
pub fn ready(registry: &mut Registry, host: &str) -> (String, Arc<Session>) {
    let origin = Url::parse(&format!("https://{host}/")).unwrap();
    let id = registry.add(origin.clone()).unwrap();
    let session = Arc::new(Session {
        id: id.clone(),
        instance: Arc::new(Instance {
            client: client().unwrap(),
            jar: Jar::new(None, 0),
            origin,
            limits: Limits::default(),
        }),
        held: held(),
        community: None,
        sockets: Arc::new(crate::web_ws::Sockets::default()),
    });
    registry.slot_mut(&id).unwrap().phase = Phase::Ready(Arc::clone(&session));
    (id, session)
}
