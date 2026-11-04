//! Addresses, invite links, external links and the dialogs that ask about them.
use super::*;

#[test]
fn external_links_cannot_launch_files_scripts_or_credentialed_targets() {
    assert!(external_allowed(&url(
        "https://example.invalid/page?q=value"
    )));
    for raw in [
        "file:///tmp/example",
        "javascript:alert(1)",
        "https://user:password@example.invalid/",
        "http://example.invalid/",
    ] {
        assert!(!external_allowed(&url(raw)));
    }
}

#[test]
fn an_address_keeps_its_port_and_drops_a_pasted_invite_link() {
    for valid in [
        "community.example",
        "https://community.example/",
        "https://community.example:8443",
    ] {
        assert!(address(valid).is_ok());
    }
    assert_eq!(
        address("https://community.example:8443/?invite=code")
            .unwrap()
            .as_str(),
        "https://community.example:8443/"
    );
    assert_eq!(
        address("http://community.example:8443/path?invite=code")
            .unwrap()
            .as_str(),
        "https://community.example:8443/"
    );
    assert_eq!(
        address("http://localhost:8080/path?x=1").unwrap().as_str(),
        "http://localhost:8080/"
    );
    assert_eq!(
        address("<https://community.example:8443/>")
            .unwrap()
            .as_str(),
        "https://community.example:8443/"
    );
    for invalid in [
        "",
        "https://user@community.example",
        "https://community.example\\other",
        "https://community.example\n",
    ] {
        assert!(address(invalid).is_err(), "{invalid}");
    }
}

#[test]
fn a_pasted_invite_opens_the_join_screen_and_a_bare_address_does_not() {
    assert_eq!(invite_code("community.example:8443"), None);
    assert_eq!(
        invite_code("https://community.example:8443/?invite=abc234xyz789").as_deref(),
        Some("abc234xyz789")
    );
    assert_eq!(
        invite_code("http://community.example:8443/invite/Team_Night-2026/").as_deref(),
        Some("Team_Night-2026")
    );
    assert_eq!(
        invite_code("https://community.example/#/join?invite=abc234xyz789").as_deref(),
        Some("abc234xyz789")
    );
    assert_eq!(
        invite_code("https://community.example:8443/?invite=abc234xyz789.").as_deref(),
        Some("abc234xyz789")
    );
    assert_eq!(
        invite_code("<https://community.example/?invite=abc234xyz789>").as_deref(),
        Some("abc234xyz789")
    );
    assert_eq!(invite_code("https://community.example/?invite=short"), None);
    assert_eq!(
        invite_code("https://user@community.example/?invite=abc234xyz789"),
        None
    );
    assert_eq!(
        join_page(Some("abc234xyz789")).path(),
        "/invite/abc234xyz789"
    );
    assert_eq!(join_page(None).path(), "/");
    assert_eq!(join_page(Some("../x")).path(), "/");
    assert_eq!(
        address("https://community.example:8443/invite/abc234xyz789")
            .unwrap()
            .as_str(),
        "https://community.example:8443/"
    );
}

#[test]
fn only_https_links_and_mail_addresses_leave_the_app() {
    for ok in [
        "https://example.org/x?y=1",
        "mailto:alice@example.org",
        "mailto:alice@example.org?subject=Hi%20there",
    ] {
        assert!(external_allowed(&url(ok)), "{ok}");
    }
    for refused in [
        "http://example.org/",
        "tel:+4930123456",
        "sms:+4930123456",
        "file:///etc/passwd",
        "javascript:alert(1)",
        "mnema://localhost/api/x",
        "https://user:pw@example.org/",
        "mailto:",
    ] {
        assert!(!external_allowed(&url(refused)), "{refused}");
    }
    let long = format!("mailto:{}@example.org", "a".repeat(2100));
    assert!(!external_allowed(&url(&long)));
}

#[test]
fn the_confirmation_shows_the_whole_mailto_including_hidden_recipients() {
    let text = confirm_text(&url(
        "mailto:alice@example.org?bcc=eve@example.org&body=hello",
    ));
    assert!(text.contains("bcc=eve@example.org"), "{text}");
    assert!(text.contains("body=hello"), "{text}");
}

