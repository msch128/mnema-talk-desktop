//! The store: selection by contract and release, marks and floors, key
//! withdrawal, installs and the cache on disk.
use super::*;
use super::{scan::cached_files, state::State};
use crate::web_bundle::fixture::*;

fn dir(tag: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("mnema-bundle-test-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn the_server_is_matched_by_contract_and_release() {
    let mut spec_new = Spec::new("0.9.0");
    spec_new.min_server = "0.8.9";
    let mut spec_old = Spec::new("0.8.0");
    spec_old.min_server = "0.7.0";
    let mut store = Store::load(None, Some(&bundle(&spec_old)), &trusted(7));
    store.install(&bundle(&spec_new), &trusted(7)).unwrap();
    let server = |v: Option<Version>, api| ServerInfo {
        version: v,
        web_client_api: api,
    };
    // The newest bundle that supports the server's release.
    assert_eq!(
        store
            .select(&server(Some(Version(0, 9, 0)), 1))
            .unwrap()
            .bundle
            .version,
        Version(0, 9, 0)
    );
    // A server that claims an older release does not get the older bundle:
    // 0.9.0 was verified, so nothing below it is run for this contract.
    assert_eq!(
        store.select(&server(Some(Version(0, 8, 5)), 1)).err(),
        Some(Refusal::ServerTooOld {
            needs: Version(0, 8, 9)
        })
    );
    // A development server only has to speak the contract.
    assert_eq!(
        store.select(&server(None, 1)).unwrap().bundle.version,
        Version(0, 9, 0)
    );
    // Too old for every bundle.
    assert_eq!(
        store.select(&server(Some(Version(0, 6, 0)), 1)).err(),
        Some(Refusal::ServerTooOld {
            needs: Version(0, 8, 9)
        })
    );
    // A newer contract than any bundle: an update may help.
    assert_eq!(
        store.select(&server(Some(Version(0, 9, 0)), 2)).err(),
        Some(Refusal::ServerNewer)
    );
    // Nothing speaks an older contract any more.
    let mut api2 = Spec::new("1.0.0");
    api2.api = 2;
    let store2 = Store::load(None, Some(&bundle(&api2)), &trusted(7));
    assert_eq!(
        store2.select(&server(None, 1)).err(),
        Some(Refusal::ServerContractRetired)
    );
    assert_eq!(
        Store::load(None, None, &trusted(7))
            .select(&server(None, 1))
            .err(),
        Some(Refusal::NoBundle)
    );
    for refusal in [
        Refusal::NoBundle,
        Refusal::ServerNewer,
        Refusal::ServerTooOld {
            needs: Version(0, 7, 0),
        },
        Refusal::ServerContractRetired,
        Refusal::RolledBack,
    ] {
        assert!(!refusal.to_string().is_empty());
    }
}

#[test]
fn the_high_water_mark_blocks_older_bundles_even_without_a_floor() {
    let d = dir("hwm");
    let old = Spec::new("0.8.0");
    let mut store = Store::load(Some(&d), Some(&bundle(&old)), &trusted(7));
    store
        .install(&bundle(&Spec::new("0.8.4")), &trusted(7))
        .unwrap();
    assert_eq!(store.floor(), Version(0, 0, 0));
    assert_eq!(store.high_water(1), Some(Version(0, 8, 4)));
    let server = ServerInfo {
        version: Some(Version(0, 1, 0)),
        web_client_api: 1,
    };
    // Whatever the server claims, the built-in 0.8.0 is not offered.
    assert_eq!(
        store.select(&server).unwrap().bundle.version,
        Version(0, 8, 4)
    );
    // The mark survives a restart and the loss of the newer bundle: the
    // client refuses instead of running the older one.
    std::fs::remove_file(d.join("mnema-web-0.8.4.zip")).unwrap();
    let again = Store::load(Some(&d), Some(&bundle(&old)), &trusted(7));
    assert_eq!(again.high_water(1), Some(Version(0, 8, 4)));
    assert_eq!(again.select(&server).err(), Some(Refusal::RolledBack));
    // A lost state file is rebuilt from what verifies, never lowered.
    std::fs::remove_file(d.join("state.json")).unwrap();
    let rebuilt = Store::load(Some(&d), Some(&bundle(&old)), &trusted(7));
    assert_eq!(rebuilt.high_water(1), Some(Version(0, 8, 0)));
    // Another contract is tracked on its own.
    let mut api2 = Spec::new("0.9.0");
    api2.api = 2;
    let mut other = Store::load(Some(&d), Some(&bundle(&old)), &trusted(7));
    other.install(&bundle(&api2), &trusted(7)).unwrap();
    assert_eq!(other.high_water(1), Some(Version(0, 8, 0)));
    assert_eq!(other.high_water(2), Some(Version(0, 9, 0)));
    assert_eq!(
        other
            .select(&ServerInfo {
                version: None,
                web_client_api: 1
            })
            .unwrap()
            .bundle
            .version,
        Version(0, 8, 0)
    );
    let _ = std::fs::remove_dir_all(&d);
}

fn both_keys() -> Vec<TrustedKey> {
    let mut keys = trusted(7);
    keys.extend(trusted(5));
    keys
}

fn server() -> ServerInfo {
    ServerInfo {
        version: None,
        web_client_api: 1,
    }
}

fn forged_and_good(d: &Path) -> Store {
    let good = Spec::new("0.8.0");
    let mut forged = Spec::new("0.99.0");
    forged.seed = 5;
    forged.floor = "0.99.0";
    let mut store = Store::load(Some(d), Some(&bundle(&good)), &both_keys());
    store.install(&bundle(&forged), &both_keys()).unwrap();
    store
}

#[test]
fn a_forged_mark_of_a_revoked_key_is_cleared() {
    let d = dir("revoke");
    let mut store = forged_and_good(&d);
    assert_eq!(store.floor(), Version(0, 99, 0));
    assert!(store.reconcile(&trusted(7)));
    assert!(!d.join("mnema-web-0.99.0.zip").exists());
    assert_eq!(store.floor(), Version(0, 0, 0));
    assert_eq!(store.high_water(1), Some(Version(0, 8, 0)));
    // The good built-in bundle, held back by the forged floor, runs again.
    assert_eq!(
        store.select(&server()).unwrap().bundle.version,
        Version(0, 8, 0)
    );
    assert!(!store.reconcile(&trusted(7)));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_cleared_marks_stay_cleared_after_a_restart() {
    let d = dir("revoke-restart");
    drop(forged_and_good(&d));
    let again = Store::load(Some(&d), Some(&bundle(&Spec::new("0.8.0"))), &trusted(7));
    assert_eq!(again.rejected.len(), 1);
    assert_eq!(again.floor(), Version(0, 0, 0));
    assert_eq!(again.high_water(1), Some(Version(0, 8, 0)));
    assert_eq!(
        again.select(&server()).unwrap().bundle.version,
        Version(0, 8, 0)
    );
    // With both keys trusted the forged marks are in force again.
    let both = Store::load(Some(&d), Some(&bundle(&Spec::new("0.8.0"))), &both_keys());
    assert_eq!(both.floor(), Version(0, 99, 0));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn marks_and_floors_of_still_trusted_keys_are_kept() {
    let d = dir("keep");
    let mut legit = Spec::new("0.9.0");
    legit.floor = "0.8.5";
    let mut forged = Spec::new("0.99.0");
    forged.seed = 5;
    forged.floor = "0.99.0";
    let mut store = Store::load(Some(&d), Some(&bundle(&Spec::new("0.8.0"))), &both_keys());
    store.install(&bundle(&legit), &both_keys()).unwrap();
    store.install(&bundle(&forged), &both_keys()).unwrap();
    assert!(store.reconcile(&trusted(7)));
    // Never below what the trusted key reached or declared.
    assert_eq!(store.floor(), Version(0, 8, 5));
    assert_eq!(store.high_water(1), Some(Version(0, 9, 0)));
    assert_eq!(
        store.select(&server()).unwrap().bundle.version,
        Version(0, 9, 0)
    );
    // The built-in 0.8.0 stays below the trusted floor.
    assert_eq!(store.held().len(), 1);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn when_only_revoked_bundles_are_left_the_client_says_so_until_a_new_one_arrives() {
    let mut forged = Spec::new("0.8.0");
    forged.seed = 5;
    let mut store = Store::load(None, Some(&bundle(&forged)), &trusted(5));
    assert!(store.select(&server()).is_ok());
    assert!(store.reconcile(&trusted(7)));
    assert_eq!(
        store.select(&server()).err(),
        Some(Refusal::SignerWithdrawn)
    );
    assert!(
        Refusal::SignerWithdrawn
            .to_string()
            .contains("no longer trusted")
    );
    store
        .install(&bundle(&Spec::new("0.8.1")), &trusted(7))
        .unwrap();
    assert_eq!(
        store.select(&server()).unwrap().bundle.version,
        Version(0, 8, 1)
    );
    // An offline start with a built-in bundle nobody trusts any more.
    let offline = Store::load(None, Some(&bundle(&forged)), &trusted(7));
    assert_eq!(
        offline.select(&server()).err(),
        Some(Refusal::SignerWithdrawn)
    );
}

#[test]
fn a_cap_from_the_statement_lowers_marks_even_of_unknown_or_trusted_signers() {
    // A state written before signers were recorded: forged marks of an
    // unknown signer always count, and only a cap can clear them.
    let d = dir("cap");
    std::fs::write(
        d.join("state.json"),
        serde_json::json!({
            "security_floor": [0, 99, 0],
            "high_water": {"1": [0, 99, 0]},
            "last_check": 0
        })
        .to_string(),
    )
    .unwrap();
    let mut store = Store::load(Some(&d), Some(&bundle(&Spec::new("0.8.0"))), &trusted(7));
    assert_eq!(store.floor(), Version(0, 99, 0));
    assert!(store.select(&server()).is_err());
    let caps = BTreeMap::from([(1, Version(0, 8, 0))]);
    store.apply_caps(&caps);
    assert_eq!(store.floor(), Version(0, 8, 0));
    assert_eq!(store.high_water(1), Some(Version(0, 8, 0)));
    assert_eq!(
        store.select(&server()).unwrap().bundle.version,
        Version(0, 8, 0)
    );
    // The cap survives a restart in the recorded marks.
    let again = Store::load(Some(&d), Some(&bundle(&Spec::new("0.8.0"))), &trusted(7));
    assert_eq!(again.floor(), Version(0, 8, 0));
    // Bundles above the cap are dropped, even from a trusted key; other
    // contracts keep their own marks; no caps change nothing.
    let mut high = Spec::new("0.99.0");
    high.floor = "0.5.0";
    let mut api2 = Spec::new("1.0.0");
    api2.api = 2;
    let mut store = Store::load(None, Some(&bundle(&Spec::new("0.8.0"))), &trusted(7));
    store.install(&bundle(&high), &trusted(7)).unwrap();
    store.install(&bundle(&api2), &trusted(7)).unwrap();
    store.apply_caps(&BTreeMap::new());
    assert_eq!(store.high_water(1), Some(Version(0, 99, 0)));
    store.apply_caps(&BTreeMap::from([(1, Version(0, 9, 0))]));
    assert_eq!(store.high_water(1), Some(Version(0, 9, 0)));
    assert_eq!(store.high_water(2), Some(Version(1, 0, 0)));
    assert!(
        store
            .held()
            .iter()
            .filter(|h| h.bundle.web_client_api == 1)
            .all(|h| h.bundle.version <= Version(0, 9, 0))
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn installs_use_the_keys_in_force_when_the_store_lock_is_held() {
    use std::sync::Mutex;
    let store = Mutex::new(Store::load(None, None, &both_keys()));
    let keys = Mutex::new(both_keys());
    let mut forged = Spec::new("0.8.0");
    forged.seed = 5;
    // A statement withdrawing key 5 lands after the keys were first read.
    *keys.lock().unwrap() = trusted(7);
    assert!(install_current(&store, &keys, &bundle(&forged)).is_err());
    assert!(store.lock().unwrap().held().is_empty());
    assert!(install_current(&store, &keys, &bundle(&Spec::new("0.8.0"))).is_ok());
}

#[test]
fn the_security_floor_only_moves_up_and_withdraws_older_bundles() {
    let d = dir("floor");
    let old = Spec::new("0.8.0");
    let mut fixed = Spec::new("0.8.2");
    fixed.floor = "0.8.1";
    let mut store = Store::load(Some(&d), Some(&bundle(&old)), &trusted(7));
    assert_eq!(store.floor(), Version(0, 0, 0));
    store.install(&bundle(&fixed), &trusted(7)).unwrap();
    assert_eq!(store.floor(), Version(0, 8, 1));
    // The built-in 0.8.0 is below the floor now and cannot be selected.
    assert_eq!(store.held().len(), 1);
    assert_eq!(store.newest().unwrap().bundle.version, Version(0, 8, 2));
    // The floor survives a restart, even if the bundle that raised it is gone.
    std::fs::remove_file(d.join("mnema-web-0.8.2.zip")).unwrap();
    let again = Store::load(Some(&d), Some(&bundle(&old)), &trusted(7));
    assert_eq!(again.floor(), Version(0, 8, 1));
    assert!(again.newest().is_none());
    // Nor can an old bundle come back through an update.
    let mut again = Store::load(Some(&d), None, &trusted(7));
    assert!(again.install(&bundle(&old), &trusted(7)).is_err());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn updates_install_only_newer_verified_bundles_and_survive_a_restart() {
    let d = dir("install");
    let mut store = Store::load(Some(&d), Some(&bundle(&Spec::new("0.8.0"))), &trusted(7));
    assert!(
        store
            .install(&bundle(&Spec::new("0.8.0")), &trusted(7))
            .is_err(),
        "equal"
    );
    assert!(
        store
            .install(&bundle(&Spec::new("0.7.0")), &trusted(7))
            .is_err(),
        "older"
    );
    assert!(store.install(b"garbage", &trusted(7)).is_err());
    let mut other = Spec::new("0.9.0");
    other.seed = 9;
    assert!(
        store.install(&bundle(&other), &trusted(7)).is_err(),
        "wrong key"
    );
    assert_eq!(
        store
            .install(&bundle(&Spec::new("0.8.1")), &trusted(7))
            .unwrap(),
        Version(0, 8, 1)
    );
    assert_eq!(store.newest().unwrap().source, Source::Cache);
    let reloaded = Store::load(Some(&d), Some(&bundle(&Spec::new("0.8.0"))), &trusted(7));
    assert_eq!(reloaded.held().len(), 2);
    assert_eq!(reloaded.newest().unwrap().bundle.version, Version(0, 8, 1));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_modified_cached_bundle_is_refused_and_the_next_verified_one_is_used() {
    let d = dir("tamper");
    let mut store = Store::load(Some(&d), Some(&bundle(&Spec::new("0.8.0"))), &trusted(7));
    store
        .install(&bundle(&Spec::new("0.8.1")), &trusted(7))
        .unwrap();
    // Flip one byte of the cached copy's content.
    let path = d.join("mnema-web-0.8.1.zip");
    let mut bytes = std::fs::read(&path).unwrap();
    let at = bytes
        .windows(14)
        .position(|w| w == b"console.log(1)")
        .unwrap();
    bytes[at] ^= 1;
    std::fs::write(&path, bytes).unwrap();
    let reloaded = Store::load(Some(&d), Some(&bundle(&Spec::new("0.8.0"))), &trusted(7));
    assert_eq!(reloaded.rejected, vec!["mnema-web-0.8.1.zip".to_owned()]);
    assert_eq!(reloaded.held().len(), 1);
    assert_eq!(reloaded.newest().unwrap().source, Source::Builtin);
    assert_eq!(reloaded.newest().unwrap().bundle.version, Version(0, 8, 0));
    // And the damaged file does not stop the update from replacing it.
    let mut reloaded = reloaded;
    assert_eq!(
        reloaded
            .install(&bundle(&Spec::new("0.8.1")), &trusted(7))
            .unwrap(),
        Version(0, 8, 1)
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[cfg(unix)]
#[test]
fn a_cached_bundle_that_cannot_be_read_is_kept_for_the_next_start() {
    use std::os::unix::fs::PermissionsExt;
    let d = dir("locked");
    let mut store = Store::load(Some(&d), Some(&bundle(&Spec::new("0.8.0"))), &trusted(7));
    store
        .install(&bundle(&Spec::new("0.8.1")), &trusted(7))
        .unwrap();
    let path = d.join("mnema-web-0.8.1.zip");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
    // A superuser reads through the mode; the test only means something without.
    if std::fs::read(&path).is_ok() {
        let _ = std::fs::remove_dir_all(&d);
        return;
    }
    let locked = Store::load(Some(&d), Some(&bundle(&Spec::new("0.8.0"))), &trusted(7));
    assert_eq!(locked.rejected, vec!["mnema-web-0.8.1.zip".to_owned()]);
    assert!(path.exists(), "a read failure does not delete the file");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    let again = Store::load(Some(&d), Some(&bundle(&Spec::new("0.8.0"))), &trusted(7));
    assert_eq!(again.newest().unwrap().bundle.version, Version(0, 8, 1));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_broken_builtin_bundle_is_reported_and_unusable() {
    let store = Store::load(None, Some(b"not a bundle"), &trusted(7));
    assert_eq!(store.rejected, vec!["built-in".to_owned()]);
    assert!(store.newest().is_none());
}

#[test]
fn the_cache_keeps_only_the_newest_few_versions_and_ignores_strangers() {
    let d = dir("prune");
    std::fs::write(d.join("notes.txt"), "x").unwrap();
    std::fs::write(d.join("mnema-web-9.9.9.zip"), vec![0u8; 10]).unwrap();
    let mut store = Store::load(Some(&d), Some(&bundle(&Spec::new("0.8.0"))), &trusted(7));
    for v in ["0.8.1", "0.8.2", "0.8.3", "0.8.4"] {
        store.install(&bundle(&Spec::new(v)), &trusted(7)).unwrap();
    }
    let versions: Vec<_> = store
        .held()
        .iter()
        .map(|h| h.bundle.version.to_string())
        .collect();
    assert_eq!(versions, ["0.8.4", "0.8.3", "0.8.2", "0.8.0"]);
    assert!(!d.join("mnema-web-0.8.1.zip").exists());
    assert!(d.join("notes.txt").exists());
    assert_eq!(store.rejected, vec!["mnema-web-9.9.9.zip".to_owned()]);
    assert!(!d.join("mnema-web-9.9.9.zip").exists());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_patch_for_an_older_contract_is_not_shadowed_by_a_newer_one() {
    let mut api2 = Spec::new("0.9.0");
    api2.api = 2;
    let mut store = Store::load(None, Some(&bundle(&api2)), &trusted(7));
    // 0.8.5 speaks contract 1: it is new for that contract.
    assert_eq!(
        store
            .install(&bundle(&Spec::new("0.8.5")), &trusted(7))
            .unwrap(),
        Version(0, 8, 5)
    );
    // Same contract and not newer, or a version already held: refused.
    assert!(
        store
            .install(&bundle(&Spec::new("0.8.4")), &trusted(7))
            .is_err()
    );
    assert!(
        store
            .install(&bundle(&Spec::new("0.8.5")), &trusted(7))
            .is_err()
    );
    let info = |api| ServerInfo {
        version: None,
        web_client_api: api,
    };
    assert_eq!(
        store.select(&info(1)).unwrap().bundle.version,
        Version(0, 8, 5)
    );
    assert_eq!(
        store.select(&info(2)).unwrap().bundle.version,
        Version(0, 9, 0)
    );
}

#[test]
fn only_a_withdrawn_signer_picks_the_withdrawn_message() {
    // A damaged file is not a withdrawn signer, and it is removed.
    let d = dir("withdrawn-damaged");
    std::fs::write(d.join("mnema-web-0.1.0.zip"), b"garbage").unwrap();
    let store = Store::load(Some(&d), None, &trusted(7));
    assert_eq!(store.rejected, vec!["mnema-web-0.1.0.zip".to_owned()]);
    assert_eq!(store.empty_reason(), Refusal::NoBundle);
    assert!(!d.join("mnema-web-0.1.0.zip").exists());
    let _ = std::fs::remove_dir_all(&d);

    // A bundle of a key that is not trusted is, and its file is kept.
    let d = dir("withdrawn-key");
    let mut other = Spec::new("0.2.0");
    other.seed = 5;
    std::fs::write(d.join("mnema-web-0.2.0.zip"), bundle(&other)).unwrap();
    let mut store = Store::load(Some(&d), None, &trusted(7));
    assert_eq!(store.empty_reason(), Refusal::SignerWithdrawn);
    assert!(d.join("mnema-web-0.2.0.zip").exists());
    // A trusted bundle installed afterwards clears it.
    store
        .install(&bundle(&Spec::new("0.3.0")), &trusted(7))
        .unwrap();
    assert!(!store.withdrawn);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_damaged_state_file_falls_back_to_the_last_good_one() {
    let d = dir("state-backup");
    let mut floor = Spec::new("0.8.4");
    floor.floor = "0.8.2";
    let mut store = Store::load(Some(&d), Some(&bundle(&Spec::new("0.8.0"))), &trusted(7));
    store.install(&bundle(&floor), &trusted(7)).unwrap();
    store
        .install(&bundle(&Spec::new("0.8.5")), &trusted(7))
        .unwrap();
    assert!(!d.join("state.json.partial").exists());
    // A torn write, and the bundles that carried the floor are gone.
    std::fs::write(d.join("state.json"), b"{\"floors\":").unwrap();
    for v in ["0.8.4", "0.8.5"] {
        std::fs::remove_file(d.join(format!("mnema-web-{v}.zip"))).unwrap();
    }
    let again = Store::load(Some(&d), Some(&bundle(&Spec::new("0.8.0"))), &trusted(7));
    assert_eq!(again.floor(), Version(0, 8, 2));
    // The next save is whole again.
    let raw = std::fs::read(d.join("state.json")).unwrap();
    assert!(serde_json::from_slice::<State>(&raw).is_ok());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn cached_files_are_ordered_by_version_and_leftovers_are_removed() {
    let d = dir("scan");
    for name in [
        "mnema-web-0.9.0.zip",
        "mnema-web-0.10.0.zip",
        "mnema-web-1.2.0.zip",
        "mnema-web-1.2.0.zip.partial",
        "mnema-web-x.zip",
        "other.zip",
    ] {
        std::fs::write(d.join(name), b"x").unwrap();
    }
    let versions: Vec<_> = cached_files(Some(&d)).into_iter().map(|(v, _)| v).collect();
    assert_eq!(
        versions,
        [Version(1, 2, 0), Version(0, 10, 0), Version(0, 9, 0)]
    );
    assert!(!d.join("mnema-web-1.2.0.zip.partial").exists());
    assert!(d.join("mnema-web-x.zip").exists());
    assert!(d.join("other.zip").exists());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_duplicate_of_the_builtin_version_is_removed_from_the_cache() {
    let d = dir("duplicate");
    let spec = Spec::new("0.8.0");
    std::fs::write(d.join("mnema-web-0.8.0.zip"), bundle(&spec)).unwrap();
    let store = Store::load(Some(&d), Some(&bundle(&spec)), &trusted(7));
    assert_eq!(store.held().len(), 1);
    assert!(!d.join("mnema-web-0.8.0.zip").exists());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn shelved_bundles_are_bounded_too() {
    let d = dir("shelved");
    let mut store = Store::load(Some(&d), Some(&bundle(&Spec::new("0.8.0"))), &trusted(7));
    for v in ["0.8.1", "0.8.2", "0.8.3"] {
        store.install(&bundle(&Spec::new(v)), &trusted(7)).unwrap();
    }
    for v in ["0.9.0", "0.9.1"] {
        let mut raised = Spec::new(v);
        raised.floor = v;
        store.install(&bundle(&raised), &trusted(7)).unwrap();
    }
    assert_eq!(store.newest().unwrap().bundle.version, Version(0, 9, 1));
    assert!(!d.join("mnema-web-0.8.1.zip").exists());
    for v in ["0.8.2", "0.8.3", "0.9.0", "0.9.1"] {
        assert!(d.join(format!("mnema-web-{v}.zip")).exists(), "{v}");
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn update_checks_are_rate_limited_and_remembered() {
    let d = dir("check");
    let mut store = Store::load(Some(&d), None, &trusted(7));
    assert!(store.check_due(1_000_000));
    store.mark_checked(1_000_000);
    assert!(!store.check_due(1_000_000 + CHECK_INTERVAL_SECS - 1));
    assert!(store.check_due(1_000_000 + CHECK_INTERVAL_SECS));
    assert!(!Store::load(Some(&d), None, &trusted(7)).check_due(1_000_001));
    let _ = std::fs::remove_dir_all(&d);
}
