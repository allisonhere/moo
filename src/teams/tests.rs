use super::*;

fn temporary_root(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "moo-teams-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

#[test]
fn discovery_and_unknown_methods_do_not_create_state_or_expose_secrets() {
    let root = temporary_root("discovery");
    let service = Service::new(root.clone());
    assert!(service
        .route(r#"{"id":"core","method":"ping","params":{}}"#)
        .is_none());
    let discovery = service
        .route(r#"{"id":"cap","method":"teams.capabilities.v1"}"#)
        .unwrap();
    assert_eq!(discovery["result"]["api_version"], 1);
    assert_eq!(discovery["result"]["adapters"]["automatic_dispatch"], false);
    let response = service
        .route(
            r#"{"id":"unknown","method":"teams.missing.v1","auth":{"bearer":"secret-sentinel"}}"#,
        )
        .unwrap();
    assert_eq!(response["error"]["code"], "unknown_method");
    assert!(!response.to_string().contains("secret-sentinel"));
    assert!(!root.exists());
}

#[test]
fn duplicate_envelope_fields_are_rejected_without_echoing_the_bearer() {
    let service = Service::new(temporary_root("duplicate"));
    let response = service.route(r#"{"id":"a","method":"teams.snapshot.get.v1","auth":{"bearer":"secret-sentinel","bearer":"other"},"params":{}}"#).unwrap();
    assert_eq!(response["error"]["code"], "invalid_request");
    assert!(!response.to_string().contains("secret-sentinel"));
}

#[cfg(unix)]
#[test]
fn unauthenticated_requests_never_open_or_create_store() {
    let root = temporary_root("unauthenticated");
    let service = Service::new(root.clone());
    let response = service
        .route(r#"{"id":"unauth","method":"teams.snapshot.get.v1","params":{}}"#)
        .unwrap();
    assert_eq!(response["error"]["code"], "unauthorized");
    assert!(!root.exists());
}

#[cfg(unix)]
#[test]
fn durable_mutation_retry_and_revision_conflicts_survive_restart() {
    let root = temporary_root("restart");
    let key_file = initialize(&root).unwrap();
    let key = read_credential(&key_file).unwrap();
    let service = Service::new(root.clone());
    let request = json!({"id":"create", "method":"teams.mutate.v1", "auth":{"bearer":key},
        "params":{"expected_revision":0,"idempotency_key":"create-project",
            "command":{"kind":"create_project","project_id":"project-one","name":"One"}}});
    let created = service.route(&request.to_string()).unwrap();
    assert_eq!(created["result"]["revision"], 1, "{created}");
    assert_eq!(created["result"]["watermark"], 1);
    drop(service);
    let service = Service::new(root.clone());
    assert_eq!(service.route(&request.to_string()).unwrap(), created);
    let mut conflict = request.clone();
    conflict["params"]["command"]["name"] = json!("Changed");
    assert_eq!(
        service.route(&conflict.to_string()).unwrap()["error"]["code"],
        "idempotency_conflict"
    );
    conflict["params"]["idempotency_key"] = json!("another-key");
    assert_eq!(
        service.route(&conflict.to_string()).unwrap()["error"]["code"],
        "revision_conflict"
    );
    let snapshot = service
        .route(
            &json!({"id":"read", "method":"teams.snapshot.get.v1",
        "auth":{"bearer":key},"params":{}})
            .to_string(),
        )
        .unwrap();
    assert_eq!(snapshot["result"]["revision"], 1);
    assert_eq!(snapshot["result"]["projects"][0]["name"], "One");
    assert!(!snapshot.to_string().contains(&key));
    assert!(!snapshot.to_string().contains(&credential_digest(&key)));
    let stream_id = snapshot["result"]["stream_id"].as_str().unwrap();
    let page = service
        .route(
            &json!({"id":"events","method":"teams.events.list.v1",
        "auth":{"bearer":key},"params":{"stream_id":stream_id,"after":0,"limit":100}})
            .to_string(),
        )
        .unwrap();
    assert_eq!(page["result"]["events"][0]["sequence"], 1, "{page}");
    drop(service);
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn preview_does_not_commit_and_loose_credential_permissions_are_rejected() {
    use std::os::unix::fs::PermissionsExt;
    let root = temporary_root("preview");
    let key_file = initialize(&root).unwrap();
    let key = read_credential(&key_file).unwrap();
    let service = Service::new(root.clone());
    let preview = service
        .route(
            &json!({"id":"preview","method":"teams.validate.v1",
        "auth":{"bearer":key},"params":{"expected_revision":0,"idempotency_key":"preview",
            "command":{"kind":"create_project","project_id":"project-one","name":"One"}}})
            .to_string(),
        )
        .unwrap();
    assert_eq!(preview["result"]["valid"], true, "{preview}");
    let snapshot = service
        .route(
            &json!({"id":"read","method":"teams.snapshot.get.v1",
        "auth":{"bearer":key},"params":{}})
            .to_string(),
        )
        .unwrap();
    assert_eq!(snapshot["result"]["revision"], 0);
    assert_eq!(snapshot["result"]["projects"], json!([]));
    std::fs::set_permissions(&key_file, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(read_credential(&key_file).is_err());
    drop(service);
    std::fs::remove_dir_all(root).unwrap();
}
