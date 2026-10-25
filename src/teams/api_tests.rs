//! The Teams API as a client sees it: JSON lines in, JSON lines out, through `Service::route`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::*;

struct Harness {
    root: PathBuf,
    service: Arc<Service>,
    owner: String,
    lead: String,
    dev: String,
    qa: String,
}

fn root(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "moo-teams-api-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn call(service: &Service, token: &str, method: &str, params: Value) -> Value {
    let mut request = json!({"id": "t", "method": method, "params": params});
    if !token.is_empty() {
        request["auth"] = json!({"bearer": token});
    }
    service
        .route(&request.to_string())
        .expect("a teams response")
}

fn ok(response: Value) -> Value {
    assert!(response.get("error").is_none(), "{response}");
    response["result"].clone()
}

fn err(response: Value) -> Value {
    assert!(
        response.get("result").is_none(),
        "expected an error: {response}"
    );
    response["error"].clone()
}

fn revision(service: &Service, token: &str) -> u64 {
    ok(call(service, token, "teams.snapshot.get.v1", json!({})))["revision"]
        .as_u64()
        .unwrap()
}

fn mutate(service: &Service, token: &str, key: &str, command: Value) -> Value {
    let rev = revision(service, token);
    call(
        service,
        token,
        "teams.mutate.v1",
        json!({"expected_revision": rev, "idempotency_key": key, "command": command}),
    )
}

/// Launches team "alpha" (lead, dev, qa) through the draft API and returns the three credentials.
fn launch_alpha(service: &Service, owner: &str) -> (String, String, String) {
    let draft =
        json!({"name": "Alpha", "project_id": "", "lead": "lead", "developer": "dev", "qa": "qa"});
    let rev = revision(service, owner);
    let launched = ok(call(
        service,
        owner,
        "teams.drafts.launch.v1",
        json!({"expected_revision": rev, "idempotency_key": "launch-alpha", "draft": draft}),
    ));
    let token = |role: &str| {
        read_credential(&PathBuf::from(
            launched["credential_files"][role].as_str().unwrap(),
        ))
        .unwrap()
    };
    (token("lead"), token("developer"), token("qa"))
}

