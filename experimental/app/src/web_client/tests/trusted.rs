//! The trusted state: keys in force, withdrawn bundles and the sessions on them.
use super::*;

#[test]
fn only_a_statement_from_the_pinned_root_key_changes_the_keys_in_force() {
    let trusted = Trusted::new(None, None);
    let before: Vec<String> = trusted.keys().iter().map(|k| k.id.clone()).collect();
    let doc = include_bytes!("../../../fixtures/trust.json");
    let sig = include_bytes!("../../../fixtures/trust.json.sig");
    let zero = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, [0u8; 64]);
    assert!(trusted.apply_trust(doc, zero.as_bytes(), NOW).is_err());
    let mut tampered = doc.to_vec();
    tampered[12] ^= 1;
    assert!(trusted.apply_trust(&tampered, sig, NOW).is_err());
    // The real statement lists the pinned key: nothing is dropped.
    assert_eq!(trusted.apply_trust(doc, sig, NOW), Ok(false));
    let after: Vec<String> = trusted.keys().iter().map(|k| k.id.clone()).collect();
    assert_eq!(before, after);
    // Applying it again is a no-op.
    assert_eq!(trusted.apply_trust(doc, sig, NOW), Ok(false));
}

#[test]
fn a_cap_that_withdraws_the_running_bundle_ends_the_session() {
    use crate::web_bundle::fixture::{Spec, bundle, trusted as keys};
    let trusted = Trusted::new(None, None);
    let caps = |v: Version| std::collections::BTreeMap::from([(1, v)]);
    let (id, session) = {
        let mut store = trusted.store.lock().unwrap();
        *store = Store::load(None, Some(&bundle(&Spec::new("0.9.0"))), &keys(7));
        run_server(&trusted, "community.example", held_of("0.9.0", 7))
    };
    // A cap at the running version changes nothing.
    let mut store = trusted.store.lock().unwrap();
    assert!(!trusted.enforce(&mut store, &keys(7), &caps(Version(0, 9, 0))));
    assert!(trusted.session(&id).is_some());
    // A cap below it withdraws the bundle, and the session ends with it.
    assert!(trusted.enforce(&mut store, &keys(7), &caps(Version(0, 8, 0))));
    assert!(trusted.session(&id).is_none());
    drop(store);
    let registry = trusted.registry.lock().unwrap();
    let Phase::Refused(why) = &registry.slot(&id).unwrap().phase else {
        panic!("the server's tab must say why it stopped")
    };
    assert_eq!(why, WITHDRAWN);
    drop(registry);
    // Its sockets went with it (none were open, and none may be opened).
    assert!(session.sockets.send(1, "x".into()).is_err());
}

#[test]
fn a_round_that_did_not_look_for_a_bundle_is_not_a_completed_check() {
    assert!(
        !looked(None),
        "no fresh trust statement: nothing was checked"
    );
    assert!(!looked(Some(&Err("unreachable".to_owned()))));
    assert!(looked(Some(&Ok(None))));
    assert!(looked(Some(&Ok(Some(Version(1, 0, 0))))));
}

#[test]
fn a_session_registered_after_its_bundle_was_withdrawn_is_ended() {
    use crate::web_bundle::fixture::{Spec, bundle, trusted as keys};
    let trusted = Trusted::new(None, None);
    // The bundle was chosen while key 7 was in force; the store lost it
    // (revoked) before the session reached the registry.
    let held = held_of("0.9.0", 7);
    *trusted.store.lock().unwrap() =
        Store::load(None, Some(&bundle(&Spec::new("0.9.0"))), &keys(8));
    let (id, session) = run_server(&trusted, "late.example", held);
    assert!(!trusted.keep_if_held(&id, &session));
    assert!(trusted.session(&id).is_none());
    let registry = trusted.registry.lock().unwrap();
    let Phase::Refused(why) = &registry.slot(&id).unwrap().phase else {
        panic!("the tab must say why it stopped")
    };
    assert_eq!(why, WITHDRAWN);
    drop(registry);
    assert!(session.sockets.send(1, "x".into()).is_err());
    // A held bundle stays.
    *trusted.store.lock().unwrap() =
        Store::load(None, Some(&bundle(&Spec::new("0.9.0"))), &keys(7));
    let (other, running) = run_server(&trusted, "ok.example", held_of("0.9.0", 7));
    assert!(trusted.keep_if_held(&other, &running));
    assert!(trusted.session(&other).is_some());
}

