//! Group lifecycle across the ciphersuites the Mnema clients can negotiate:
//! the web client's MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519 and the ECDSA
//! P-256 suite whose signature and key paths run on RustCrypto p256 0.14.
//! Join by Welcome, application messages both ways, an add commit, an update
//! commit and a removal, each processed by the other members.
use openmls::prelude::{tls_codec::*, *};
use openmls_basic_credential::SignatureKeyPair;
use openmls_rust_crypto::OpenMlsRustCrypto;

struct Member {
    provider: OpenMlsRustCrypto,
    signer: SignatureKeyPair,
    credential: CredentialWithKey,
}

fn member(name: &str, cs: Ciphersuite) -> Member {
    let provider = OpenMlsRustCrypto::default();
    let signer = SignatureKeyPair::new(cs.signature_algorithm()).unwrap();
    signer.store(provider.storage()).unwrap();
    let credential = CredentialWithKey {
        credential: BasicCredential::new(name.as_bytes().to_vec()).into(),
        signature_key: signer.to_public_vec().into(),
    };
    Member {
        provider,
        signer,
        credential,
    }
}

fn key_package(m: &Member, cs: Ciphersuite) -> KeyPackage {
    KeyPackage::builder()
        .build(cs, &m.provider, &m.signer, m.credential.clone())
        .unwrap()
        .key_package()
        .clone()
}

fn deliver(to: &mut MlsGroup, m: &Member, msg: MlsMessageOut) -> ProcessedMessageContent {
    let bytes = msg.tls_serialize_detached().unwrap();
    let msg = MlsMessageIn::tls_deserialize_exact(&bytes).unwrap();
    let protocol = msg.try_into_protocol_message().unwrap();
    let processed = to.process_message(&m.provider, protocol).unwrap();
    processed.into_content()
}

fn join(m: &Member, welcome: MlsMessageOut) -> MlsGroup {
    let bytes = welcome.tls_serialize_detached().unwrap();
    let MlsMessageBodyIn::Welcome(w) = MlsMessageIn::tls_deserialize_exact(&bytes)
        .unwrap()
        .extract()
    else {
        panic!("expected a welcome");
    };
    StagedWelcome::new_from_welcome(
        &m.provider,
        &MlsGroupJoinConfig::builder()
            .use_ratchet_tree_extension(true)
            .build(),
        w,
        None,
    )
    .unwrap()
    .into_group(&m.provider)
    .unwrap()
}

fn merge(to: &mut MlsGroup, m: &Member, commit: MlsMessageOut) {
    match deliver(to, m, commit) {
        ProcessedMessageContent::StagedCommitMessage(c) => {
            to.merge_staged_commit(&m.provider, *c).unwrap()
        }
        _ => panic!("expected a commit"),
    }
}

fn say(from: &mut MlsGroup, f: &Member, to: &mut MlsGroup, t: &Member, text: &[u8]) {
    let msg = from.create_message(&f.provider, &f.signer, text).unwrap();
    match deliver(to, t, msg) {
        ProcessedMessageContent::ApplicationMessage(a) => assert_eq!(a.into_bytes(), text),
        _ => panic!("expected an application message"),
    }
}

fn run(cs: Ciphersuite) {
    let (alice, bob, carol) = (member("alice", cs), member("bob", cs), member("carol", cs));
    let config = MlsGroupCreateConfig::builder()
        .ciphersuite(cs)
        .use_ratchet_tree_extension(true)
        .build();
    let mut a = MlsGroup::new(
        &alice.provider,
        &alice.signer,
        &config,
        alice.credential.clone(),
    )
    .unwrap();

    let (_, welcome, _) = a
        .add_members(&alice.provider, &alice.signer, &[key_package(&bob, cs)])
        .unwrap();
    a.merge_pending_commit(&alice.provider).unwrap();
    let mut b = join(&bob, welcome);
    assert_eq!(a.epoch(), b.epoch());
    assert_eq!(
        a.export_secret(alice.provider.crypto(), "mnema", b"ctx", 32)
            .unwrap(),
        b.export_secret(bob.provider.crypto(), "mnema", b"ctx", 32)
            .unwrap()
    );
    say(&mut a, &alice, &mut b, &bob, b"hello bob");
    say(&mut b, &bob, &mut a, &alice, b"hello alice");

    // Bob adds Carol; Alice processes the commit, Carol joins by Welcome.
    let (commit, welcome, _) = b
        .add_members(&bob.provider, &bob.signer, &[key_package(&carol, cs)])
        .unwrap();
    b.merge_pending_commit(&bob.provider).unwrap();
    merge(&mut a, &alice, commit);
    let mut c = join(&carol, welcome);
    assert_eq!(a.epoch(), c.epoch());
    say(&mut c, &carol, &mut a, &alice, b"carol here");

    // Alice updates her leaf; Bob and Carol process the commit.
    let (commit, _, _) = a
        .self_update(
            &alice.provider,
            &alice.signer,
            LeafNodeParameters::default(),
        )
        .unwrap()
        .into_messages();
    a.merge_pending_commit(&alice.provider).unwrap();
    merge(&mut b, &bob, commit.clone());
    merge(&mut c, &carol, commit);
    say(&mut b, &bob, &mut c, &carol, b"after update");

    // Alice removes Carol; Bob follows, Carol is out.
    let carol_index = c.own_leaf_index();
    let (commit, _, _) = a
        .remove_members(&alice.provider, &alice.signer, &[carol_index])
        .unwrap();
    a.merge_pending_commit(&alice.provider).unwrap();
    merge(&mut b, &bob, commit.clone());
    match deliver(&mut c, &carol, commit) {
        ProcessedMessageContent::StagedCommitMessage(staged) => {
            assert!(staged.self_removed());
        }
        _ => panic!("expected a commit"),
    }
    assert_eq!(a.epoch(), b.epoch());
    say(&mut a, &alice, &mut b, &bob, b"two left");
}

#[test]
fn x25519_ed25519_suite_roundtrip() {
    run(Ciphersuite::MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519);
}

#[test]
fn p256_ecdsa_suite_roundtrip() {
    run(Ciphersuite::MLS_128_DHKEMP256_AES128GCM_SHA256_P256);
}
