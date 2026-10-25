use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use serde_json::json;

use super::data::Snapshot;
use super::layout::Area;
use super::render::render;
use super::state::{Effect, Focus, Link, Scope, Screen, View};
use crate::app::state::Palette;

fn fixture(viewer_role: &str) -> Snapshot {
    serde_json::from_value(json!({
        "revision": 42, "stream_id": "s",
        "viewer": {"id": "owner", "role": viewer_role, "team_id": null},
        "projects": [{"id": "moo", "name": "Moo"}],
        "teams": [
            {"id": "alpha", "name": "Alpha", "project_id": "moo", "status": {"status": "active"}, "lead_id": "lead1", "developer_id": "dev1", "qa_id": "qa1"},
            {"id": "beta", "name": "Beta platform team with a deliberately very long display name that cannot fit", "project_id": "moo", "status": {"status": "suspended", "reason": "pause"}, "lead_id": "lead2", "developer_id": "dev2", "qa_id": "qa2"}
        ],
        "actors": [
            {"id": "lead1", "role": "lead", "team_id": "alpha", "run_generation": 1},
            {"id": "dev1", "role": "developer", "team_id": "alpha", "run_generation": 1},
            {"id": "qa1", "role": "qa", "team_id": "alpha", "run_generation": 1},
            {"id": "lead2", "role": "lead", "team_id": "beta", "run_generation": 1},
            {"id": "dev2", "role": "developer", "team_id": "beta", "run_generation": 2},
            {"id": "qa2", "role": "qa", "team_id": "beta", "run_generation": 1}
        ],
        "tasks": [
            {"id": "t1", "team_id": "alpha", "title": "Build the plugin settings", "description": "d", "developer_id": "dev1", "dependencies": [], "status": {"status": "in_progress", "acceptance_id": "a"}},
            {"id": "t2", "team_id": "alpha", "title": "Document settings", "description": "d", "developer_id": "dev1", "dependencies": ["t1"], "status": {"status": "assigned"}},
            {"id": "t3", "team_id": "beta", "title": "Rename the thing", "description": "d", "developer_id": "dev2", "dependencies": [], "status": {"status": "awaiting_qa", "handoff_id": "h1"}}
        ],
        "handoffs": [{"id": "h1", "task_id": "t3", "summary": "done, see tests", "evidence": [{"kind": "test", "reference": "cargo nextest run"}]}],
        "decisions": [{"id": "q1", "team_id": "alpha", "task_id": "t1", "actor_id": "qa1", "kind": {"kind": "qa_review", "handoff_id": "h0", "verdict": "accept"}, "body": "looks right"}],
        "messages": [
            {"id": "m1", "team_id": "alpha", "sender_id": "lead1", "task_id": null, "kind": {"kind": "text"}, "body": "@dev1 please start on t1"},
            {"id": "m2", "team_id": "alpha", "sender_id": "dev1", "task_id": "t1", "kind": {"kind": "text"}, "body": "on it"},
            {"id": "m4", "team_id": "alpha", "sender_id": "lead1", "task_id": null, "kind": {"kind": "text"}, "audience": {"to": "role", "role": "developer"}, "body": "keep this between developers"},
            {"id": "m5", "team_id": "alpha", "sender_id": "qa1", "task_id": null, "kind": {"kind": "text"}, "audience": {"to": "actor", "actor_id": "lead1"}, "body": "a private word"},
            {"id": "m3", "team_id": "beta", "sender_id": "dev2", "task_id": "t3", "kind": {"kind": "handoff", "task_id": "t3", "handoff_id": "h1"}, "body": "done, see tests"}
        ],
        "approvals": [
            {"id": "ap-1", "team_id": "alpha", "requested_by": "lead1", "action": {"kind": "protected_branch_integration", "target": "main", "parameters": {"commit": "abc", "squash": true}}, "action_digest": "0123456789abcdef0123", "justification": "tests green", "alternatives": ["wait for the next release"], "risks": ["touches the public API"], "ttl_seconds": 3600, "status": {"status": "pending"}},
            {"id": "ap-0", "team_id": "alpha", "requested_by": "lead1", "action": {"kind": "release", "target": "v1", "parameters": null}, "action_digest": "ff", "justification": "old", "ttl_seconds": 60, "status": {"status": "used", "execution_id": "x", "consumed_by": "owner", "at": 1}}
        ],
        "recruitments": [], "integrations": [],
        "bindings": [{"actor_id": "dev1", "pane_id": "pane-7", "provider": "claude"}],
        "conflicts": [{"id": "c1", "kind": "dependency_impact", "team_ids": ["alpha", "beta"], "integration_ids": ["i1", "i2"], "detail": ["plugin-api"], "status": {"status": "open"}}],
        "notices": [],
        "attention": {"pending_approvals": ["ap-1"], "queued_recruitments": [], "open_conflicts": ["c1"], "open_notices": [], "merge_unknown": []}
    }))
    .unwrap()
}

