#!/usr/bin/env python3
"""A minimal, independent client for the Moo Teams local API.

It shares no code with Moo: it speaks the documented protocol (one JSON object per line over the
local Moo API socket, one request and one response per connection) using only the standard library.

    teams_client.py --credential-file KEY list-teams
    teams_client.py --credential-file KEY read-chat --team alpha [--limit 20]
    teams_client.py --credential-file KEY send-message --team alpha "hello" [--to lead|dev|qa|AGENT_ID]
    teams_client.py --credential-file KEY watch-status [--once] [--timeout SECONDS]
    teams_client.py --credential-file OWNER_KEY submit-approval AP_ID --approve|--deny --reason "..."

The socket is found from --socket, then $MOO_SOCKET_PATH, then the default session location.
A credential file holds a 64-character token and must be private to you (mode 0600). Only the
Owner's credential can decide approvals: chat messages are never approvals, and the server
refuses everyone else with `forbidden`.

Exit status: 0 success, 1 the server refused the request (code printed), 2 usage or connection error.
"""

import argparse
import json
import os
import socket
import sys
import time
import uuid

ROLE_ALIASES = {"lead": "lead", "dev": "developer", "developer": "developer", "qa": "qa"}


class ApiError(Exception):
    def __init__(self, error):
        super().__init__(f"{error.get('code')}: {error.get('message')}")
        self.code = error.get("code")
        self.retry_after_ms = error.get("retry_after_ms")
        self.details = error.get("details") or {}


def default_socket():
    if os.environ.get("MOO_SOCKET_PATH"):
        return os.environ["MOO_SOCKET_PATH"]
    config = os.environ.get("XDG_CONFIG_HOME") or os.path.expanduser("~/.config")
    return os.path.join(config, "moo-dev", "moo.sock")


class Client:
    def __init__(self, socket_path, token):
        self.socket_path = socket_path
        self.token = token

    def call(self, method, params=None, timeout=35.0):
        request = {"id": uuid.uuid4().hex[:16], "method": method, "params": params or {}}
        if self.token:
            request["auth"] = {"bearer": self.token}
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as sock:
            sock.settimeout(timeout)
            sock.connect(self.socket_path)
            sock.sendall(json.dumps(request).encode() + b"\n")
            buffer = b""
            while not buffer.endswith(b"\n"):
                chunk = sock.recv(65536)
                if not chunk:
                    break
                buffer += chunk
        if not buffer.endswith(b"\n"):
            # The server went away mid-request (a restart, a stop): a connection problem, not a reply.
            raise ConnectionError("the connection closed before a full reply arrived")
        response = json.loads(buffer)
        if "error" in response:
            raise ApiError(response["error"])
        return response["result"]

    def call_retrying(self, method, params=None, attempts=5):
        """Honors the server's retry hint for rate limits and busy waits instead of hammering it."""
        for attempt in range(attempts):
            try:
                return self.call(method, params)
            except ApiError as error:
                if error.retry_after_ms is None or attempt == attempts - 1:
                    raise
                time.sleep(error.retry_after_ms / 1000.0)

    def mutate(self, command):
        """Applies one command. The idempotency key makes a retry after a lost reply safe."""
        key = uuid.uuid4().hex
        for _ in range(3):
            revision = self.call_retrying("teams.snapshot.get.v1")["revision"]
            try:
                return self.call_retrying(
                    "teams.mutate.v1",
                    {"expected_revision": revision, "idempotency_key": key, "command": command},
                )
            except ApiError as error:
                if error.code != "revision_conflict":
                    raise
        raise ApiError({"code": "revision_conflict", "message": "the store kept changing"})


def read_token(path):
    st = os.stat(path)
    if st.st_mode & 0o077:
        sys.exit(f"{path}: credential files must be private (chmod 600)")
    with open(path) as handle:
        return handle.read().strip()


def list_teams(client, _args):
    for team in client.call_retrying("teams.list.v1")["teams"]:
        print(f"{team['id']:<20} {team['status']['status']:<10} tasks={team['open_tasks']} "
              f"integrations={team['open_integrations']}  {team['name']}")


def read_chat(client, args):
    cursor = None
    printed = 0
    while True:
        params = {"team_id": args.team, "limit": min(args.limit, 100)}
        if cursor:
            params["cursor"] = cursor
        page = client.call_retrying("teams.messages.list.v1", params)
        for message in page["items"]:
            to = message.get("audience", {})
            label = "" if to.get("to") in (None, "team") else f" -> {to.get('role') or to.get('actor_id')}"
            print(f"{message['sender_id']}{label}: {message['body']}")
            printed += 1
        if not page["has_more"] or printed >= args.limit:
            return
        cursor = page["next_cursor"]


