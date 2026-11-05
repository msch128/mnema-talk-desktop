//! The commands registered at start match the capability files.

/// What the shipped client's capabilities grant is exactly what it
/// registers: a permission without a handler in this build is a grant
/// nobody reviewed, and a handler list that grows later must grow the
/// capability files on purpose. Webviews are named by label, never by window.
#[test]
fn every_granted_permission_has_a_handler_in_this_build_and_no_capability_names_a_window() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let source = include_str!("../run.rs");
    let start = source.find("tauri::generate_handler![").unwrap();
    let handlers = &source[start..start + source[start..].find("\n    ]").unwrap()];
    let registered: Vec<&str> = handlers
        .lines()
        .skip(1)
        .map(|l| l.trim().trim_end_matches(',').rsplit("::").next().unwrap())
        .filter(|n| !n.is_empty())
        .collect();
    assert!(registered.contains(&"desktop_open_instance"));
    let config: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(root.join("tauri.conf.json")).unwrap())
            .unwrap();
    let listed: Vec<&str> = config["app"]["security"]["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|c| c.as_str())
        .collect();
    for entry in std::fs::read_dir(root.join("capabilities")).unwrap() {
        let path = entry.unwrap().path();
        let json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let id = json["identifier"].as_str().unwrap();
        assert!(listed.contains(&id), "{id} is not in tauri.conf.json");
        assert!(json.get("windows").is_none(), "{id} names windows");
        for permission in json["permissions"].as_array().unwrap() {
            let command = permission
                .as_str()
                .unwrap()
                .strip_prefix("allow-")
                .unwrap()
                .replace('-', "_");
            assert!(
                registered.contains(&command.as_str()),
                "{id} grants {command}, which this build does not register"
            );
        }
    }
    assert_eq!(
        listed.len(),
        std::fs::read_dir(root.join("capabilities"))
            .unwrap()
            .count()
    );
}