fn screen(width: u16, height: u16, role: &str) -> Screen {
    let mut screen = Screen::new(Rect::new(0, 0, width, height));
    screen.apply_snapshot(fixture(role));
    screen
}

fn draw(screen: &Screen) -> Buffer {
    let area = Rect::new(0, 0, screen.area.width, screen.area.height);
    let mut buf = Buffer::empty(area);
    render(&mut buf, area, screen, &Palette::catppuccin());
    buf
}

fn rows(buf: &Buffer) -> Vec<String> {
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol().to_owned())
                .collect::<String>()
        })
        .collect()
}

fn text(buf: &Buffer) -> String {
    rows(buf).join("\n")
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn press(screen: &mut Screen, keys: &str) -> Vec<Effect> {
    let mut effects = Vec::new();
    for c in keys.chars() {
        effects.extend(screen.key(key(KeyCode::Char(c))));
    }
    effects
}

#[test]
fn every_target_size_draws_every_view_with_every_option_inside_its_area() {
    for (width, height) in [(160, 45), (100, 30), (80, 24), (60, 24), (40, 12), (20, 6)] {
        for view in View::ALL {
            for (compact, ascii, contrast) in [(true, false, false), (false, true, true)] {
                let mut s = screen(width, height, "owner");
                s.view = view;
                s.options.density = if compact {
                    super::theme::Density::Compact
                } else {
                    super::theme::Density::Comfortable
                };
                s.options.ascii = ascii;
                s.options.high_contrast = contrast;
                let buf = draw(&s);
                assert_eq!((buf.area.width, buf.area.height), (width, height));
                if ascii {
                    assert!(
                        text(&buf).is_ascii(),
                        "{width}x{height} {view:?} has non-ascii cells in ascii mode"
                    );
                }
            }
        }
    }
}

#[test]
fn wide_layout_shows_navigator_workspace_and_inspector() {
    for (width, height) in [(160, 45), (100, 30)] {
        let s = screen(width, height, "owner");
        let t = text(&draw(&s));
        assert!(t.contains("teams"), "{width}: navigator");
        assert!(t.contains("[1 terminals]"), "{width}: workspace tabs");
        assert!(t.contains("blockers"), "{width}: inspector");
        assert!(
            t.contains("All teams") && t.contains("Moo"),
            "{width}: tree"
        );
    }
}

#[test]
fn eighty_columns_collapse_the_side_areas_into_a_drawer() {
    let mut s = screen(80, 24, "owner");
    let t = text(&draw(&s));
    assert!(
        !t.contains("blockers") && !t.contains("All teams"),
        "collapsed: {t}"
    );
    assert!(t.contains("[1 terminals]"));
    press(&mut s, "n");
    assert_eq!((s.active, s.focus), (Area::Navigator, Focus::Navigator));
    assert!(text(&draw(&s)).contains("All teams"));
    press(&mut s, "n");
    assert_eq!(s.active, Area::Main);
}

#[test]
fn sixty_columns_switch_between_single_areas_from_a_strip() {
    let mut s = screen(60, 24, "owner");
    let first = draw(&s);
    assert!(
        rows(&first)[0].contains("[workspace]"),
        "{}",
        rows(&first)[0]
    );
    assert!(!text(&first).contains("All teams"));
    s.key(key(KeyCode::Tab));
    assert_eq!(s.active, Area::Inspector);
    let inspector = draw(&s);
    assert!(rows(&inspector)[0].contains("[info]") && text(&inspector).contains("blockers"));
    s.key(key(KeyCode::Tab));
    assert_eq!(s.active, Area::Navigator);
    assert!(rows(&draw(&s))[0].contains("[teams]"));
    assert!(text(&draw(&s)).contains("All teams"));
    // Choosing a team returns to the workspace.
    press(&mut s, "jjj");
    s.key(key(KeyCode::Enter));
    assert_eq!(
        (s.active, s.scope.clone()),
        (Area::Main, Scope::Team("alpha".into()))
    );
}

#[test]
fn key_hints_are_whole_words_at_every_width() {
    for width in [160, 100, 80, 60, 40] {
        let s = screen(width, 24, "owner");
        let footer = rows(&draw(&s)).pop().unwrap();
        assert!(
            footer.trim_start().starts_with("tab") || footer.trim_start().starts_with("j/k"),
            "{width}: {footer:?}"
        );
        // A hint is dropped whole; none is cut mid-word.
        for hint in ["quit", "help", "area"] {
            assert!(
                !footer.trim_end().ends_with(&hint[..hint.len() - 1]),
                "{width}: {footer:?}"
            );
        }
    }
}

#[test]
fn selecting_a_team_restores_its_view_cursor_and_agent() {
    let mut s = screen(160, 45, "owner");
    // alpha: tasks view, second task selected.
    s.scope = Scope::Team("alpha".into());
    s.focus = Focus::Main;
    press(&mut s, "3j");
    assert_eq!((s.view, s.cursor_of(View::Tasks)), (View::Tasks, 1));
    // beta: decisions.
    s.focus = Focus::Navigator;
    s.nav_cursor = s
        .nav_rows()
        .iter()
        .position(|r| r.node == super::state::NavNode::Team("beta".into()))
        .unwrap();
    s.key(key(KeyCode::Enter));
    assert_eq!(s.scope, Scope::Team("beta".into()));
    assert_eq!(
        s.view,
        View::Terminals,
        "a team with no history opens on terminals"
    );
    press(&mut s, "4");
    // Back to alpha: everything where it was left.
    s.focus = Focus::Navigator;
    s.nav_cursor = s
        .nav_rows()
        .iter()
        .position(|r| r.node == super::state::NavNode::Team("alpha".into()))
        .unwrap();
    s.key(key(KeyCode::Enter));
    assert_eq!((s.view, s.cursor_of(View::Tasks)), (View::Tasks, 1));
    // And the memory survives a restart.
    let saved = s.memory_snapshot();
    let mut fresh = Screen::new(Rect::new(0, 0, 160, 45));
    fresh.restore_memory(&saved);
    fresh.apply_snapshot(fixture("owner"));
    assert_eq!(
        (
            fresh.scope.clone(),
            fresh.view,
            fresh.cursor_of(View::Tasks)
        ),
        (Scope::Team("alpha".into()), View::Tasks, 1)
    );
}

#[test]
fn selecting_an_agent_focuses_it_and_asks_for_its_pane() {
    let mut s = screen(160, 45, "owner");
    s.nav_cursor = s
        .nav_rows()
        .iter()
        .position(|r| matches!(&r.node, super::state::NavNode::Agent { id, .. } if id == "dev1"))
        .unwrap();
    let effects = s.key(key(KeyCode::Enter));
    assert_eq!(
        effects,
        vec![Effect::FocusAgent {
            actor_id: "dev1".into(),
            pane_id: Some("pane-7".into())
        }]
    );
    assert_eq!(
        (s.view, s.agent.as_deref(), s.focus),
        (View::Terminals, Some("dev1"), Focus::Main)
    );
    let t = text(&draw(&s));
    assert!(
        t.contains("dev1") && t.contains("developer") && t.contains("working"),
        "inspector shows the agent: {t}"
    );
    assert!(
        t.contains("blocked by t1") || t.contains("t2"),
        "its blocked task is visible"
    );
}

#[test]
fn all_teams_aggregates_and_folding_hides_children() {
    let mut s = screen(160, 45, "owner");
    assert_eq!(s.scope_team_ids().len(), 2);
    assert_eq!(s.list_len(View::Tasks), 3);
    let before = s.nav_rows().len();
    s.nav_cursor = s
        .nav_rows()
        .iter()
        .position(|r| r.node == super::state::NavNode::Team("alpha".into()))
        .unwrap();
    s.key(key(KeyCode::Left));
    assert_eq!(s.nav_rows().len(), before - 3);
    s.key(key(KeyCode::Right));
    assert_eq!(s.nav_rows().len(), before);
}

#[test]
fn approving_works_from_the_keyboard_and_sends_exactly_the_approval_shown() {
    let mut s = screen(160, 45, "owner");
    s.focus = Focus::Main;
    press(&mut s, "4");
    let card = text(&draw(&s));
    assert!(
        card.contains("approval ap-1") && card.contains("protected_branch_integration -> main")
    );
    assert!(
        card.contains(r#""commit":"abc""#),
        "the exact scope is on the card"
    );
    assert!(card.contains("[a] approve") && card.contains("[d] deny"));
    // Approvals first, then conflicts; ap-1 is the first card.
    assert!(press(&mut s, "a").is_empty());
    let modal = text(&draw(&s));
    assert!(
        modal.contains("approve ap-1")
            && modal.contains("digest   0123456789abcdef")
            && modal.contains("single use")
    );
    let effects = s.key(key(KeyCode::Enter));
    assert_eq!(effects.len(), 1);
    let Effect::Mutate { command } = &effects[0] else {
        panic!("{effects:?}")
    };
    assert_eq!(command["kind"], "decide_approval");
    assert_eq!(
        (
            command["approval_id"].as_str(),
            command["approve"].as_bool()
        ),
        (Some("ap-1"), Some(true))
    );
}

#[test]
fn denying_needs_a_reason_and_escape_cancels() {
    let mut s = screen(100, 30, "owner");
    s.focus = Focus::Main;
    press(&mut s, "4d");
    assert!(
        s.key(key(KeyCode::Enter)).is_empty(),
        "no reason, nothing sent"
    );
    assert!(s.modal.is_some());
    press(&mut s, "too risky");
    let effects = s.key(key(KeyCode::Enter));
    let Effect::Mutate { command } = &effects[0] else {
        panic!()
    };
    assert_eq!(
        (command["approve"].as_bool(), command["rationale"].as_str()),
        (Some(false), Some("too risky"))
    );
    press(&mut s, "4a");
    s.key(key(KeyCode::Esc));
    assert!(s.modal.is_none());
}

#[test]
fn only_the_owner_is_offered_decisions_and_chat_text_never_becomes_one() {
    let mut s = screen(160, 45, "lead");
    s.focus = Focus::Main;
    press(&mut s, "4");
    assert!(text(&draw(&s)).contains("(owner only)"));
    assert!(press(&mut s, "a").is_empty() && s.modal.is_none());
    assert!(s.toast.as_deref().unwrap().contains("Owner"));
    // Typing an approval phrase into chat sends a chat message, nothing else.
    let mut s = screen(160, 45, "owner");
    s.scope = Scope::Team("alpha".into());
    s.focus = Focus::Main;
    press(&mut s, "2c");
    assert_eq!(s.focus, Focus::Composer);
    press(&mut s, "approve ap-1 @owner");
    let effects = s.key(key(KeyCode::Enter));
    let Effect::Mutate { command } = &effects[0] else {
        panic!()
    };
    assert_eq!(command["kind"], "append_chat");
    assert_eq!(command["team_id"], "alpha");
    assert!(s.composer.is_empty());
}

#[test]
fn chat_composer_needs_one_team_and_shows_structured_messages() {
    let mut s = screen(160, 45, "owner");
    s.focus = Focus::Main;
    press(&mut s, "2");
    assert!(press(&mut s, "c").is_empty());
    assert_eq!(s.focus, Focus::Main, "all-teams chat is read only");
    let all = text(&draw(&s));
    assert!(
        all.contains("handoff") && all.contains("cargo nextest run"),
        "typed handoff with its evidence: {all}"
    );
    s.scope = Scope::Team("alpha".into());
    let team = text(&draw(&s));
    assert!(team.contains("@dev1 please start on t1") && team.contains("write to Alpha"));
    assert!(
        !team.contains("done, see tests"),
        "another team's messages stay out"
    );
}

#[test]
fn tasks_show_owner_dependencies_progress_and_evidence() {
    let mut s = screen(160, 45, "owner");
    s.focus = Focus::Main;
    press(&mut s, "3");
    let t = text(&draw(&s));
    assert!(t.contains("owner dev1") && t.contains("needs t1") && t.contains("blocked by t1"));
    assert!(t.contains("1/5 in progress") && t.contains("2/5 awaiting QA"));
    press(&mut s, "jj");
    assert!(text(&draw(&s)).contains("evidence test: cargo nextest run"));
}

#[test]
fn statuses_are_words_as_well_as_colors() {
    let s = screen(160, 45, "owner");
    let t = text(&draw(&s));
    for word in ["working", "waiting", "idle", "paused"] {
        assert!(t.contains(word), "{word} missing from {t}");
    }
}

#[test]
fn long_names_are_cut_with_an_ellipsis_not_overflowed() {
    let s = screen(100, 30, "owner");
    let t = rows(&draw(&s)).join("\n");
    assert!(t.contains("Beta platform"));
    assert!(!t.contains("cannot fit"));
    assert!(t.contains('…'));
}

#[test]
fn loading_empty_and_reconnecting_states_are_explicit() {
    let blank = Screen::new(Rect::new(0, 0, 100, 30));
    let t = text(&draw(&blank));
    assert!(t.contains("loading"));
    let mut empty = Screen::new(Rect::new(0, 0, 100, 30));
    empty.apply_snapshot(Snapshot::default());
    assert!(text(&draw(&empty)).contains("no teams yet"));
    let mut lost = screen(160, 45, "owner");
    lost.set_link_error("connection refused".into());
    assert!(matches!(lost.link, Link::Reconnecting(_)));
    let t = text(&draw(&lost));
    assert!(t.contains("reconnecting - connection refused"), "{t}");
    assert!(t.contains("Alpha"), "the last data stays on screen");
}

#[test]
fn a_refresh_keeps_the_users_place_and_drops_what_vanished() {
    let mut s = screen(160, 45, "owner");
    s.scope = Scope::Team("beta".into());
    s.view = View::Tasks;
    s.apply_snapshot(fixture("owner"));
    assert_eq!(
        (s.scope.clone(), s.view),
        (Scope::Team("beta".into()), View::Tasks)
    );
    let mut gone = fixture("owner");
    gone.teams.retain(|t| t.id != "beta");
    s.apply_snapshot(gone);
    assert_eq!(s.scope, Scope::All);
}

#[test]
fn mouse_selects_rows_and_switches_views_when_it_lands_on_them() {
    let mut s = screen(160, 45, "owner");
    let click = |s: &mut Screen, x, y| {
        s.mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        })
    };
    let frame = s.frame();
    // Row 2 of the navigator is the first team (row 0 is "All teams", 1 the project).
    click(&mut s, 5, frame.navigator.unwrap().y + 3);
    assert_eq!(s.scope, Scope::Team("alpha".into()));
    let tabs = super::state::view_tabs(frame.main);
    let (rect, view) = tabs[2];
    click(&mut s, rect.x + 1, rect.y);
    assert_eq!((s.view, view), (View::Tasks, View::Tasks));
}

