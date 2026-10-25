use std::io::{self, Read};
use std::path::{Path, PathBuf};

use clap::{Arg, Command};
use serde_json::{json, Value};

const MAX_REQUEST_BYTES: u64 = 1024 * 1024;
const DISCOVERY_METHOD: &str = "teams.capabilities.v1";

pub(super) fn command() -> Command {
    Command::new("teams")
        .about("Manage local Teams collaboration through versioned JSON requests")
        .subcommand_required(true)
        .subcommand(
            Command::new("ui")
                .about("Open the Teams screen: navigator, chat, tasks and decisions")
                .arg(
                    Arg::new("view")
                        .long("view")
                        .value_name("VIEW")
                        .value_parser(["terminals", "chat", "tasks", "decisions"])
                        .help("Open on this view"),
                )
                .arg(
                    Arg::new("start")
                        .long("start")
                        .value_name("WHAT")
                        .value_parser(["builder"])
                        .help("Start straight in the team builder"),
                )
                .arg(
                    Arg::new("credential-file")
                        .long("credential-file")
                        .value_name("PATH")
                        .help("Credential to act as; defaults to the session's owner credential")
                        .value_parser(clap::value_parser!(PathBuf)),
                ),
        )
        .subcommand(
            Command::new("tab")
                .about("Open the Teams screen in its own tab, or focus that tab if it is open")
                .arg(
                    Arg::new("view")
                        .long("view")
                        .value_name("VIEW")
                        .value_parser(["terminals", "chat", "tasks", "decisions"])
                        .help("Open on this view"),
                )
                .arg(
                    Arg::new("start")
                        .long("start")
                        .value_name("WHAT")
                        .value_parser(["builder"])
                        .help("Start straight in the team builder"),
                ),
        )
        .subcommand(
            Command::new("schema")
                .about("Print this build's machine-readable Teams API schema (no server needed)"),
        )
        .subcommand(Command::new("init").about("Create the session's private owner credential"))
        .subcommand(
            Command::new("keygen")
                .about("Create a private member credential and print its public digest")
                .arg(
                    Arg::new("path")
                        .required(true)
                        .value_parser(clap::value_parser!(PathBuf)),
                ),
        )
        .subcommand(
            Command::new("call")
                .about("Send a local Teams JSON request without placing a credential in argv")
                .arg(
                    Arg::new("request")
                        .required(true)
                        .value_parser(clap::value_parser!(PathBuf)),
                )
                .arg(
                    Arg::new("credential-file")
                        .long("credential-file")
                        .value_name("PATH")
                        .value_parser(clap::value_parser!(PathBuf)),
                ),
        )
}

pub(super) fn run(args: &[String]) -> io::Result<i32> {
    let matches = match command()
        .try_get_matches_from(std::iter::once("moo teams").chain(args.iter().map(String::as_str)))
    {
        Ok(matches) => matches,
        Err(error) => return super::print_clap_error(&error),
    };
    if super::target::is_remote() {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Teams commands currently support local sessions only",
        ));
    }
    match matches.subcommand() {
        Some(("schema", _)) => {
            let schema = serde_json::to_string_pretty(&crate::teams::schema_document())
                .map_err(io::Error::other)?;
            println!("{schema}");
            Ok(0)
        }
        Some(("init", _)) => {
            let path =
                crate::teams::initialize(&crate::session::data_dir()).map_err(io::Error::other)?;
            super::print_response(&json!({"result": {"owner_credential_file": path}}))
        }
        Some(("ui", args)) => {
            let credential = match args.get_one::<PathBuf>("credential-file") {
                Some(path) => path.clone(),
                // The first time the screen opens there is no store yet; create it.
                None => crate::teams::initialize(&crate::session::data_dir())
                    .map_err(io::Error::other)?,
            };
            let token = crate::teams::read_credential(&credential).map_err(io::Error::other)?;
            let client = super::target::api_client()?;
            let start = crate::teams_ui::Start {
                view: args.get_one::<String>("view").cloned(),
                builder: args.get_one::<String>("start").is_some(),
            };
            crate::teams_ui::run(
                client,
                token,
                crate::session::data_dir().join("teams-ui-state.json"),
                start,
            )?;
            Ok(0)
        }
        Some(("tab", args)) => open_in_tab(args),
        Some(("keygen", args)) => {
            let path = required_path(args, "path")?;
            let digest = crate::teams::create_credential(path).map_err(io::Error::other)?;
            super::print_response(&json!({"result": {
                "credential_file": path,
                "credential_digest": digest,
            }}))
        }
        Some(("call", args)) => {
            let path = required_path(args, "request")?;
            call(
                path,
                args.get_one::<PathBuf>("credential-file")
                    .map(PathBuf::as_path),
            )
        }
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "unknown Teams command",
        )),
    }
}

/// Runs `moo <args>` and returns its JSON `result`.
fn moo_json(args: &[&str]) -> io::Result<Value> {
    let output = std::process::Command::new(std::env::current_exe()?)
        .args(args)
        .output()?;
    let value: Value = serde_json::from_slice(&output.stdout).map_err(io::Error::other)?;
    match value.get("result") {
        Some(result) if output.status.success() => Ok(result.clone()),
        _ => Err(io::Error::other(format!(
            "moo {} failed: {}",
            args.join(" "),
            value
        ))),
    }
}

