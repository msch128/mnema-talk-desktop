//! Interop checks for the ECDSA paths of the RustCrypto provider (p256/p384 0.14).
//!
//! Vectors in the first block were produced by OpenSSL (an independent
//! implementation): the provider must verify what OpenSSL signed and accept the
//! key material OpenSSL emitted. The RFC 6979 vectors pin the deterministic
//! signer. These are throwaway test keys.

use openmls_rust_crypto::RustCrypto;
use openmls_traits::{crypto::OpenMlsCrypto, types::SignatureScheme};

const P256: SignatureScheme = SignatureScheme::ECDSA_SECP256R1_SHA256;
const P384: SignatureScheme = SignatureScheme::ECDSA_SECP384R1_SHA384;

const P256_PRIV: &str = "58a7294d8dc5af31394b7a050e9d2d6c09c6cb3b4f1c20461a7d9f2af2ff8362";
const P256_PUB: &str = "04dff55bf8ecc9ee377180b76b390d7578a1ba5da16f887bce6dd0aa985b33203d0d3e416e4f4e89be9617cafaced2bd4a9fd140ac358de488228d2c0bb4a465f5";
const P256_MSG: &str = "6d6e656d61206f70656e73736c20766563746f722050323536";
const P256_SIG: &str = "3045022100898064ae11053a3ff68b160756a77ea3fce41a66bdbcdc6f59d8b7de68ab478c02202f8377512b9bd62f2a1069dd4be8b010d83c27f64d4f9327d292120a1caa8f06";
const P256_HIGHS: &str = "3046022100898064ae11053a3ff68b160756a77ea3fce41a66bdbcdc6f59d8b7de68ab478c022100d07c88add46429d1d5ef9622b4174feee4aad2b759c80b5d2127b8b8dfb8964b";

const P384_PRIV: &str = "747eb84d843f4046e326dd78719a94ee17758a8f39946dc82e9cf123846454dcc10e1668ad746d93a7ad17bc27439f65";
const P384_PUB: &str = "04acf7d3cf474218dda7f1264d7ee7b13212a76a4e3d422373d84333a7922aa85d263ec0cfe5ec642516d984e6d68dec4b4455c76a498bb4507faf42278310a9a0f7b1c3cdfdf429b9ab2cd3ba93dec880537f4dbbbecea5caa0ef5ecf0fc770ee";
const P384_MSG: &str = "6d6e656d61206f70656e73736c20766563746f722050333834";
const P384_SIG: &str = "3066023100a0b5d29f0b5f26de593a2f8bacedf6e5180d9c4e474796317b435b123d599d07a53507c34ef2e8cea804c4877d3527d9023100852f07fca3cd455b57b722c59c56126f2f3b089f769b48de4942802f26a485ba95ae63bf5dbcf5ae157a22554e23edc0";
const P384_HIGHS: &str = "3065023100a0b5d29f0b5f26de593a2f8bacedf6e5180d9c4e474796317b435b123d599d07a53507c34ef2e8cea804c4877d3527d902307ad0f8035c32baa4a848dd3a63a9ed90d0c4f7608964b7217e20cd52cd92a824c26ba9f2eaf3b1ccd771f7157ea13bb3";

fn hex(s: &str) -> Vec<u8> {
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap())
        .collect()
}

/// Splits a DER ECDSA signature into fixed-width big-endian (r, s).
fn der_rs(der: &[u8], width: usize) -> (Vec<u8>, Vec<u8>) {
    assert_eq!(der[0], 0x30);
    let mut i = if der[1] < 0x80 { 2 } else { 3 };
    let mut parts = Vec::new();
    for _ in 0..2 {
        assert_eq!(der[i], 0x02);
        let n = der[i + 1] as usize;
        let int = &der[i + 2..i + 2 + n];
        let int = &int[int.iter().take_while(|b| **b == 0).count()..];
        let mut out = vec![0u8; width - int.len()];
        out.extend_from_slice(int);
        parts.push(out);
        i += 2 + n;
    }
    (parts.remove(0), parts.remove(0))
}

fn cases() -> Vec<(SignatureScheme, Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>)> {
    vec![
        (
            P256,
            hex(P256_PRIV),
            hex(P256_PUB),
            hex(P256_MSG),
            hex(P256_SIG),
            hex(P256_HIGHS),
        ),
        (
            P384,
            hex(P384_PRIV),
            hex(P384_PUB),
            hex(P384_MSG),
            hex(P384_SIG),
            hex(P384_HIGHS),
        ),
    ]
}

#[test]
fn openssl_signatures_verify() {
    let c = RustCrypto::default();
    for (alg, _, pk, msg, sig, _) in cases() {
        c.verify_signature(alg, &msg, &pk, &sig).unwrap();
    }
}

#[test]
fn openssl_key_material_signs_and_verifies() {
    let c = RustCrypto::default();
    for (alg, sk, pk, msg, _, _) in cases() {
        let sig = c.sign(alg, &msg, &sk).unwrap();
        c.verify_signature(alg, &msg, &pk, &sig).unwrap();
        // RFC 6979 signing is deterministic.
        assert_eq!(sig, c.sign(alg, &msg, &sk).unwrap());
    }
}