#[test]
fn display_options_toggle_from_the_keyboard() {
    let mut s = screen(160, 45, "owner");
    press(&mut s, "DAH");
    assert!(s.options.ascii && s.options.high_contrast);
    assert_eq!(s.options.density, super::theme::Density::Comfortable);
    press(&mut s, "?");
    assert!(text(&draw(&s)).contains("any key closes"));
    s.key(key(KeyCode::Char('x')));
    assert!(!s.help);
    assert_eq!(press(&mut s, "q"), vec![Effect::Quit]);
}

#[test]
fn ui_subcommand_parses_with_an_optional_credential_file() {
    let command = crate::cli::teams_command_for_tests();
    assert!(command
        .clone()
        .try_get_matches_from(["teams", "ui"])
        .is_ok());
    assert!(command
        .clone()
        .try_get_matches_from(["teams", "ui", "--credential-file", "k"])
        .is_ok());
    assert!(command
        .clone()
        .try_get_matches_from(["teams", "ui", "--view", "decisions", "--start", "builder"])
        .is_ok());
    assert!(command
        .clone()
        .try_get_matches_from(["teams", "ui", "--view", "nope"])
        .is_err());
    assert!(command
        .try_get_matches_from(["teams", "ui", "--token", "secret"])
        .is_err());
}