/// Focuses the Teams tab of the current workspace, or makes one that runs the Teams screen.
fn open_in_tab(args: &clap::ArgMatches) -> io::Result<i32> {
    let workspace = std::env::var("MOO_ACTIVE_WORKSPACE_ID").ok();
    let mut list = vec!["tab", "list"];
    if let Some(workspace) = workspace.as_deref() {
        list.extend(["--workspace", workspace]);
    }
    let existing = moo_json(&list)?;
    let open = existing["tabs"].as_array().and_then(|tabs| {
        tabs.iter().find_map(|tab| {
            (tab["label"] == crate::teams::TAB_LABEL)
                .then(|| tab["tab_id"].as_str().map(str::to_owned))
                .flatten()
        })
    });
    if let Some(tab_id) = open {
        moo_json(&["tab", "focus", &tab_id])?;
        return Ok(0);
    }
    let mut create = vec![
        "tab",
        "create",
        "--label",
        crate::teams::TAB_LABEL,
        "--focus",
    ];
    if let Some(workspace) = workspace.as_deref() {
        create.extend(["--workspace", workspace]);
    }
    let created = moo_json(&create)?;
    let pane = created["root_pane"]["pane_id"]
        .as_str()
        .ok_or_else(|| io::Error::other("the new tab has no pane"))?;
    // `exec` makes quitting the screen close the tab instead of leaving a shell behind.
    let exe = std::env::current_exe()?;
    let mut command = format!(
        "exec '{}' teams ui",
        exe.display().to_string().replace('\'', "'\\''")
    );
    if let Some(view) = args.get_one::<String>("view") {
        command.push_str(&format!(" --view {view}"));
    }
    if args.get_one::<String>("start").is_some() {
        command.push_str(" --start builder");
    }
    moo_json(&["pane", "run", pane, &command])?;
    Ok(0)
}

fn required_path<'a>(args: &'a clap::ArgMatches, name: &str) -> io::Result<&'a Path> {
    args.get_one::<PathBuf>(name)
        .map(PathBuf::as_path)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing required path"))
}

fn read_request(path: &Path) -> io::Result<Value> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(MAX_REQUEST_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_REQUEST_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Teams request file exceeds 1 MiB",
        ));
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid Teams request JSON"))
}

fn authorize_request(request: &mut Value, credential: Option<&str>) -> io::Result<String> {
    let object = request.as_object_mut().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "Teams request must be a JSON object",
        )
    })?;
    let method = object
        .get("method")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "Teams request requires a method",
            )
        })?
        .to_string();
    if !method.starts_with("teams.") || !method.ends_with(".v1") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "only teams.*.v1 methods are supported",
        ));
    }
    if object.contains_key("auth") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "request files must not contain credentials; use --credential-file",
        ));
    }
    if let Some(credential) = credential {
        object.insert("auth".into(), json!({"bearer": credential}));
    } else if method != DISCOVERY_METHOD {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "--credential-file is required for this Teams method",
        ));
    }
    Ok(method)
}

fn call(path: &Path, credential_path: Option<&Path>) -> io::Result<i32> {
    let mut request = read_request(path)?;
    let credential = credential_path
        .map(crate::teams::read_credential)
        .transpose()
        .map_err(io::Error::other)?;
    let method = authorize_request(&mut request, credential.as_deref())?;
    let client = super::target::api_client()?;
    if method != DISCOVERY_METHOD {
        let discovery = client
            .request_json_value(
                &json!({"id": "cli:teams:discover", "method": DISCOVERY_METHOD, "params": {}}),
            )
            .map_err(io::Error::other)?;
        if unsupported(&discovery) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "this server does not support the local Teams API",
            ));
        }
        if discovery.get("error").is_some() {
            return super::print_response(&discovery);
        }
    }
    let response = client
        .request_json_value(&request)
        .map_err(io::Error::other)?;
    if (method == DISCOVERY_METHOD && unsupported(&response))
        || matches!(
            response["error"]["code"].as_str(),
            Some("unknown_method" | "unsupported_method")
        )
    {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "this server does not support this Teams method",
        ));
    }
    super::print_response(&response)
}

fn unsupported(response: &Value) -> bool {
    matches!(
        response["error"]["code"].as_str(),
        Some("invalid_request" | "unknown_method" | "unsupported_method")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn teams_commands_parse_only_file_based_credentials() {
        let matches = command()
            .try_get_matches_from([
                "teams",
                "call",
                "request.json",
                "--credential-file",
                "owner.key",
            ])
            .unwrap();
        let (_, args) = matches.subcommand().unwrap();
        assert_eq!(
            args.get_one::<PathBuf>("credential-file"),
            Some(&PathBuf::from("owner.key"))
        );
        assert!(command()
            .try_get_matches_from(["teams", "call", "request.json", "--token", "secret"])
            .is_err());
    }

    #[test]
    fn teams_request_authentication_never_accepts_credentials_in_the_request_file() {
        let mut discovery = json!({"id": "1", "method": DISCOVERY_METHOD, "params": {}});
        assert_eq!(
            authorize_request(&mut discovery, None).unwrap(),
            DISCOVERY_METHOD
        );
        let mut mutation = json!({"id": "2", "method": "teams.mutate.v1", "params": {}});
        assert!(authorize_request(&mut mutation, None).is_err());
        authorize_request(&mut mutation, Some("private-key")).unwrap();
        assert_eq!(mutation["auth"]["bearer"], "private-key");
        assert!(authorize_request(&mut mutation, Some("replacement-key")).is_err());
        let mut unrelated = json!({"method": "server.stop"});
        assert!(authorize_request(&mut unrelated, Some("private-key")).is_err());
    }

    #[test]
    fn old_server_unknown_method_is_unsupported() {
        assert!(unsupported(&json!({"error": {"code": "invalid_request"}})));
        assert!(!unsupported(&json!({"error": {"code": "unauthorized"}})));
    }
}