def send_message(client, args):
    audience = {"to": "team"}
    if args.to:
        if args.to in ROLE_ALIASES:
            audience = {"to": "role", "role": ROLE_ALIASES[args.to]}
        else:
            audience = {"to": "actor", "actor_id": args.to}
    result = client.mutate({
        "kind": "append_chat", "team_id": args.team, "message_id": "cli-" + uuid.uuid4().hex[:12],
        "body": args.text, "audience": audience,
    })
    print(f"sent (revision {result['revision']})")


def snapshot_summary(client):
    snap = client.call_retrying("teams.snapshot.get.v1")
    attention = snap["attention"]
    print(f"status: rev={snap['revision']} pending_approvals={len(attention['pending_approvals'])} "
          f"open_conflicts={len(attention['open_conflicts'])} merge_unknown={len(attention['merge_unknown'])}")
    return snap


def watch_status(client, args):
    """Prints events as they happen. Survives disconnects, restarts and expired cursors."""
    snap = snapshot_summary(client)
    stream, after = snap["stream_id"], snap["last_event_sequence"]
    deadline = time.time() + args.timeout if args.timeout else None
    while deadline is None or time.time() < deadline:
        try:
            page = client.call("teams.events.wait.v1",
                               {"stream_id": stream, "after": after, "limit": 50, "timeout_ms": 15000})
        except ApiError as error:
            if error.code == "cursor_expired":
                # Our cursor is older than the retained log (or from another stream): resync from a
                # snapshot, then resume from its watermark. Nothing after the watermark is lost.
                print("cursor expired; resyncing from a snapshot")
                snap = snapshot_summary(client)
                stream, after = snap["stream_id"], snap["last_event_sequence"]
                continue
            if error.retry_after_ms is not None:
                time.sleep(error.retry_after_ms / 1000.0)
                continue
            raise
        except (ConnectionError, FileNotFoundError, socket.timeout):
            time.sleep(1.0)  # the server restarted or the socket is not there yet: try again
            continue
        for event in page["events"]:
            print(f"#{event['sequence']} {event['kind']} team={event.get('team_id')} by={event['actor_id']}")
        after = page["next_after"]
        if page["events"]:
            snapshot_summary(client)
            if args.once:
                return


def submit_approval(client, args):
    result = client.mutate({
        "kind": "decide_approval", "approval_id": args.approval_id,
        "approve": args.approve, "rationale": args.reason,
    })
    print(f"{'approved' if args.approve else 'denied'} {args.approval_id} (revision {result['revision']})")


def main():
    # Events are printed as they happen, even when stdout is a pipe.
    sys.stdout.reconfigure(line_buffering=True)
    parser = argparse.ArgumentParser(description="Minimal Moo Teams API client")
    parser.add_argument("--socket", default=default_socket())
    parser.add_argument("--credential-file", required=True)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("list-teams")
    chat = commands.add_parser("read-chat")
    chat.add_argument("--team")
    chat.add_argument("--limit", type=int, default=20)
    send = commands.add_parser("send-message")
    send.add_argument("--team", required=True)
    send.add_argument("--to", help="lead, dev, qa or an agent id; omit for the whole team")
    send.add_argument("text")
    watch = commands.add_parser("watch-status")
    watch.add_argument("--once", action="store_true", help="exit after the first batch of events")
    watch.add_argument("--timeout", type=float, default=0, help="stop after this many seconds")
    approve = commands.add_parser("submit-approval")
    approve.add_argument("approval_id")
    group = approve.add_mutually_exclusive_group(required=True)
    group.add_argument("--approve", action="store_true")
    group.add_argument("--deny", action="store_true")
    approve.add_argument("--reason", required=True)
    args = parser.parse_args()
    client = Client(args.socket, read_token(args.credential_file))
    handler = {"list-teams": list_teams, "read-chat": read_chat, "send-message": send_message,
               "watch-status": watch_status, "submit-approval": submit_approval}[args.command]
    try:
        handler(client, args)
    except ApiError as error:
        print(f"refused: {error}", file=sys.stderr)
        return 1
    except (ConnectionError, FileNotFoundError) as error:
        print(f"cannot reach the Moo socket at {args.socket}: {error}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())