#[test]
fn selecting_an_agent_without_a_bound_pane_says_so_instead_of_guessing() {
    let mut s = screen(160, 45, "owner");
    s.nav_cursor = s
        .nav_rows()
        .iter()
        .position(|r| matches!(&r.node, super::state::NavNode::Agent { id, .. } if id == "lead1"))
        .unwrap();
    assert_eq!(
        s.key(key(KeyCode::Enter)),
        vec![Effect::FocusAgent {
            actor_id: "lead1".into(),
            pane_id: None
        }]
    );
}

#[test]
fn leading_mentions_choose_the_audience_and_the_server_gets_it_explicitly() {
    let mut s = screen(160, 45, "owner");
    s.scope = Scope::Team("alpha".into());
    s.focus = Focus::Main;
    let cases = [
        ("@lead please look", json!({"to": "role", "role": "lead"})),
        (
            "@dev, a question",
            json!({"to": "role", "role": "developer"}),
        ),
        ("@qa1 only you", json!({"to": "actor", "actor_id": "qa1"})),
        (
            "@owner heads up",
            json!({"to": "actor", "actor_id": "owner"}),
        ),
        ("hello @lead in the middle", json!({"to": "team"})),
        ("@nobody", json!({"to": "team"})),
        ("plain", json!({"to": "team"})),
    ];
    for (text, audience) in cases {
        s.composer = text.into();
        assert_eq!(s.composer_audience(), audience, "{text}");
    }
    press(&mut s, "2c");
    s.composer.clear();
    press(&mut s, "@qa1 ready?");
    assert!(
        text(&draw(&s)).contains("to qa1 (direct)"),
        "the composer says who will read it"
    );
    let effects = s.key(key(KeyCode::Enter));
    let Effect::Mutate { command } = &effects[0] else {
        panic!()
    };
    assert_eq!(
        command["audience"],
        json!({"to": "actor", "actor_id": "qa1"})
    );
    assert_eq!(command["body"], "@qa1 ready?", "the text is sent as typed");
    // All-teams chat has no single channel, so everything it would send is team-wide.
    s.scope = Scope::All;
    s.composer = "@lead x".into();
    assert_eq!(s.composer_audience(), json!({"to": "team"}));
}

