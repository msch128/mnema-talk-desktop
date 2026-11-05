//! The statement itself: its fields, strict time parsing, and verification
//! (signature first, then parse, then validate).
use super::{MAX_STATEMENT, updater::minisign_key_id};
use crate::web_bundle::{TrustedKey, Version, is_hex_id16, parse_key};
use base64::{Engine, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signature, VerifyingKey};
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Statement {
    pub schema: u32,
    pub serial: u64,
    pub issued_at: String,
    pub expires_at: String,
    pub bundle_keys: Vec<String>,
    pub revoked_bundle_keys: Vec<String>,
    pub updater_keys: Vec<String>,
    pub revoked_updater_keys: Vec<String>,
    /// Optional caps on the rollback marks, per web client API number.
    #[serde(default, deserialize_with = "crate::web_json::unique_map")]
    pub max_version: BTreeMap<String, String>,
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Seconds since the epoch for `YYYY-MM-DDTHH:MM:SSZ` (a real calendar date),
/// nothing else.
pub fn parse_utc(text: &str) -> Option<u64> {
    let b = text.as_bytes();
    if b.len() != 20
        || [
            (4, b'-'),
            (7, b'-'),
            (10, b'T'),
            (13, b':'),
            (16, b':'),
            (19, b'Z'),
        ]
        .iter()
        .any(|&(i, c)| b[i] != c)
    {
        return None;
    }
    let num = |from: usize, to: usize| -> Option<i64> {
        let digits = text.get(from..to)?;
        digits
            .bytes()
            .all(|c| c.is_ascii_digit())
            .then(|| digits.parse().ok())?
    };
    let (y, m, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (hh, mm, ss) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    if !(1..=12).contains(&m) || d < 1 || d > days_in_month(y, m) || hh > 23 || mm > 59 || ss > 59 {
        return None;
    }
    // Days since 1970-01-01 (Howard Hinnant's civil-to-days).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    u64::try_from(days * 86400 + hh * 3600 + mm * 60 + ss).ok()
}

/// Longest validity a statement may claim (the signing tool's bound).
const MAX_VALIDITY_SECS: u64 = 400 * 86400;

/// Verifies the signature over the exact bytes, then parses and validates.
pub fn verify_statement(doc: &[u8], sig: &[u8], root: &VerifyingKey) -> Result<Statement, String> {
    if doc.len() > MAX_STATEMENT {
        return Err("trust statement too large".into());
    }
    let sig: [u8; 64] = STANDARD
        .decode(String::from_utf8_lossy(sig).trim())
        .ok()
        .and_then(|raw| raw.try_into().ok())
        .ok_or("malformed trust signature")?;
    root.verify_strict(doc, &Signature::from_bytes(&sig))
        .map_err(|_| "trust signature does not match the root key")?;
    let statement: Statement = crate::web_json::parse(doc).ok_or("unreadable trust statement")?;
    statement.validate()?;
    Ok(statement)
}

impl Statement {
    fn validate(&self) -> Result<(), String> {
        if self.schema != 1 || self.serial < 1 {
            return Err("unsupported trust statement".into());
        }
        let issued = parse_utc(&self.issued_at).ok_or("invalid issued_at")?;
        let expires = parse_utc(&self.expires_at).ok_or("invalid expires_at")?;
        if expires <= issued || expires - issued > MAX_VALIDITY_SECS {
            return Err("invalid validity period".into());
        }
        for (api, version) in &self.max_version {
            if api.parse::<u32>().ok().filter(|n| *n >= 1).is_none()
                || Version::parse(version).is_none()
            {
                return Err("invalid max_version".into());
            }
        }
        if self.bundle_keys.is_empty() {
            return Err("trust statement lists no bundle key".into());
        }
        if !self.revoked_bundle_keys.iter().all(|id| is_hex_id16(id))
            || !self.revoked_updater_keys.iter().all(|id| is_hex_id16(id))
        {
            return Err("invalid revoked key id".into());
        }
        for raw in &self.bundle_keys {
            let key = parse_key(raw).ok_or("invalid bundle key")?;
            if self.revoked_bundle_keys.contains(&key.id) {
                return Err("a bundle key is both allowed and revoked".into());
            }
        }
        for key in &self.updater_keys {
            let id = minisign_key_id(key).ok_or("invalid updater key")?;
            if self.revoked_updater_keys.contains(&id) {
                return Err("an updater key is both allowed and revoked".into());
            }
        }
        Ok(())
    }

    /// The caps as numbers (validated when the statement was verified).
    pub fn caps(&self) -> BTreeMap<u32, Version> {
        self.max_version
            .iter()
            .filter_map(|(api, v)| Some((api.parse().ok()?, Version::parse(v)?)))
            .collect()
    }

    /// Whether the statement is past `expires_at` at `now` (unix seconds).
    pub fn expired(&self, now: u64) -> bool {
        parse_utc(&self.expires_at).is_none_or(|e| now >= e)
    }

    /// Allowed web bundle keys that are not revoked.
    pub fn bundle_keys(&self) -> Vec<TrustedKey> {
        self.bundle_keys
            .iter()
            .filter_map(|raw| parse_key(raw))
            .filter(|k| !self.revoked_bundle_keys.contains(&k.id))
            .collect()
    }
}