/// An initialized session with one launched team.
fn harness(label: &str) -> Harness {
    let root = root(label);
    let owner = read_credential(&initialize(&root).unwrap()).unwrap();
    let service = Arc::new(Service::new(root.clone()));
    let (lead, dev, qa) = launch_alpha(&service, &owner);
    Harness {
        root,
        service,
        owner,
        lead,
        dev,
        qa,
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn chat(h: &Harness, who: &str, id: &str, audience: Value) -> Value {
    mutate(
        &h.service,
        who,
        &format!("k-{id}"),
        json!({"kind": "append_chat", "team_id": "alpha", "message_id": id, "body": format!("body of {id}"), "audience": audience}),
    )
}

fn ids(page: &Value) -> Vec<String> {
    page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn capabilities_advertise_the_whole_surface_and_stay_open_without_a_credential() {
    let service = Service::new(root("caps"));
    let caps = ok(call(&service, "", "teams.capabilities.v1", json!({})));
    assert_eq!(
        (caps["api_version"].as_u64(), caps["api_minor"].as_u64()),
        (Some(1), Some(3))
    );
    let methods: Vec<&str> = caps["methods"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    for expected in [
        "teams.mutate.v1",
        "teams.events.wait.v1",
        "teams.messages.list.v1",
        "teams.drafts.launch.v1",
        "teams.schema.v1",
    ] {
        assert!(methods.contains(&expected), "{expected}");
    }
    assert!(caps["operations"].as_array().unwrap().len() >= 61);
    assert!(caps["errors"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["code"] == "rate_limited" && e["retryable"] == true));
    assert_eq!(
        caps["adapters"]["network_gateway"], false,
        "no network listener is offered"
    );
    assert_eq!(caps["limits"]["event_waits"]["max_open_per_actor"], 4);
    let schema = ok(call(&service, "", "teams.schema.v1", json!({})));
    assert!(schema["schema"]["params"]["teams.mutate.v1"].is_object());
    assert!(!service.root.exists(), "discovery creates nothing on disk");
}

#[test]
fn an_unknown_method_and_an_unauthenticated_call_are_typed_errors() {
    let h = harness("typed");
    assert_eq!(
        err(call(&h.service, &h.owner, "teams.nope.v1", json!({})))["code"],
        "unknown_method"
    );
    assert_eq!(
        err(call(&h.service, "", "teams.list.v1", json!({})))["code"],
        "unauthorized"
    );
    let bad = "f".repeat(64);
    let error = err(call(&h.service, &bad, "teams.list.v1", json!({})));
    assert_eq!(
        (error["code"].as_str(), error["retryable"].as_bool()),
        (Some("unauthorized"), Some(false))
    );
    assert_eq!(
        err(call(
            &h.service,
            &h.owner,
            "teams.get.v1",
            json!({"team_id": "x", "extra": 1})
        ))["code"],
        "invalid_params"
    );
}

#[test]
fn clients_written_for_api_1_0_keep_working() {
    // Frozen response shapes: what a 1.0 client reads must still be there.
    let shapes: Value = serde_json::from_str(include_str!(
        "../../tests/fixtures/teams-api-v1/baseline-shapes.json"
    ))
    .unwrap();
    let h = harness("compat");
    chat(&h, &h.lead, "m1", json!({"to": "team"}));
    let keys = |value: &Value, expected: &str| {
        for key in shapes[expected].as_array().unwrap() {
            assert!(
                value.get(key.as_str().unwrap()).is_some(),
                "{expected} lost `{key}`: {value}"
            );
        }
    };
    let stream = ok(call(
        &h.service,
        &h.lead,
        "teams.snapshot.get.v1",
        json!({}),
    ))["stream_id"]
        .as_str()
        .unwrap()
        .to_owned();
    keys(
        &ok(call(&h.service, "", "teams.capabilities.v1", json!({}))),
        "teams.capabilities.v1",
    );
    keys(
        &ok(call(
            &h.service,
            &h.lead,
            "teams.snapshot.get.v1",
            json!({}),
        )),
        "teams.snapshot.get.v1",
    );
    keys(
        &ok(call(
            &h.service,
            &h.lead,
            "teams.get.v1",
            json!({"team_id": "alpha"}),
        )),
        "teams.get.v1",
    );
    let list = ok(call(&h.service, &h.lead, "teams.list.v1", json!({})));
    keys(&list, "teams.list.v1");
    keys(&list["teams"][0], "team_in_list");
    keys(
        &ok(call(
            &h.service,
            &h.lead,
            "teams.conflicts.list.v1",
            json!({}),
        )),
        "teams.conflicts.list.v1",
    );
    keys(
        &ok(call(
            &h.service,
            &h.lead,
            "teams.integrations.list.v1",
            json!({}),
        )),
        "teams.integrations.list.v1",
    );
    for method in ["teams.events.list.v1", "teams.events.resume.v1"] {
        let page = ok(call(
            &h.service,
            &h.lead,
            method,
            json!({"stream_id": stream, "after": 0, "limit": 100}),
        ));
        keys(&page, method);
        keys(&page["events"][0], "event");
    }
    let rev = revision(&h.service, &h.lead);
    let mutation = call(
        &h.service,
        &h.lead,
        "teams.mutate.v1",
        json!({"expected_revision": rev, "idempotency_key": "x1",
        "command": {"kind": "append_chat", "team_id": "alpha", "message_id": "m9", "body": "x"}}),
    );
    keys(&ok(mutation), "teams.mutate.v1");
    let rev = revision(&h.service, &h.lead);
    let dry = call(
        &h.service,
        &h.lead,
        "teams.validate.v1",
        json!({"expected_revision": rev, "idempotency_key": "x2",
        "command": {"kind": "append_chat", "team_id": "alpha", "message_id": "m10", "body": "x"}}),
    );
    keys(&ok(dry), "teams.validate.v1");
    keys(
        &err(call(&h.service, &h.lead, "teams.nope.v1", json!({}))),
        "error",
    );
}

#[test]
fn the_frozen_1_0_request_set_still_replays_against_this_server() {
    let root = root("replay");
    let owner = read_credential(&initialize(&root).unwrap()).unwrap();
    let service = Service::new(root.clone());
    let dir = root.join("keys");
    std::fs::create_dir_all(&dir).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let mut tokens = std::collections::BTreeMap::new();
    let mut digests = std::collections::BTreeMap::new();
    for name in ["lead", "dev", "qa"] {
        let path = dir.join(format!("{name}.key"));
        digests.insert(name, create_credential(&path).unwrap());
        tokens.insert(name, read_credential(&path).unwrap());
    }
    let requests: Vec<Value> = serde_json::from_str(include_str!(
        "../../tests/fixtures/teams-api-v1/requests-1.0.json"
    ))
    .unwrap();
    let mut stream = String::new();
    for request in requests {
        let mut text = request
            .to_string()
            .replace("$DIGEST_LEAD", &digests["lead"])
            .replace("$DIGEST_DEV", &digests["dev"])
            .replace("$DIGEST_QA", &digests["qa"])
            .replace("$STREAM", &stream);
        text = text.replace("\\u0024", "$");
        let request: Value = serde_json::from_str(&text).unwrap();
        let who = request["as"]
            .as_str()
            .map_or(owner.clone(), |name| tokens[name].clone());
        let mut params = request["params"].clone();
        if request["method"] == "teams.mutate.v1" || request["method"] == "teams.validate.v1" {
            params["expected_revision"] = json!(revision(&service, &who));
        }
        let response = call(&service, &who, request["method"].as_str().unwrap(), params);
        assert!(
            response.get("error").is_none(),
            "{}: {response}",
            request["name"]
        );
        if request["name"] == "snapshot" {
            stream = response["result"]["stream_id"].as_str().unwrap().to_owned();
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn reading_chat_respects_every_private_boundary_and_pages_without_gaps() {
    let h = harness("chat");
    chat(&h, &h.lead, "a-open", json!({"to": "team"}));
    chat(
        &h,
        &h.lead,
        "b-dev-role",
        json!({"to": "role", "role": "developer"}),
    );
    chat(
        &h,
        &h.dev,
        "c-dev-qa",
        json!({"to": "actor", "actor_id": "qa"}),
    );
    chat(
        &h,
        &h.qa,
        "d-qa-lead",
        json!({"to": "actor", "actor_id": "lead"}),
    );
    let list =
        |who: &str, params: Value| ok(call(&h.service, who, "teams.messages.list.v1", params));
    let seen = |who: &str| ids(&list(who, json!({})));
    // Each party reads exactly what is theirs, in the order it happened.
    assert_eq!(seen(&h.lead), ["a-open", "b-dev-role", "d-qa-lead"]);
    assert_eq!(seen(&h.dev), ["a-open", "b-dev-role", "c-dev-qa"]);
    assert_eq!(seen(&h.qa), ["a-open", "c-dev-qa", "d-qa-lead"]);
    assert_eq!(
        seen(&h.owner),
        ["a-open", "b-dev-role", "c-dev-qa", "d-qa-lead"],
        "the Owner reads every channel"
    );
    // Paging one at a time visits every visible message once, in order.
    let mut visited = Vec::new();
    let mut cursor = Value::Null;
    loop {
        let page = list(&h.owner, json!({"limit": 1, "cursor": cursor}));
        visited.extend(ids(&page));
        if !page["has_more"].as_bool().unwrap() {
            break;
        }
        cursor = page["next_cursor"].clone();
    }
    assert_eq!(visited, seen(&h.owner));
    // Bad cursors and limits are typed errors.
    assert_eq!(
        err(call(
            &h.service,
            &h.owner,
            "teams.messages.list.v1",
            json!({"cursor": "nonsense"})
        ))["code"],
        "invalid_cursor"
    );
    assert_eq!(
        err(call(
            &h.service,
            &h.owner,
            "teams.messages.list.v1",
            json!({"limit": 1000})
        ))["code"],
        "invalid_params"
    );
}

#[test]
fn a_second_team_cannot_read_the_first_teams_channel_or_lists() {
    let h = harness("teams");
    let rev = revision(&h.service, &h.owner);
    let draft = json!({"name": "Beta", "project_id": "", "lead": "b-lead", "developer": "b-dev", "qa": "b-qa"});
    let launched = ok(call(
        &h.service,
        &h.owner,
        "teams.drafts.launch.v1",
        json!({"expected_revision": rev, "idempotency_key": "launch-beta", "draft": draft}),
    ));
    let other = read_credential(&PathBuf::from(
        launched["credential_files"]["lead"].as_str().unwrap(),
    ))
    .unwrap();
    chat(&h, &h.lead, "alpha-only", json!({"to": "team"}));
    let theirs = ok(call(
        &h.service,
        &other,
        "teams.messages.list.v1",
        json!({}),
    ));
    assert!(ids(&theirs).is_empty());
    assert_eq!(
        err(call(
            &h.service,
            &other,
            "teams.messages.list.v1",
            json!({"team_id": "alpha"})
        ))["code"],
        "forbidden"
    );
    assert_eq!(
        err(call(
            &h.service,
            &other,
            "teams.get.v1",
            json!({"team_id": "alpha"})
        ))["code"],
        "forbidden"
    );
    let listed = ok(call(&h.service, &other, "teams.list.v1", json!({})));
    assert_eq!(
        listed["teams"].as_array().unwrap().len(),
        1,
        "only their own team is listed"
    );
    assert_eq!(
        err(call(
            &h.service,
            &h.owner,
            "teams.get.v1",
            json!({"team_id": "nowhere"})
        ))["code"],
        "not_found"
    );
}

#[test]
fn lists_filter_by_status_and_task() {
    let h = harness("lists");
    let approval = |id: &str| {
        mutate(
            &h.service,
            &h.lead,
            &format!("k-{id}"),
            json!({"kind": "request_approval", "approval_id": id, "team_id": "alpha",
            "action": {"kind": "release", "target": "v1", "parameters": null}, "justification": "j", "ttl_seconds": 600}),
        )
    };
    ok(approval("ap-1"));
    ok(approval("ap-2"));
    ok(mutate(
        &h.service,
        &h.owner,
        "k-d",
        json!({"kind": "decide_approval", "approval_id": "ap-1", "approve": false, "rationale": "no"}),
    ));
    let pending = ok(call(
        &h.service,
        &h.lead,
        "teams.approvals.list.v1",
        json!({"status": "pending"}),
    ));
    assert_eq!(ids(&pending), ["ap-2"]);
    let all = ok(call(
        &h.service,
        &h.lead,
        "teams.approvals.list.v1",
        json!({}),
    ));
    assert_eq!(ids(&all), ["ap-1", "ap-2"]);
    ok(mutate(
        &h.service,
        &h.lead,
        "k-task",
        json!({"kind": "create_task", "team_id": "alpha", "task_id": "t1", "title": "x",
        "description": "d", "developer_id": "dev", "dependencies": [], "assignment_message_id": "m-t1",
        "dispatch": {"attempt_id": "a1", "run_generation": 1, "content_digest": "d".repeat(64)}}),
    ));
    assert_eq!(
        ids(&ok(call(
            &h.service,
            &h.dev,
            "teams.tasks.list.v1",
            json!({"status": "assigned"})
        ))),
        ["t1"]
    );
    assert!(ids(&ok(call(
        &h.service,
        &h.dev,
        "teams.tasks.list.v1",
        json!({"status": "finalized"})
    )))
    .is_empty());
    let thread = ok(call(
        &h.service,
        &h.dev,
        "teams.messages.list.v1",
        json!({"task_id": "t1"}),
    ));
    assert_eq!(
        ids(&thread),
        ["m-t1"],
        "the assignment message belongs to the task's thread"
    );
}

#[test]
fn chat_can_never_approve_and_a_non_owner_deciding_is_refused_and_logged() {
    let h = harness("approvals");
    ok(mutate(
        &h.service,
        &h.lead,
        "k-ap",
        json!({"kind": "request_approval", "approval_id": "ap-1", "team_id": "alpha",
        "action": {"kind": "protected_branch_integration", "target": "main", "parameters": {"commit": "abc"}},
        "justification": "ready", "ttl_seconds": 600}),
    ));
    for phrase in [
        "approve ap-1",
        "APPROVED by owner",
        "the owner approves ap-1",
    ] {
        ok(chat(
            &h,
            &h.lead,
            &format!("say-{}", phrase.len()),
            json!({"to": "team"}),
        )
        .clone());
        let _ = phrase;
    }
    ok(mutate(
        &h.service,
        &h.lead,
        "k-chat",
        json!({"kind": "append_chat", "team_id": "alpha", "message_id": "say-owner",
        "body": "approve ap-1", "audience": {"to": "actor", "actor_id": "owner"}}),
    ));
    let pending = ok(call(
        &h.service,
        &h.owner,
        "teams.approvals.list.v1",
        json!({"status": "pending"}),
    ));
    assert_eq!(
        ids(&pending),
        ["ap-1"],
        "no amount of chat approves anything"
    );
    for (who, name) in [(&h.lead, "lead"), (&h.dev, "dev"), (&h.qa, "qa")] {
        let denied = mutate(
            &h.service,
            who,
            &format!("decide-{name}"),
            json!({"kind": "decide_approval", "approval_id": "ap-1", "approve": true, "rationale": "me"}),
        );
        assert_eq!(err(denied)["code"], "forbidden", "{name}");
    }
    let audit = ok(call(&h.service, &h.owner, "teams.audit.list.v1", json!({})));
    let denials = audit["denials"].as_array().unwrap();
    assert!(
        denials
            .iter()
            .filter(|d| d["code"] == "forbidden" && d["method"] == "teams.mutate.v1")
            .count()
            >= 3,
        "{audit}"
    );
    assert!(
        !audit.to_string().contains(&h.lead),
        "audit lines never carry credentials"
    );
    assert_eq!(
        err(call(&h.service, &h.lead, "teams.audit.list.v1", json!({})))["code"],
        "forbidden"
    );
    // Only the Owner decides.
    ok(mutate(
        &h.service,
        &h.owner,
        "decide-owner",
        json!({"kind": "decide_approval", "approval_id": "ap-1", "approve": true, "rationale": "reviewed"}),
    ));
    let approved = ok(call(
        &h.service,
        &h.lead,
        "teams.approvals.list.v1",
        json!({"status": "approved"}),
    ));
    assert_eq!(ids(&approved), ["ap-1"]);
}

#[test]
fn a_waiting_reader_wakes_for_events_it_may_read_and_sleeps_through_others() {
    let h = harness("wait");
    let snapshot = ok(call(&h.service, &h.dev, "teams.snapshot.get.v1", json!({})));
    let (stream, watermark) = (
        snapshot["stream_id"].as_str().unwrap().to_owned(),
        snapshot["last_event_sequence"].as_u64().unwrap(),
    );
    let (service, token) = (Arc::clone(&h.service), h.dev.clone());
    let waiter = std::thread::spawn(move || {
        call(
            &service,
            &token,
            "teams.events.wait.v1",
            json!({"stream_id": stream, "after": watermark, "timeout_ms": 20_000}),
        )
    });
    std::thread::sleep(Duration::from_millis(200));
    // A direct message between two others moves the watermark but is not the developer's to read.
    chat(
        &h,
        &h.lead,
        "secret",
        json!({"to": "actor", "actor_id": "qa"}),
    );
    std::thread::sleep(Duration::from_millis(200));
    assert!(
        !waiter.is_finished(),
        "the waiter must not wake for an event it cannot read"
    );
    let start = Instant::now();
    chat(&h, &h.lead, "open", json!({"to": "team"}));
    let result = ok(waiter.join().unwrap());
    assert!(start.elapsed() < Duration::from_secs(5), "woke promptly");
    let payloads: Vec<String> = result["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["payload"]["message_id"].as_str().map(str::to_owned))
        .collect();
    assert_eq!(payloads, ["open"], "{result}");
}

#[test]
fn waits_time_out_with_an_empty_page_and_polling_returns_at_once() {
    let h = harness("timeout");
    let snapshot = ok(call(
        &h.service,
        &h.lead,
        "teams.snapshot.get.v1",
        json!({}),
    ));
    let (stream, watermark) = (
        snapshot["stream_id"].as_str().unwrap(),
        snapshot["last_event_sequence"].as_u64().unwrap(),
    );
    let start = Instant::now();
    let page = ok(call(
        &h.service,
        &h.lead,
        "teams.events.wait.v1",
        json!({"stream_id": stream, "after": watermark, "timeout_ms": 250}),
    ));
    assert!(
        start.elapsed() >= Duration::from_millis(200) && start.elapsed() < Duration::from_secs(5)
    );
    assert_eq!(page["events"], json!([]));
    assert_eq!(
        page["next_after"], watermark,
        "an empty wait leaves the cursor where it was"
    );
    let start = Instant::now();
    ok(call(
        &h.service,
        &h.lead,
        "teams.events.wait.v1",
        json!({"stream_id": stream, "after": watermark, "timeout_ms": 0}),
    ));
    assert!(
        start.elapsed() < Duration::from_millis(500),
        "timeout_ms 0 polls"
    );
}

#[test]
fn open_waits_are_bounded_and_a_refused_wait_says_when_to_retry() {
    let h = harness("bounded");
    let snapshot = ok(call(&h.service, &h.dev, "teams.snapshot.get.v1", json!({})));
    let (stream, watermark) = (
        snapshot["stream_id"].as_str().unwrap().to_owned(),
        snapshot["last_event_sequence"].as_u64().unwrap(),
    );
    let mut waiters = Vec::new();
    for _ in 0..4 {
        let (service, token, stream) = (Arc::clone(&h.service), h.dev.clone(), stream.clone());
        waiters.push(std::thread::spawn(move || {
            call(
                &service,
                &token,
                "teams.events.wait.v1",
                json!({"stream_id": stream, "after": watermark, "timeout_ms": 20_000}),
            )
        }));
    }
    std::thread::sleep(Duration::from_millis(400));
    let refused = err(call(
        &h.service,
        &h.dev,
        "teams.events.wait.v1",
        json!({"stream_id": stream, "after": watermark, "timeout_ms": 20_000}),
    ));
    assert_eq!(
        (refused["code"].as_str(), refused["retryable"].as_bool()),
        (Some("too_many_subscriptions"), Some(true))
    );
    assert!(refused["retry_after_ms"].as_u64().unwrap() > 0);
    // Another actor is not starved by this one, and everyone is released by the next event.
    let lead_wait = ok(call(
        &h.service,
        &h.lead,
        "teams.events.wait.v1",
        json!({"stream_id": stream, "after": watermark, "timeout_ms": 0}),
    ));
    assert_eq!(lead_wait["events"], json!([]));
    chat(&h, &h.lead, "release", json!({"to": "team"}));
    for waiter in waiters {
        assert!(!ok(waiter.join().unwrap())["events"]
            .as_array()
            .unwrap()
            .is_empty());
    }
    // The slots are free again.
    ok(call(
        &h.service,
        &h.dev,
        "teams.events.wait.v1",
        json!({"stream_id": stream, "after": watermark, "timeout_ms": 0}),
    ));
}

#[test]
fn a_slow_reader_sees_every_event_once_and_in_order_while_writers_keep_going() {
    let h = harness("slow");
    let snapshot = ok(call(
        &h.service,
        &h.owner,
        "teams.snapshot.get.v1",
        json!({}),
    ));
    let (stream, start) = (
        snapshot["stream_id"].as_str().unwrap().to_owned(),
        snapshot["last_event_sequence"].as_u64().unwrap(),
    );
    let (service, lead) = (Arc::clone(&h.service), h.lead.clone());
    let writer = std::thread::spawn(move || {
        for i in 0..40 {
            let rev = revision(&service, &lead);
            let done = call(
                &service,
                &lead,
                "teams.mutate.v1",
                json!({"expected_revision": rev, "idempotency_key": format!("w{i}"),
                "command": {"kind": "append_chat", "team_id": "alpha", "message_id": format!("w{i}"), "body": "x"}}),
            );
            assert!(done.get("error").is_none(), "{done}");
        }
    });
    let mut seen = Vec::new();
    let mut after = start;
    let deadline = Instant::now() + Duration::from_secs(60);
    while seen.len() < 40 && Instant::now() < deadline {
        // A deliberately small page and a slow pace: the server never queues anything for us.
        let page = ok(call(
            &h.service,
            &h.owner,
            "teams.events.wait.v1",
            json!({"stream_id": stream, "after": after, "limit": 7, "timeout_ms": 500}),
        ));
        for event in page["events"].as_array().unwrap() {
            seen.push(event["sequence"].as_u64().unwrap());
        }
        after = page["next_after"].as_u64().unwrap();
        std::thread::sleep(Duration::from_millis(15));
    }
    writer.join().unwrap();
    assert_eq!(
        seen,
        (start + 1..=start + 40).collect::<Vec<_>>(),
        "no gap, no repeat, in order"
    );
}

#[test]
fn requests_are_rate_limited_per_actor_with_a_usable_retry_hint() {
    let h = harness("rate");
    let mut limited = 0;
    let mut hint = None;
    for i in 0..300 {
        let response = call(
            &h.service,
            &h.lead,
            "teams.mutate.v1",
            json!({"expected_revision": 0, "idempotency_key": format!("r{i}"),
            "command": {"kind": "append_chat", "team_id": "alpha", "message_id": format!("r{i}"), "body": "x"}}),
        );
        match response["error"]["code"].as_str() {
            Some("rate_limited") => {
                limited += 1;
                hint.get_or_insert(response["error"]["retry_after_ms"].as_u64().unwrap());
            }
            Some("revision_conflict") => {}
            other => panic!("unexpected {other:?}: {response}"),
        }
    }
    assert!(limited >= 150, "{limited} of 300 were limited");
    assert!(hint.unwrap() > 0);
    // Reads have their own allowance, and other actors are unaffected.
    ok(call(&h.service, &h.lead, "teams.list.v1", json!({})));
    let rev = revision(&h.service, &h.owner);
    ok(call(
        &h.service,
        &h.owner,
        "teams.validate.v1",
        json!({"expected_revision": rev, "idempotency_key": "o",
        "command": {"kind": "rename_team", "team_id": "alpha", "name": "Alpha"}}),
    ));
    // Waiting out the hint is enough.
    std::thread::sleep(Duration::from_millis(hint.unwrap() + 100));
    let response = call(
        &h.service,
        &h.lead,
        "teams.mutate.v1",
        json!({"expected_revision": 0, "idempotency_key": "later",
        "command": {"kind": "append_chat", "team_id": "alpha", "message_id": "later", "body": "x"}}),
    );
    assert_eq!(response["error"]["code"], "revision_conflict");
}

#[test]
fn guessing_credentials_is_throttled_and_audited_without_touching_real_users() {
    let h = harness("guess");
    let mut codes = Vec::new();
    for i in 0..60 {
        let guess = format!("{i:064x}");
        codes.push(
            err(call(&h.service, &guess, "teams.list.v1", json!({})))["code"]
                .as_str()
                .unwrap()
                .to_owned(),
        );
    }
    assert!(
        codes.iter().any(|c| c == "unauthorized") && codes.iter().any(|c| c == "rate_limited"),
        "{codes:?}"
    );
    ok(call(&h.service, &h.lead, "teams.list.v1", json!({})));
    let audit = ok(call(
        &h.service,
        &h.owner,
        "teams.audit.list.v1",
        json!({"limit": 200}),
    ));
    let text = audit.to_string();
    assert!(text.contains("unauthorized") && text.contains("rate_limited"));
    assert!(
        !text.contains(&format!("{:064x}", 3)),
        "guesses are not stored"
    );
}

#[test]
fn drafts_are_proposed_checked_and_launched_through_the_api() {
    let h = harness("drafts");
    let proposal = ok(call(
        &h.service,
        &h.owner,
        "teams.drafts.propose.v1",
        json!({"goal": "Alpha"}),
    ));
    assert_eq!(proposal["source"], "template");
    let draft = proposal["draft"].clone();
    assert_eq!(
        draft["lead"], "alpha-2-lead",
        "proposals never collide with existing agents"
    );
    let preview = ok(call(
        &h.service,
        &h.owner,
        "teams.drafts.validate.v1",
        json!({"draft": draft}),
    ));
    assert_eq!(
        (preview["valid"].as_bool(), preview["team_id"].as_str()),
        (Some(true), Some("alpha-2"))
    );
    let clash =
        json!({"name": "X", "project_id": "", "lead": "lead", "developer": "dev", "qa": "qa"});
    let checked = ok(call(
        &h.service,
        &h.owner,
        "teams.drafts.validate.v1",
        json!({"draft": clash}),
    ));
    assert_eq!(checked["valid"], false);
    assert!(checked["problems"].as_array().unwrap().len() >= 3);
    // Launch is Owner only, and a bad draft is refused with its problems.
    let rev = revision(&h.service, &h.owner);
    let params = |draft: &Value, key: &str| json!({"expected_revision": rev, "idempotency_key": key, "draft": draft});
    assert_eq!(
        err(call(
            &h.service,
            &h.lead,
            "teams.drafts.launch.v1",
            params(&draft, "l1")
        ))["code"],
        "forbidden"
    );
    let invalid = err(call(
        &h.service,
        &h.owner,
        "teams.drafts.launch.v1",
        params(&clash, "l2"),
    ));
    assert_eq!(invalid["code"], "draft_invalid");
    assert!(!invalid["details"]["problems"]
        .as_array()
        .unwrap()
        .is_empty());
    // Launch creates the team, private credentials, and is idempotent.
    let launched = ok(call(
        &h.service,
        &h.owner,
        "teams.drafts.launch.v1",
        params(&draft, "l3"),
    ));
    assert_eq!(launched["team_id"], "alpha-2");
    let lead_file = PathBuf::from(launched["credential_files"]["lead"].as_str().unwrap());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&lead_file).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(lead_file.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
    let new_lead = read_credential(&lead_file).unwrap();
    assert_eq!(
        ok(call(&h.service, &new_lead, "teams.list.v1", json!({})))["teams"][0]["id"],
        "alpha-2"
    );
    let again = ok(call(
        &h.service,
        &h.owner,
        "teams.drafts.launch.v1",
        params(&draft, "l3"),
    ));
    assert_eq!(again, launched, "the same key replays the same result");
    let teams = ok(call(&h.service, &h.owner, "teams.list.v1", json!({})));
    assert_eq!(
        teams["teams"].as_array().unwrap().len(),
        2,
        "and creates nothing twice"
    );
    let other = json!({"name": "Different", "project_id": "", "lead": "d-lead", "developer": "d-dev", "qa": "d-qa"});
    assert_eq!(
        err(call(
            &h.service,
            &h.owner,
            "teams.drafts.launch.v1",
            params(&other, "l3")
        ))["code"],
        "idempotency_conflict"
    );
}

#[test]
fn a_restart_keeps_the_stream_so_cursors_stay_valid_and_stale_ones_say_where_to_resume() {
    let root = root("restart");
    let owner = read_credential(&initialize(&root).unwrap()).unwrap();
    let first = Service::new(root.clone());
    let (lead, _dev, _qa) = launch_alpha(&first, &owner);
    let send = |service: &Service, id: &str| {
        ok(mutate(
            service,
            &lead,
            &format!("k-{id}"),
            json!({"kind": "append_chat", "team_id": "alpha", "message_id": id, "body": "x"}),
        ))
    };
    send(&first, "before");
    let snapshot = ok(call(&first, &lead, "teams.snapshot.get.v1", json!({})));
    let (stream, watermark) = (
        snapshot["stream_id"].as_str().unwrap().to_owned(),
        snapshot["last_event_sequence"].as_u64().unwrap(),
    );
    // The process exits and a new one starts on the same data.
    drop(first);
    let restarted = Service::new(root.clone());
    send(&restarted, "during");
    let after_restart = ok(call(&restarted, &lead, "teams.snapshot.get.v1", json!({})));
    assert_eq!(
        after_restart["stream_id"],
        stream.as_str(),
        "the stream is the store's, not the process's"
    );
    let page = ok(call(
        &restarted,
        &lead,
        "teams.events.list.v1",
        json!({"stream_id": stream, "after": watermark}),
    ));
    assert_eq!(page["events"][0]["sequence"].as_u64(), Some(watermark + 1));
    assert_eq!(page["events"][0]["payload"]["message_id"], "during");
    // A cursor from another stream, or from the future, is told where to resume.
    for (stream_id, after) in [("another-stream", 0u64), (stream.as_str(), 9_999_999)] {
        let error = err(call(
            &restarted,
            &lead,
            "teams.events.list.v1",
            json!({"stream_id": stream_id, "after": after}),
        ));
        assert_eq!(error["code"], "cursor_expired");
        assert_eq!(error["details"]["stream_id"], stream.as_str());
        assert!(error["details"]["watermark"].as_u64().unwrap() >= watermark);
        assert!(error["details"]["retained_from"].as_u64().unwrap() >= 1);
    }
    drop(restarted);
    std::fs::remove_dir_all(root).unwrap();
}

fn declare_provider(h: &Harness, id: &str, caps: &[&str], metering: &str) {
    ok(mutate(
        &h.service,
        &h.owner,
        &format!("prov-{id}"),
        json!({"kind": "declare_provider", "provider_id": id, "label": id, "capabilities": caps,
            "models": [{"id": "m1"}], "metering": metering, "hard_spend_limit": metering == "metered"}),
    ));
}

#[test]
fn a_reviewed_plan_launches_a_team_with_its_task_graph_and_lists_specialists_without_starting_them()
{
    let h = harness("plans");
    declare_provider(&h, "coder", &["code_edit", "shell", "review"], "estimated");
    ok(mutate(
        &h.service,
        &h.owner,
        "plan-1",
        json!({"kind": "propose_plan", "plan_id": "plan-1",
        "goal": "Add contained OAuth state validation tests and document them"}),
    ));
    let plan = ok(call(
        &h.service,
        &h.owner,
        "teams.snapshot.get.v1",
        json!({}),
    ));
    assert!(
        plan.get("plans").is_none(),
        "plans are read through their own record, not the shared snapshot"
    );
    // Review: drop the documentation task, then launch.
    ok(mutate(
        &h.service,
        &h.owner,
        "plan-edit",
        json!({"kind": "edit_plan", "plan_id": "plan-1",
        "edits": [{"op": "remove_task", "key": "docs"}, {"op": "rename_team", "name": "OAuth tests"}]}),
    ));
    let rev = revision(&h.service, &h.owner);
    let launch = |key: &str| {
        call(
            &h.service,
            &h.owner,
            "teams.plans.launch.v1",
            json!({"expected_revision": revision(&h.service, &h.owner), "idempotency_key": key, "plan_id": "plan-1"}),
        )
    };
    assert_eq!(
        err(call(
            &h.service,
            &h.lead,
            "teams.plans.launch.v1",
            json!({"expected_revision": rev, "idempotency_key": "x", "plan_id": "plan-1"})
        ))["code"],
        "forbidden"
    );
    let launched = ok(launch("launch-1"));
    let team_id = launched["team_id"].as_str().unwrap().to_owned();
    assert_eq!(team_id, "oauth-tests");
    assert_eq!(
        launched["task_ids"].as_object().unwrap().len(),
        2,
        "the removed task was not created"
    );
    assert_eq!(launched["pending_specialists"][0]["specialty"], "security");
    assert_eq!(
        launched["pending_specialists"][0]["request_after_task"],
        "oauth-tests-tests"
    );
    // The graph exists, with the dependency, and the roles have their providers.
    let lead = read_credential(&PathBuf::from(
        launched["credential_files"]["lead"].as_str().unwrap(),
    ))
    .unwrap();
    let tasks = ok(call(&h.service, &lead, "teams.tasks.list.v1", json!({})));
    assert_eq!(ids(&tasks), ["oauth-tests-implement", "oauth-tests-tests"]);
    assert_eq!(
        tasks["items"][1]["dependencies"],
        json!(["oauth-tests-implement"])
    );
    let snapshot = ok(call(&h.service, &lead, "teams.snapshot.get.v1", json!({})));
    assert_eq!(snapshot["teams"][0]["id"], team_id.as_str());
    // Nothing started on its own: no recruitment exists until the Lead asks for one.
    assert_eq!(snapshot["recruitments"], json!([]));
    // A launched plan cannot launch twice, and replaying the key gives the same answer.
    assert_eq!(err(launch("launch-2"))["code"], "invalid_state");
    assert_eq!(ok(launch("launch-1")), launched);
    assert_eq!(
        ok(call(&h.service, &h.owner, "teams.list.v1", json!({})))["teams"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn memory_and_summary_are_served_over_the_api_to_the_right_people() {
    let h = harness("memory-api");
    ok(mutate(
        &h.service,
        &h.lead,
        "t1",
        json!({"kind": "create_task", "team_id": "alpha", "task_id": "t1", "title": "Work", "description": "d",
        "developer_id": "dev", "dependencies": [], "assignment_message_id": "m-t1",
        "dispatch": {"attempt_id": "a1", "run_generation": 1, "content_digest": "d".repeat(64)}}),
    ));
    let pack = ok(call(
        &h.service,
        &h.dev,
        "teams.memory.retrieve.v1",
        json!({"task_id": "t1", "budget_bytes": 4096}),
    ));
    assert_eq!(pack["task_id"], "t1");
    assert_eq!(pack["context"]["task"][0]["id"], "t1");
    assert!(pack["used_bytes"].as_u64().unwrap() <= 4096);
    assert_eq!(
        err(call(
            &h.service,
            &h.owner,
            "teams.memory.retrieve.v1",
            json!({"task_id": "t1"})
        ))["code"],
        "invalid_params",
        "the Owner names the team"
    );
    assert_eq!(
        ok(call(
            &h.service,
            &h.owner,
            "teams.memory.retrieve.v1",
            json!({"team_id": "alpha", "task_id": "t1"})
        ))["team_id"],
        "alpha"
    );
    assert_eq!(
        err(call(
            &h.service,
            &h.dev,
            "teams.memory.retrieve.v1",
            json!({"team_id": "other"})
        ))["code"],
        "forbidden"
    );
    let summary = ok(call(
        &h.service,
        &h.owner,
        "teams.summary.v1",
        json!({"stuck_after_seconds": 1}),
    ));
    assert_eq!(summary["totals"]["teams"], 1);
    assert_eq!(summary["projects"][0]["teams"][0]["open_tasks"], 1);
    let mine = ok(call(&h.service, &h.dev, "teams.summary.v1", json!({})));
    assert_eq!(
        mine["totals"]["teams"], 1,
        "everyone gets the view of what they may see"
    );
    let caps = ok(call(&h.service, "", "teams.capabilities.v1", json!({})));
    assert_eq!(
        caps["limits"]["work_limits"]["max_active_tasks_per_team"],
        3
    );
    assert_eq!(caps["limits"]["work_limits"]["max_dispute_positions"], 3);
}