#[test]
fn scoped_messages_are_labelled_in_the_log() {
    let mut s = screen(160, 45, "owner");
    s.scope = Scope::Team("alpha".into());
    s.focus = Focus::Main;
    press(&mut s, "2");
    let t = text(&draw(&s));
    assert!(
        t.contains("to developers") && t.contains("keep this between developers"),
        "{t}"
    );
    assert!(
        t.contains("to lead1 (direct)") && t.contains("a private word"),
        "{t}"
    );
}

#[test]
fn approval_cards_and_the_confirmation_show_alternatives_and_risks() {
    let mut s = screen(160, 45, "owner");
    s.focus = Focus::Main;
    press(&mut s, "4");
    let t = text(&draw(&s));
    assert!(
        t.contains("else") && t.contains("wait for the next release"),
        "{t}"
    );
    assert!(
        t.contains("risk") && t.contains("touches the public API"),
        "{t}"
    );
}

#[test]
fn the_inspector_shows_the_provider_and_pane_of_a_bound_agent() {
    let mut s = screen(160, 45, "owner");
    s.agent = Some("dev1".into());
    let t = text(&draw(&s));
    assert!(t.contains("claude") && t.contains("pane-7"), "{t}");
    s.agent = Some("lead1".into());
    assert!(text(&draw(&s)).contains("none bound"));
}