#[test]
fn a_navigation_inside_the_app_is_allowed_and_a_foreign_one_is_not() {
    assert!(navigation_allowed(&app("/c/general")));
    assert!(navigation_allowed(&app("/api/media/x")));
    assert!(!navigation_allowed(&url("https://example.org/")));
    assert!(!navigation_allowed(&app("/other.html")));
}

#[test]
fn dialogs_pass_their_text_as_arguments_and_name_the_platforms_own_dependency() {
    let link = url("https://example.invalid/a?q=\"x\"&r='y'");
    let text = confirm_text(&link);
    for kind in [Dialog::Error, Dialog::Confirm, Dialog::Add] {
        let args = osascript_args(kind, &text);
        // The text is data for the script, never part of its source.
        assert_eq!(
            &args[args.len() - 2..],
            [text.clone(), APP_TITLE.to_owned()]
        );
        assert!(
            args[..args.len() - 2]
                .iter()
                .all(|a| !a.contains("example.invalid"))
        );
    }
    let message = startup_message();
    assert!(message.starts_with(APP_TITLE));
    assert_eq!(message.contains("WebView2"), cfg!(windows));
}

#[test]
fn an_invite_link_is_for_the_app_when_its_server_is_in_the_list_or_can_join_it() {
    let listed = [url("https://talk.example/")];
    let decide = |link: &str, room: bool| invite_decision(&url(link), &listed, room);
    let add = |code: &str| Link::Add { code: code.into() };
    // A server that is not in the list: offered, while there is room.
    assert_eq!(
        decide("https://other.example/invite/abc123xyz", true),
        add("abc123xyz")
    );
    assert_eq!(
        decide("https://other.example/invite/abc123xyz/", true),
        add("abc123xyz")
    );
    assert_eq!(
        decide("https://other.example:8443/invite/abc123xyz", true),
        add("abc123xyz")
    );
    assert_eq!(
        decide("https://other.example/invite/abc123xyz", false),
        Link::Browser
    );
    // A server that is: shown there, even with the list full. The port and the path do not matter.
    let known = Link::Known {
        origin: url("https://talk.example/"),
        code: "abc123xyz".into(),
    };
    assert_eq!(
        decide("https://talk.example/invite/abc123xyz", false),
        known
    );
    assert_eq!(
        decide("https://TALK.example/invite/abc123xyz?x=1#y", true),
        known
    );
    // A look-alike host is another server: offered under its own name, never taken for the listed one.
    assert_eq!(
        decide("https://talk.example.evil.test/invite/abc123xyz", true),
        add("abc123xyz")
    );
    assert_eq!(
        decide("https://talk-example.test/invite/abc123xyz", false),
        Link::Browser
    );
    // Not an invite, not a code, not https, with credentials: the browser's business.
    for link in [
        "https://other.example/",
        "https://other.example/invites/abc123xyz",
        "https://other.example/invite/abc",
        "https://other.example/invite/abc123xyz/more",
        "https://other.example/?invite=abc123xyz",
        "http://other.example/invite/abc123xyz",
        "https://user:pw@other.example/invite/abc123xyz",
        "https://user@talk.example/invite/abc123xyz",
    ] {
        assert_eq!(decide(link, true), Link::Browser, "{link}");
    }
}

#[test]
fn an_error_dialog_can_be_copied_and_a_question_cannot_lose_its_cancel() {
    let hostile = "a \"quote\" \\ back\nline & 'single' \u{2028} end run -e";
    let error = osascript_args(Dialog::Error, hostile);
    let script = error[..error.len() - 2].join("\n");
    assert!(script.contains("buttons {\"Copy\", \"OK\"}"));
    assert!(script.contains("set the clipboard to (item 1 of argv)"));
    assert_eq!(error[error.len() - 2], hostile);
    // Every script line is introduced by `-e`; nothing else is a flag.
    let lines = &error[..error.len() - 2];
    assert!(lines.chunks(2).all(|pair| pair[0] == "-e"));
    let confirm = osascript_args(Dialog::Confirm, hostile);
    let script = confirm.join("\n");
    assert!(script.contains("cancel button \"Cancel\""));
    assert!(!script.contains("clipboard"));
}
