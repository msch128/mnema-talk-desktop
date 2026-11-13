//! What the error dialog says about a download that saved nothing. The same
//! table is in the Linux client; `download_errors` in `test-vectors/proxy.json`
//! keeps the two equal.
use crate::web_proxy::{Disk, Failure};

/// The sentences, by key. `{message}`, `{status}` and `{reason}` are filled in.
pub const SENTENCES: &[(&str, &str)] = &[
    ("bad_path", "This link cannot be downloaded."),
    (
        "unreachable",
        "The server could not be reached, so nothing was saved. Check the connection and try again.",
    ),
    (
        "gone",
        "There is nothing to download: the file was downloaded already, removed or has expired. For your data export, request a new one under Settings.",
    ),
    (
        "refused",
        "The server refused the download: {message}. Try again later.",
    ),
    (
        "failed",
        "The download failed (HTTP {status}). Try again later.",
    ),
    (
        "too_large",
        "The file is larger than this client accepts, so nothing was saved.",
    ),
    (
        "cut",
        "The download was interrupted, so nothing was saved. Try again.",
    ),
    (
        "disk",
        "The file could not be saved: {reason}. Check the folder, then try again.",
    ),
    (
        "busy",
        "Another download is still in progress. Try again when it has finished.",
    ),
    (
        "no_folder",
        "There is no Downloads folder to save the file in. Create one, then try again.",
    ),
    (
        "webview_failed",
        "The download failed, so nothing was saved. Try again.",
    ),
];

/// Why a file could not be written, by key.
pub const REASONS: &[(&str, &str)] = &[
    ("no_space", "there is no space left on the disk"),
    ("denied", "this app may not write to the folder"),
    ("read_only", "the disk is read-only"),
    ("missing", "the folder does not exist"),
    ("name_too_long", "the name is too long for this disk"),
    ("quota", "the disk quota is used up"),
    ("other", "the disk reported an error"),
];

fn lookup(table: &[(&str, &'static str)], key: &str) -> &'static str {
    table
        .iter()
        .find(|(k, _)| *k == key)
        .map_or("", |(_, text)| text)
}

/// The sentence `key` of the table.
pub fn text(key: &str) -> String {
    lookup(SENTENCES, key).to_owned()
}

fn reason_key(reason: Disk) -> &'static str {
    match reason {
        Disk::NoSpace => "no_space",
        Disk::Denied => "denied",
        Disk::ReadOnly => "read_only",
        Disk::Missing => "missing",
        Disk::NameTooLong => "name_too_long",
        Disk::Quota => "quota",
        Disk::Other => "other",
    }
}

/// What the error dialog says about `failure`.
pub fn sentence(failure: &Failure) -> String {
    match failure {
        Failure::Path => text("bad_path"),
        Failure::Unreachable => text("unreachable"),
        Failure::Refused(404, _) => text("gone"),
        Failure::Refused(_, Some(message)) => text("refused").replace("{message}", message),
        Failure::Refused(status, None) => text("failed").replace("{status}", &status.to_string()),
        Failure::TooLarge => text("too_large"),
        Failure::Cut => text("cut"),
        Failure::Disk(reason) => {
            text("disk").replace("{reason}", lookup(REASONS, reason_key(*reason)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web_proxy::clean_message;

    fn vectors() -> serde_json::Value {
        let all: serde_json::Value =
            serde_json::from_str(include_str!("../../../../test-vectors/proxy.json")).unwrap();
        all["download_errors"].clone()
    }

    fn table(v: &serde_json::Value) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = v
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, t)| (k.clone(), t.as_str().unwrap().to_owned()))
            .collect();
        out.sort();
        out
    }

    fn ours(t: &[(&str, &str)]) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = t
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        out.sort();
        out
    }

    #[test]
    fn the_sentences_and_reasons_are_the_linux_clients() {
        let v = vectors();
        assert_eq!(ours(SENTENCES), table(&v["sentences"]));
        assert_eq!(ours(REASONS), table(&v["reasons"]));
    }

    #[test]
    fn a_servers_message_is_cleaned_as_on_linux() {
        for c in vectors()["messages"].as_array().unwrap() {
            assert_eq!(
                clean_message(c["input"].as_str().unwrap()).as_deref(),
                c["output"].as_str(),
                "{c}"
            );
        }
    }

    #[test]
    fn every_sentence_says_what_happened_and_what_to_do() {
        let gone = sentence(&Failure::Refused(404, Some("anything".into())));
        assert!(gone.contains("request a new one"), "{gone}");
        assert_eq!(
            sentence(&Failure::Refused(
                403,
                Some("the download cannot be prefetched".into())
            )),
            "The server refused the download: the download cannot be prefetched. Try again later."
        );
        assert_eq!(
            sentence(&Failure::Refused(500, None)),
            "The download failed (HTTP 500). Try again later."
        );
        assert!(sentence(&Failure::Cut).ends_with("Try again."));
        assert_eq!(
            sentence(&Failure::Disk(Disk::NoSpace)),
            "The file could not be saved: there is no space left on the disk. Check the folder, then try again."
        );
        for (key, _) in SENTENCES {
            assert!(!text(key).is_empty(), "{key}");
        }
    }
}