#[test]
fn a_pane_is_bound_and_unbound_from_the_keyboard() {
    let mut s = screen(160, 45, "owner");
    s.scope = Scope::Team("alpha".into());
    s.focus = Focus::Main;
    s.view = View::Terminals;
    press(&mut s, "j");
    assert_eq!(s.selected_agent_id().as_deref(), Some("dev1"));
    press(&mut s, "b");
    let modal = s.modal.clone().unwrap();
    assert_eq!(
        modal.input, "pane-7",
        "the current binding is offered for editing"
    );
    s.modal.as_mut().unwrap().input.clear();
    press(&mut s, "pane-9 my provider");
    let effects = s.key(key(KeyCode::Enter));
    let Effect::Mutate { command } = &effects[0] else {
        panic!("{effects:?}")
    };
    assert_eq!(command["kind"], "bind_agent_pane");
    assert_eq!(
        (command["pane_id"].as_str(), command["provider"].as_str()),
        (Some("pane-9"), Some("my provider"))
    );
    let effects = press(&mut s, "B");
    assert!(
        matches!(&effects[0], Effect::Mutate { command } if command["kind"] == "unbind_agent_pane")
    );
}

#[test]
fn the_command_palette_offers_what_applies_here_and_runs_it() {
    let mut s = screen(160, 45, "owner");
    press(&mut s, ":");
    assert!(text(&draw(&s)).contains("commands") && s.palette.is_some());
    let labels: Vec<String> = s.palette_matches().into_iter().map(|(l, _)| l).collect();
    assert!(
        labels.iter().any(|l| l == "go to chat")
            && labels.iter().any(|l| l == "new team (manual builder)")
    );
    assert!(
        !labels.iter().any(|l| l.contains("approve")),
        "nothing to approve in terminals"
    );
    s.key(key(KeyCode::Esc));
    assert!(s.palette.is_none());
    // In decisions with a pending approval selected, approve and deny appear and approve opens the confirmation.
    s.focus = Focus::Main;
    press(&mut s, "4:approve");
    let matches = s.palette_matches();
    assert_eq!(matches.len(), 1, "{matches:?}");
    s.key(key(KeyCode::Enter));
    assert_eq!(s.modal.as_ref().unwrap().id, "ap-1");
    s.key(key(KeyCode::Esc));
    // A Lead is not offered owner-only commands, but may rename their own team.
    let mut lead = screen(160, 45, "lead");
    lead.scope = Scope::Team("alpha".into());
    let labels: Vec<String> = lead.palette_items().into_iter().map(|(l, _)| l).collect();
    assert!(!labels
        .iter()
        .any(|l| l.contains("new team") || l.contains("propose")));
    assert!(
        labels.iter().any(|l| l == "rename team Alpha")
            && labels.iter().any(|l| l == "suspend team Alpha")
    );
    // Suspended teams offer resume instead.
    lead.scope = Scope::Team("beta".into());
    let labels: Vec<String> = lead
        .palette_items()
        .into_iter()
        .map(|(l, _)| l.to_string())
        .collect();
    assert!(
        labels.iter().any(|l| l.starts_with("resume team"))
            && !labels.iter().any(|l| l.starts_with("suspend team"))
    );
}