#[tokio::test]
async fn servers_that_start_together_share_one_network_round() {
    use crate::web_bundle::fixture::{Spec, bundle, trusted as keys};
    let trusted = Trusted::new(None, None);
    *trusted.store.lock().unwrap() =
        Store::load(None, Some(&bundle(&Spec::new("0.9.0"))), &keys(7));
    let newer = ServerInfo {
        version: Version::parse("9.0.0"),
        web_client_api: 99,
    };
    assert!(trusted.store.lock().unwrap().select(&newer).is_err());
    // A round ended a moment ago: nobody fetches again, offline or not.
    let at = std::time::Instant::now();
    *trusted.round.lock().await = Some(at);
    let started = std::time::Instant::now();
    let (a, b, c) = tokio::join!(
        choose(&trusted, &newer),
        choose(&trusted, &newer),
        choose(&trusted, &newer)
    );
    assert!(a.is_err() && b.is_err() && c.is_err());
    assert_eq!(*trusted.round.lock().await, Some(at));
    assert!(!trusted.fresh(), "no trust fetch ran");
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn a_withdrawn_bundle_ends_only_the_sessions_that_run_it() {
    use crate::web_bundle::fixture::{Spec, bundle, trusted as keys};
    let trusted = Trusted::new(None, None);
    let (a, b) = {
        let mut store = trusted.store.lock().unwrap();
        *store = Store::load(None, Some(&bundle(&Spec::new("0.9.0"))), &keys(7));
        // B runs a bundle signed by a key that is not in force (revoked).
        let a = run_server(&trusted, "a.example", held_of("0.9.0", 7)).0;
        let b = run_server(&trusted, "b.example", held_of("0.9.0", 8)).0;
        (a, b)
    };
    let mut store = trusted.store.lock().unwrap();
    trusted.enforce(&mut store, &keys(7), &std::collections::BTreeMap::new());
    drop(store);
    assert!(
        trusted.session(&a).is_some(),
        "the server on a held bundle keeps running"
    );
    assert!(
        trusted.session(&b).is_none(),
        "the one on a revoked key stops"
    );
}

#[test]
fn the_old_storage_is_wiped_once_and_never_handed_on() {
    let dir = std::env::temp_dir().join(format!("mnema-adopt-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sessions")).unwrap();
    // Sessions of two servers, whoever they were: nothing proves who owned the storage.
    for host in ["a.example", "b.example"] {
        let origin = Url::parse(&format!("https://{host}/")).unwrap();
        std::fs::write(
            dir.join("sessions")
                .join(format!("{}.json", crate::instances::id_for(&origin))),
            b"[]",
        )
        .unwrap();
    }
    // The first start of this version: the wipe is due, once.
    let first = Trusted::new(Some(dir.clone()), None);
    assert!(first.take_old_storage_wipe());
    assert!(!first.take_old_storage_wipe(), "once per start");
    // Writing the list is what marks it done: no second wipe on any later start.
    first.save_instances();
    assert!(!Trusted::new(Some(dir.clone()), None).take_old_storage_wipe());
    assert!(!Trusted::new(Some(dir.clone()), None).take_old_storage_wipe());
    // A fresh install has nothing to wipe.
    let none = std::env::temp_dir().join(format!("mnema-adopt-none-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&none);
    assert!(!Trusted::new(Some(none.clone()), None).take_old_storage_wipe());
    // And no data directory, no wipe.
    assert!(!Trusted::new(None, None).take_old_storage_wipe());
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(none);
}

#[test]
fn an_unusable_list_of_servers_stops_the_sweep_of_their_storage() {
    let dir = std::env::temp_dir().join(format!("mnema-list-bad-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    assert!(!Trusted::new(Some(dir.clone()), None).list_unreadable());
    std::fs::write(
        dir.join(INSTANCES_FILE),
        r#"{"version":2,"servers":[],"active":null}"#,
    )
    .unwrap();
    let trusted = Trusted::new(Some(dir.clone()), None);
    assert!(trusted.list_unreadable());
    assert!(trusted.registry.lock().unwrap().ids().is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn installs_need_a_fresh_statement_and_a_successful_fetch_in_this_round() {
    let doc = include_bytes!("../../../fixtures/trust.json").to_vec();
    let sig = include_bytes!("../../../fixtures/trust.json.sig").to_vec();
    let expiry = crate::web_trust::parse_utc("2027-04-15T22:00:57Z").unwrap();
    let trusted = Trusted::new(None, None);
    assert!(!trusted.fresh(), "no statement yet: no installs");
    // Offline: still nothing fresh, nothing dropped.
    assert!(!trusted.apply_round(Err("offline".into()), NOW));
    assert!(!trusted.fresh());
    // The statement arrives.
    trusted.apply_round(Ok((doc.clone(), sig.clone())), NOW);
    assert!(trusted.fresh());
    let keys = trusted.keys();
    // A later failed fetch keeps the last good statement, but installs stop.
    trusted.apply_round(Err("blocked".into()), NOW);
    assert!(!trusted.fresh());
    assert_eq!(trusted.keys().len(), keys.len());
    // The same statement is fresh again once fetched.
    trusted.apply_round(Ok((doc.clone(), sig.clone())), NOW);
    assert!(trusted.fresh());
    // Past its expiry the replay is refused, installs stay off and the
    // keys of the expired statement stay in force.
    trusted.apply_round(Ok((doc, sig)), expiry + 1);
    assert!(!trusted.fresh());
    assert_eq!(trusted.keys().len(), keys.len());
}

#[test]
fn a_client_without_any_bundle_says_so_instead_of_loading_server_code() {
    let trusted = Trusted::new(None, None);
    assert!(trusted.view().is_none());
    assert!(trusted.session("0000000000000000").is_none());
    assert_eq!(trusted.jar_file(&url("https://a.example")), None);
    let dir = std::env::temp_dir().join("mnema-trusted-test");
    let with_dir = Trusted::new(Some(dir.clone()), None);
    let file = with_dir.jar_file(&url("https://a.example/")).unwrap();
    assert!(file.starts_with(dir.join("sessions")));
    assert_ne!(file, with_dir.jar_file(&url("https://b.example/")).unwrap());
}