#[test]
fn signatures_are_der_encoded() {
    // MLS (RFC 9420 section 5.1.2) carries ECDSA signatures as DER.
    let c = RustCrypto::default();
    for (alg, sk, _, msg, _, _) in cases() {
        let sig = c.sign(alg, &msg, &sk).unwrap();
        let width = if alg == P256 { 32 } else { 48 };
        assert_eq!(sig[0], 0x30);
        assert_eq!(sig[1] as usize, sig.len() - 2);
        let (_, s) = der_rs(&sig, width);
        assert_eq!(s.len(), width);
    }
}

#[test]
fn rfc6979_p256_sha256_sample() {
    // RFC 6979 appendix A.2.5, message "sample".
    let sk = hex("c9afa9d845ba75166b5c215767b1d6934e50c3db36e89b127b8a622b120f6721");
    let pk = hex("0460fed4ba255a9d31c961eb74c6356d68c049b8923b61fa6ce669622e60f29fb67903fe1008b8bc99a41ae9e95628bc64f2f1b20c2d7e9f5177a3c294d4462299");
    let c = RustCrypto::default();
    let sig = c.sign(P256, b"sample", &sk).unwrap();
    let (r, s) = der_rs(&sig, 32);
    assert_eq!(
        r,
        hex("efd48b2aacb6a8fd1140dd9cd45e81d69d2c877b56aaf991c34d0ea84eaf3716")
    );
    assert_eq!(
        s,
        hex("f7cb1c942d657c41d436c7a1b6e29f65f3e900dbb9aff4064dc4ab2f843acda8")
    );
    c.verify_signature(P256, b"sample", &pk, &sig).unwrap();
}

#[test]
fn rejects_tampering_and_malformed_signatures() {
    let c = RustCrypto::default();
    for (alg, _, pk, msg, sig, _) in cases() {
        let mut other = msg.clone();
        other[0] ^= 1;
        assert!(c.verify_signature(alg, &other, &pk, &sig).is_err());

        let mut flipped = sig.clone();
        let last = flipped.len() - 1;
        flipped[last] ^= 1;
        assert!(c.verify_signature(alg, &msg, &pk, &flipped).is_err());

        let mut trailing = sig.clone();
        trailing.push(0);
        assert!(c.verify_signature(alg, &msg, &pk, &trailing).is_err());

        // Fixed-width r||s is not the MLS encoding.
        let width = if alg == P256 { 32 } else { 48 };
        let (r, s) = der_rs(&sig, width);
        let raw = [r, s].concat();
        assert!(c.verify_signature(alg, &msg, &pk, &raw).is_err());

        for bad in [&[][..], &[0x30][..], &sig[..sig.len() - 1], &[0u8; 8][..]] {
            assert!(c.verify_signature(alg, &msg, &pk, bad).is_err());
        }
    }
}

#[test]
fn rejects_malformed_public_keys_without_panicking() {
    let c = RustCrypto::default();
    for (alg, _, pk, msg, sig, _) in cases() {
        let mut off_curve = pk.clone();
        let last = off_curve.len() - 1;
        off_curve[last] ^= 1;
        for bad in [
            &[][..],
            &[0u8][..],
            &pk[..pk.len() - 1],
            &off_curve[..],
            &vec![0x04; pk.len()][..],
            &vec![0u8; pk.len()][..],
        ] {
            assert!(c.verify_signature(alg, &msg, bad, &sig).is_err());
        }
        // A key for the other curve must not verify.
        let other = if alg == P256 {
            hex(P384_PUB)
        } else {
            hex(P256_PUB)
        };
        assert!(c.verify_signature(alg, &msg, &other, &sig).is_err());
    }
}

#[test]
fn rejects_invalid_private_keys_without_panicking() {
    let c = RustCrypto::default();
    for (alg, sk, _, msg, _, _) in cases() {
        for bad in [
            &[][..],
            &sk[..sk.len() - 1],
            &vec![0u8; sk.len()][..],
            &vec![0xff; sk.len()][..],
        ] {
            assert!(c.sign(alg, &msg, bad).is_err());
        }
        let mut long = sk.clone();
        long.push(1);
        assert!(c.sign(alg, &msg, &long).is_err());
    }
}

#[test]
fn generated_keys_are_uncompressed_and_consistent() {
    let c = RustCrypto::default();
    for (alg, pk_len, sk_len) in [(P256, 65, 32), (P384, 97, 48)] {
        let (sk1, pk1) = c.signature_key_gen(alg).unwrap();
        let (sk2, pk2) = c.signature_key_gen(alg).unwrap();
        assert_eq!((sk1.len(), pk1.len()), (sk_len, pk_len));
        assert_eq!(pk1[0], 0x04);
        assert_ne!(sk1, sk2);
        assert_ne!(pk1, pk2);
        let sig = c.sign(alg, b"hello", &sk1).unwrap();
        c.verify_signature(alg, b"hello", &pk1, &sig).unwrap();
        assert!(c.verify_signature(alg, b"hello", &pk2, &sig).is_err());
    }
}

#[test]
fn high_s_signatures_match_upstream_behaviour() {
    // Upstream openmls_rust_crypto (p256/p384 0.13) verified without enforcing
    // low-S, so (r, n - s) is accepted. Pinned so a change is noticed.
    let c = RustCrypto::default();
    for (alg, _, pk, msg, _, high) in cases() {
        assert!(c.verify_signature(alg, &msg, &pk, &high).is_ok());
    }
}