#[test]
fn the_manual_builder_has_the_server_check_the_draft_and_reviews_before_anything_is_created() {
    use super::builder::Preview;
    use super::state::BuilderStep;
    let mut s = screen(160, 45, "owner");
    press(&mut s, ":new team");
    s.key(key(KeyCode::Enter));
    assert_eq!(s.builder.as_ref().unwrap().draft.source, "manual");
    press(&mut s, "Docs team");
    // The form walks the fields; only the Enter on the last one asks the server to check the draft.
    for _ in 0..4 {
        assert!(s.key(key(KeyCode::Enter)).is_empty());
    }
    let effects = s.key(key(KeyCode::Enter));
    let [Effect::ValidateDraft { draft }] = effects.as_slice() else {
        panic!("{effects:?}")
    };
    assert_eq!(
        (draft.name.as_str(), draft.lead.as_str()),
        ("Docs team", "docs-team-lead")
    );
    assert!(s.builder.as_ref().unwrap().checking);
    assert!(
        text(&draw(&s)).contains("asking the server"),
        "the screen says it is waiting"
    );
    assert!(
        s.key(key(KeyCode::Enter)).is_empty(),
        "no second request while one is out"
    );
    // A draft the server refuses stays in edit mode and says why.
    s.apply_draft_check(Ok(Preview {
        valid: false,
        problems: vec!["lead id 'docs-team-lead' is already taken".into()],
        team_id: "docs-team".into(),
        credential_dir: "teams-keys/docs-team".into(),
    }));
    assert_eq!(s.builder.as_ref().unwrap().step, BuilderStep::Edit);
    assert!(s.toast.as_deref().unwrap().contains("already taken"));
    assert!(text(&draw(&s)).contains("already taken"));
    assert!(!s.builder.as_ref().unwrap().checking);
    // Editing clears the stale verdict; a valid check moves to review, which sends nothing yet.
    press(&mut s, "x");
    assert!(s.builder.as_ref().unwrap().preview.is_none());
    s.builder.as_mut().unwrap().field = 4;
    assert!(matches!(
        s.key(key(KeyCode::Enter)).as_slice(),
        [Effect::ValidateDraft { .. }]
    ));
    s.apply_draft_check(Ok(Preview {
        valid: true,
        problems: vec![],
        team_id: "docs-team".into(),
        credential_dir: "teams-keys/docs-team".into(),
    }));
    assert_eq!(s.builder.as_ref().unwrap().step, BuilderStep::Review);
    let review = text(&draw(&s));
    assert!(review.contains("nothing is created until you launch") && review.contains("docs-team"));
    assert!(
        review.contains("teams-keys/docs-team"),
        "the server says where the keys go"
    );
    // Back to edit sends nothing; launching sends exactly the reviewed draft to the server.
    s.key(key(KeyCode::Char('e')));
    assert_eq!(s.builder.as_ref().unwrap().step, BuilderStep::Edit);
    s.builder.as_mut().unwrap().step = BuilderStep::Review;
    let effects = s.key(key(KeyCode::Enter));
    let [Effect::LaunchDraft { draft }] = effects.as_slice() else {
        panic!("{effects:?}")
    };
    assert!(draft.name.starts_with("Docs team"));
    assert!(s.builder.is_none());
    // Escape abandons a draft.
    press(&mut s, ":new team");
    s.key(key(KeyCode::Enter));
    assert!(s.key(key(KeyCode::Esc)).is_empty() && s.builder.is_none());
}

#[test]
fn a_goal_proposal_comes_from_the_server_and_lands_in_the_same_editable_builder() {
    use super::state::BuilderStep;
    let mut s = screen(160, 45, "owner");
    press(&mut s, ":propose");
    s.key(key(KeyCode::Enter));
    assert_eq!(
        s.modal.as_ref().unwrap().title,
        "propose a team from a goal"
    );
    assert!(
        s.key(key(KeyCode::Enter)).is_empty(),
        "an empty goal is not sent"
    );
    press(&mut s, "Ship the plugin settings");
    let effects = s.key(key(KeyCode::Enter));
    assert_eq!(
        effects,
        vec![Effect::Propose {
            goal: "Ship the plugin settings".into()
        }]
    );
    // The server's proposal arrives: it opens for editing, never straight into launch.
    let reply = json!({"source": "template", "draft": {"name": "Ship the plugin settings", "project_id": "moo",
        "lead": "ship-lead", "developer": "ship-dev", "qa": "ship-qa"}});
    s.open_builder(super::builder::Draft::from_proposal(&reply).unwrap());
    let builder = s.builder.as_ref().unwrap();
    assert_eq!(builder.step, BuilderStep::Edit);
    assert_eq!(
        (builder.draft.source.as_str(), builder.draft.lead.as_str()),
        ("template", "ship-lead")
    );
    assert!(text(&draw(&s)).contains("source: template"));
    // Editing the team name does not rewrite the proposer's agent ids.
    press(&mut s, "!");
    assert_eq!(s.builder.as_ref().unwrap().draft.lead, "ship-lead");
}

#[test]
fn themes_preview_live_follow_the_config_and_are_remembered() {
    let mut s = screen(160, 45, "owner");
    let configured = Palette::catppuccin();
    assert_eq!(super::render::resolve_palette(&s, &configured), configured);
    press(&mut s, "t");
    assert_eq!(s.theme.as_deref(), Some("catppuccin"));
    press(&mut s, "t");
    let second = s.theme.clone().unwrap();
    assert_ne!(second, "catppuccin");
    let buf = {
        let area = Rect::new(0, 0, 160, 45);
        let mut buf = Buffer::empty(area);
        render(
            &mut buf,
            area,
            &s,
            &super::render::resolve_palette(&s, &configured),
        );
        buf
    };
    assert!(text(&buf).contains(&format!("theme {second}")));
    let saved = s.memory_snapshot();
    let mut fresh = Screen::new(Rect::new(0, 0, 160, 45));
    fresh.restore_memory(&saved);
    assert_eq!(fresh.theme.as_deref(), Some(second.as_str()));
    press(&mut s, "T");
    assert!(s.theme.is_none());
    // Every built-in theme draws every view without panicking.
    for name in crate::config::THEME_NAMES {
        s.theme = Some((*name).to_owned());
        for view in View::ALL {
            s.view = view;
            let area = Rect::new(0, 0, 100, 30);
            let mut buf = Buffer::empty(area);
            render(
                &mut buf,
                area,
                &s,
                &super::render::resolve_palette(&s, &configured),
            );
        }
    }
}

#[test]
fn enter_walks_every_field_and_only_the_last_one_asks_the_server() {
    let mut s = screen(160, 45, "owner");
    press(&mut s, ":new team");
    s.key(key(KeyCode::Enter));
    press(&mut s, "Docs team");
    for expected in 1..=4 {
        assert!(s.key(key(KeyCode::Enter)).is_empty());
        let builder = s.builder.as_ref().unwrap();
        assert_eq!(
            (builder.field, builder.step),
            (expected, super::state::BuilderStep::Edit)
        );
    }
    assert!(matches!(
        s.key(key(KeyCode::Enter)).as_slice(),
        [Effect::ValidateDraft { .. }]
    ));
}

#[test]
fn agent_ids_are_not_cut_when_there_is_room_for_them() {
    let mut s = screen(160, 45, "owner");
    s.snapshot.actors[1].id = "a-rather-long-agent-name".into();
    s.snapshot.tasks[0].developer_id = "a-rather-long-agent-name".into();
    s.snapshot.teams[0].developer_id = "a-rather-long-agent-name".into();
    assert!(text(&draw(&s)).contains("a-rather-long-agent-name"));
}
