//! Screen state and input handling. Pure: keys go in, state changes and `Effect`s come out.
//! The event loop owns all I/O; nothing here touches the terminal, the socket or a clock.

use std::collections::{BTreeMap, BTreeSet};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::builder::{Draft, Preview};
use super::data::{tag, Snapshot};
use super::layout::{self, Area, Frame, Mode, Visibility};
use super::theme::{Density, Options};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum View {
    Terminals,
    Chat,
    Tasks,
    Decisions,
}

impl View {
    pub const ALL: [View; 4] = [View::Terminals, View::Chat, View::Tasks, View::Decisions];

    pub fn label(self) -> &'static str {
        match self {
            Self::Terminals => "terminals",
            Self::Chat => "chat",
            Self::Tasks => "tasks",
            Self::Decisions => "decisions",
        }
    }

    fn step(self, forward: bool) -> Self {
        let index = Self::ALL.iter().position(|view| *view == self).unwrap_or(0);
        let count = Self::ALL.len();
        Self::ALL[(index + if forward { 1 } else { count - 1 }) % count]
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Scope {
    All,
    Project(String),
    Team(String),
}

impl Scope {
    fn key(&self) -> String {
        match self {
            Self::All => "all".into(),
            Self::Project(id) => format!("project:{id}"),
            Self::Team(id) => format!("team:{id}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NavNode {
    AllTeams,
    Project(String),
    Team(String),
    Agent { team: String, id: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavRow {
    pub node: NavNode,
    pub depth: u8,
    /// Has children, so can be collapsed.
    pub branch: bool,
    pub collapsed: bool,
}

/// What the screen remembers per scope so coming back lands where the user left off.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Memory {
    pub view: Option<View>,
    pub agent: Option<String>,
    pub cursor: BTreeMap<View, usize>,
    pub scroll: BTreeMap<View, usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Link {
    Loading,
    Live,
    /// The last poll failed; the data on screen is from before.
    Reconnecting(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Navigator,
    Main,
    Inspector,
    Composer,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Approve,
    Deny,
    Revoke,
    Resolve,
    /// Text prompts: the modal's input is the answer.
    Bind,
    Rename,
    Suspend,
    Goal,
}

/// A confirmation that shows the exact scope before anything is sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Modal {
    pub action: Action,
    pub id: String,
    pub title: String,
    pub lines: Vec<String>,
    pub input: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    Quit,
    Refresh,
    Mutate {
        command: Value,
    },
    /// Show the agent's terminal pane; `pane_id` is the bound pane, when there is one.
    FocusAgent {
        actor_id: String,
        pane_id: Option<String>,
    },
    /// Ask the server whether the draft is valid and what launching it would do.
    ValidateDraft {
        draft: Draft,
    },
    /// Create the reviewed team through the server.
    LaunchDraft {
        draft: Draft,
    },
    /// Ask the proposer for a draft from a goal.
    Propose {
        goal: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuilderStep {
    Edit,
    Review,
}

pub const BUILDER_FIELDS: [&str; 5] = ["team name", "project", "lead id", "developer id", "qa id"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Builder {
    pub draft: Draft,
    pub step: BuilderStep,
    pub field: usize,
    /// What the server said about this draft, once asked.
    pub preview: Option<Preview>,
    /// A validation request is out; the screen waits instead of asking twice.
    pub checking: bool,
}

/// What a command-palette entry does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Intent {
    View(View),
    ToggleNavigator,
    ToggleInspector,
    Density,
    Ascii,
    Contrast,
    CycleTheme,
    Refresh,
    Help,
    Quit,
    Decide(Action),
    NewTeam,
    ProposeTeam,
    BindPane,
    UnbindPane,
    RenameTeam,
    SuspendTeam,
    ResumeTeam,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PaletteState {
    pub query: String,
    pub cursor: usize,
}

/// A card in the decisions view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Card {
    Approval(usize),
    Conflict(usize),
    Decision(usize),
}

pub struct Screen {
    pub snapshot: Snapshot,
    pub link: Link,
    pub loaded: bool,
    pub scope: Scope,
    pub view: View,
    pub focus: Focus,
    pub nav_cursor: usize,
    pub collapsed: BTreeSet<String>,
    pub memory: BTreeMap<String, Memory>,
    pub agent: Option<String>,
    pub show_navigator: bool,
    pub show_inspector: bool,
    pub active: Area,
    pub composer: String,
    pub modal: Option<Modal>,
    pub builder: Option<Builder>,
    pub palette: Option<PaletteState>,
    /// A built-in theme chosen for this session; `None` follows the configured one.
    pub theme: Option<String>,
    pub help: bool,
    pub toast: Option<String>,
    pub options: Options,
    pub area: Rect,
    pub cursor: BTreeMap<View, usize>,
    pub scroll: BTreeMap<View, usize>,
    next_message: u64,
}

impl Screen {
    pub fn new(area: Rect) -> Self {
        Self {
            snapshot: Snapshot::default(),
            link: Link::Loading,
            loaded: false,
            scope: Scope::All,
            view: View::Terminals,
            // With the side areas collapsed, focus starts on the workspace that is actually showing.
            focus: if matches!(layout::mode_for(area.width), Mode::Full | Mode::Reduced) {
                Focus::Navigator
            } else {
                Focus::Main
            },
            nav_cursor: 0,
            collapsed: BTreeSet::new(),
            memory: BTreeMap::new(),
            agent: None,
            show_navigator: true,
            show_inspector: true,
            active: Area::Main,
            composer: String::new(),
            modal: None,
            builder: None,
            palette: None,
            theme: None,
            help: false,
            toast: None,
            options: Options::default(),
            area,
            cursor: BTreeMap::new(),
            scroll: BTreeMap::new(),
            next_message: 0,
        }
    }

    pub fn visibility(&self) -> Visibility {
        Visibility {
            navigator: self.show_navigator,
            inspector: self.show_inspector,
            active: self.active,
        }
    }

    pub fn frame(&self) -> Frame {
        layout::compute(self.area, self.visibility())
    }

    pub fn is_owner(&self) -> bool {
        self.snapshot.viewer.role == "owner"
    }

    /// Replaces the data. The user's place survives: scope, view, cursor and scroll are kept, and
    /// anything that no longer exists falls back to the closest thing that does.
    pub fn apply_snapshot(&mut self, snapshot: Snapshot) {
        self.snapshot = snapshot;
        self.loaded = true;
        self.link = Link::Live;
        match &self.scope {
            Scope::Team(id) if self.snapshot.team(id).is_none() => self.scope = Scope::All,
            Scope::Project(id) if !self.snapshot.projects.iter().any(|p| &p.id == id) => {
                self.scope = Scope::All;
            }
            _ => {}
        }
        if self
            .agent
            .as_ref()
            .is_some_and(|id| self.snapshot.actor(id).is_none())
        {
            self.agent = None;
        }
        let rows = self.nav_rows().len();
        self.nav_cursor = self.nav_cursor.min(rows.saturating_sub(1));
    }

    pub fn set_link_error(&mut self, message: String) {
        self.link = Link::Reconnecting(message);
    }

    pub fn resize(&mut self, area: Rect) {
        self.area = area;
        // Focus must stay on something that is on screen.
        match layout::mode_for(area.width) {
            Mode::Full | Mode::Reduced => self.active = Area::Main,
            Mode::Collapsed | Mode::Single => {
                self.focus = match (self.focus, self.active) {
                    (Focus::Composer, _) => Focus::Composer,
                    (_, Area::Navigator) => Focus::Navigator,
                    (_, Area::Inspector) => Focus::Inspector,
                    (_, Area::Main) => Focus::Main,
                };
            }
        }
    }

    // ---- scope ------------------------------------------------------------------------------

    pub fn scope_team_ids(&self) -> Vec<&str> {
        match &self.scope {
            Scope::All => self.snapshot.teams.iter().map(|t| t.id.as_str()).collect(),
            Scope::Project(id) => self
                .snapshot
                .teams
                .iter()
                .filter(|t| &t.project_id == id)
                .map(|t| t.id.as_str())
                .collect(),
            Scope::Team(id) => vec![id.as_str()],
        }
    }

    pub fn in_scope(&self, team_id: &str) -> bool {
        self.scope_team_ids().contains(&team_id)
    }

    pub fn scope_title(&self) -> String {
        match &self.scope {
            Scope::All => "all teams".into(),
            Scope::Project(id) => self
                .snapshot
                .projects
                .iter()
                .find(|p| &p.id == id)
                .map_or_else(|| id.clone(), |p| p.name.clone()),
            Scope::Team(id) => self.snapshot.team_label(id),
        }
    }

    fn remember(&mut self) {
        let memory = Memory {
            view: Some(self.view),
            agent: self.agent.clone(),
            cursor: self.cursor.clone(),
            scroll: self.scroll.clone(),
        };
        self.memory.insert(self.scope.key(), memory);
    }

    fn switch_scope(&mut self, scope: Scope) {
        if scope == self.scope {
            return;
        }
        self.remember();
        self.scope = scope;
        let memory = self
            .memory
            .get(&self.scope.key())
            .cloned()
            .unwrap_or_default();
        self.view = memory.view.unwrap_or(View::Terminals);
        self.agent = memory.agent.filter(|id| self.snapshot.actor(id).is_some());
        self.cursor = memory.cursor;
        self.scroll = memory.scroll;
    }

    pub fn memory_snapshot(&mut self) -> Value {
        self.remember();
        json!({
            "scope": self.scope.key(),
            "theme": self.theme,
            "options": { "ascii": self.options.ascii, "compact": self.options.density == Density::Compact,
                "high_contrast": self.options.high_contrast },
            "memory": self.memory,
        })
    }

    pub fn restore_memory(&mut self, saved: &Value) {
        if let Some(map) = saved
            .get("memory")
            .and_then(|m| serde_json::from_value(m.clone()).ok())
        {
            self.memory = map;
        }
        if let Some(options) = saved.get("options") {
            self.options.ascii = options["ascii"].as_bool().unwrap_or(false);
            self.options.high_contrast = options["high_contrast"].as_bool().unwrap_or(false);
            self.options.density = if options["compact"].as_bool().unwrap_or(true) {
                Density::Compact
            } else {
                Density::Comfortable
            };
        }
        self.theme = saved["theme"]
            .as_str()
            .filter(|name| crate::config::THEME_NAMES.contains(name))
            .map(str::to_owned);
        let key = saved["scope"].as_str().unwrap_or("all");
        let scope = if let Some(id) = key.strip_prefix("team:") {
            Scope::Team(id.to_owned())
        } else if let Some(id) = key.strip_prefix("project:") {
            Scope::Project(id.to_owned())
        } else {
            Scope::All
        };
        let memory = self.memory.get(&scope.key()).cloned().unwrap_or_default();
        self.scope = scope;
        self.view = memory.view.unwrap_or(View::Terminals);
        self.agent = memory.agent;
        self.cursor = memory.cursor;
        self.scroll = memory.scroll;
    }

    // ---- navigator --------------------------------------------------------------------------

    pub fn nav_rows(&self) -> Vec<NavRow> {
        let mut rows = vec![NavRow {
            node: NavNode::AllTeams,
            depth: 0,
            branch: false,
            collapsed: false,
        }];
        for project in &self.snapshot.projects {
            let project_key = format!("project:{}", project.id);
            let collapsed = self.collapsed.contains(&project_key);
            rows.push(NavRow {
                node: NavNode::Project(project.id.clone()),
                depth: 0,
                branch: true,
                collapsed,
            });
            if collapsed {
                continue;
            }
            for team in self
                .snapshot
                .teams
                .iter()
                .filter(|t| t.project_id == project.id)
            {
                let team_key = format!("team:{}", team.id);
                let collapsed = self.collapsed.contains(&team_key);
                rows.push(NavRow {
                    node: NavNode::Team(team.id.clone()),
                    depth: 1,
                    branch: true,
                    collapsed,
                });
                if collapsed {
                    continue;
                }
                for agent in self.snapshot.team_agents(&team.id) {
                    rows.push(NavRow {
                        node: NavNode::Agent {
                            team: team.id.clone(),
                            id: agent.id.clone(),
                        },
                        depth: 2,
                        branch: false,
                        collapsed: false,
                    });
                }
            }
        }
        rows
    }

    fn collapse_key(node: &NavNode) -> Option<String> {
        match node {
            NavNode::Project(id) => Some(format!("project:{id}")),
            NavNode::Team(id) => Some(format!("team:{id}")),
            _ => None,
        }
    }

    fn select_row(&mut self, index: usize) -> Vec<Effect> {
        let rows = self.nav_rows();
        let Some(row) = rows.get(index) else {
            return Vec::new();
        };
        self.nav_cursor = index;
        match row.node.clone() {
            NavNode::AllTeams => self.switch_scope(Scope::All),
            NavNode::Project(id) => self.switch_scope(Scope::Project(id)),
            NavNode::Team(id) => self.switch_scope(Scope::Team(id)),
            NavNode::Agent { team, id } => {
                self.switch_scope(Scope::Team(team));
                self.agent = Some(id.clone());
                self.view = View::Terminals;
                self.focus = Focus::Main;
                self.close_drawer();
                return vec![self.focus_effect(id)];
            }
        }
        self.close_drawer();
        Vec::new()
    }

    fn close_drawer(&mut self) {
        if matches!(
            layout::mode_for(self.area.width),
            Mode::Collapsed | Mode::Single
        ) {
            self.active = Area::Main;
        }
    }

    // ---- lists the views show ---------------------------------------------------------------

    pub fn cards(&self) -> Vec<Card> {
        let mut cards = Vec::new();
        let snap = &self.snapshot;
        let pending = |status: &Value| tag(status) == "pending";
        for (i, a) in snap.approvals.iter().enumerate() {
            if self.in_scope(&a.team_id) && pending(&a.status) {
                cards.push(Card::Approval(i));
            }
        }
        for (i, c) in snap.conflicts.iter().enumerate() {
            if tag(&c.status) == "open" && c.team_ids.iter().any(|t| self.in_scope(t)) {
                cards.push(Card::Conflict(i));
            }
        }
        for (i, a) in snap.approvals.iter().enumerate() {
            if self.in_scope(&a.team_id) && !pending(&a.status) {
                cards.push(Card::Approval(i));
            }
        }
        for (i, d) in snap.decisions.iter().enumerate() {
            if self.in_scope(&d.team_id) {
                cards.push(Card::Decision(i));
            }
        }
        cards
    }

    pub fn list_len(&self, view: View) -> usize {
        let snap = &self.snapshot;
        match view {
            View::Terminals => self
                .scope_team_ids()
                .iter()
                .map(|team| snap.team_agents(team).len())
                .sum(),
            View::Chat => snap
                .messages
                .iter()
                .filter(|m| self.in_scope(&m.team_id))
                .count(),
            View::Tasks => snap
                .tasks
                .iter()
                .filter(|t| self.in_scope(&t.team_id))
                .count(),
            View::Decisions => self.cards().len(),
        }
    }

    pub fn cursor_of(&self, view: View) -> usize {
        let len = self.list_len(view);
        self.cursor
            .get(&view)
            .copied()
            .unwrap_or(0)
            .min(len.saturating_sub(1))
    }

    /// Moves the selection down the list; in chat, where 0 is the newest message, "down" is newer.
    fn move_cursor(&mut self, delta: isize) {
        let len = self.list_len(self.view);
        if len == 0 {
            return;
        }
        let delta = if self.view == View::Chat {
            -delta
        } else {
            delta
        };
        let current = self.cursor_of(self.view) as isize;
        let next = (current + delta).clamp(0, len as isize - 1) as usize;
        self.cursor.insert(self.view, next);
        if self.view == View::Terminals {
            self.agent = self.agent_at(next);
        }
    }

    pub fn agent_at(&self, index: usize) -> Option<String> {
        self.scope_team_ids()
            .iter()
            .flat_map(|team| self.snapshot.team_agents(team))
            .nth(index)
            .map(|actor| actor.id.clone())
    }

    // ---- input ------------------------------------------------------------------------------

    pub fn key(&mut self, key: KeyEvent) -> Vec<Effect> {
        if self.modal.is_some() {
            return self.modal_key(key);
        }
        if self.builder.is_some() {
            return self.builder_key(key);
        }
        if self.palette.is_some() {
            return self.palette_key(key);
        }
        if self.help {
            self.help = false;
            return Vec::new();
        }
        self.toast = None;
        if self.focus == Focus::Composer {
            return self.composer_key(key);
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('c') if ctrl => return vec![Effect::Quit],
            KeyCode::Char('p') if ctrl => self.palette = Some(PaletteState::default()),
            KeyCode::Char('q') => return vec![Effect::Quit],
            KeyCode::Char('?') => self.help = true,
            KeyCode::Char('r') => return vec![Effect::Refresh],
            KeyCode::Char('D') => {
                self.options.density = match self.options.density {
                    Density::Compact => Density::Comfortable,
                    Density::Comfortable => Density::Compact,
                };
            }
            KeyCode::Char('A') => self.options.ascii = !self.options.ascii,
            KeyCode::Char('t') => self.cycle_theme(),
            KeyCode::Char('T') => self.theme = None,
            KeyCode::Char(':') => self.palette = Some(PaletteState::default()),
            KeyCode::Char('H') => self.options.high_contrast = !self.options.high_contrast,
            KeyCode::Char('n') => self.toggle(Area::Navigator),
            KeyCode::Char('i') => self.toggle(Area::Inspector),
            KeyCode::Tab => self.cycle_focus(true),
            KeyCode::BackTab => self.cycle_focus(false),
            KeyCode::Char(c @ '1'..='4') => {
                self.view = View::ALL[(c as u8 - b'1') as usize];
                self.focus = Focus::Main;
            }
            KeyCode::Char(']') => self.view = self.view.step(true),
            KeyCode::Char('[') => self.view = self.view.step(false),
            _ => return self.focused_key(key),
        }
        Vec::new()
    }

    fn toggle(&mut self, area: Area) {
        let single_or_collapsed = matches!(
            layout::mode_for(self.area.width),
            Mode::Collapsed | Mode::Single
        );
        if single_or_collapsed {
            self.active = if self.active == area {
                Area::Main
            } else {
                area
            };
            self.focus = match self.active {
                Area::Navigator => Focus::Navigator,
                Area::Inspector => Focus::Inspector,
                Area::Main => Focus::Main,
            };
            return;
        }
        match area {
            Area::Navigator => {
                self.show_navigator = !self.show_navigator;
                if !self.show_navigator && self.focus == Focus::Navigator {
                    self.focus = Focus::Main;
                }
            }
            Area::Inspector => {
                self.show_inspector = !self.show_inspector;
                if !self.show_inspector && self.focus == Focus::Inspector {
                    self.focus = Focus::Main;
                }
            }
            Area::Main => {}
        }
    }

    fn cycle_focus(&mut self, forward: bool) {
        let frame = self.frame();
        let mut order = Vec::new();
        if frame.mode == Mode::Single {
            order = vec![Area::Navigator, Area::Main, Area::Inspector];
        } else {
            if frame.navigator.is_some() {
                order.push(Area::Navigator);
            }
            order.push(Area::Main);
            if frame.inspector.is_some() {
                order.push(Area::Inspector);
            }
        }
        let current = match self.focus {
            Focus::Navigator => Area::Navigator,
            Focus::Inspector => Area::Inspector,
            _ => Area::Main,
        };
        let index = order.iter().position(|a| *a == current).unwrap_or(0);
        let next = order[(index + if forward { 1 } else { order.len() - 1 }) % order.len()];
        self.focus = match next {
            Area::Navigator => Focus::Navigator,
            Area::Inspector => Focus::Inspector,
            Area::Main => Focus::Main,
        };
        if matches!(frame.mode, Mode::Single | Mode::Collapsed) {
            self.active = next;
        }
    }

    fn focused_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        let down = matches!(key.code, KeyCode::Down | KeyCode::Char('j'));
        let up = matches!(key.code, KeyCode::Up | KeyCode::Char('k'));
        match self.focus {
            Focus::Navigator => self.navigator_key(key, up, down),
            Focus::Main => self.main_key(key, up, down),
            Focus::Inspector | Focus::Composer => Vec::new(),
        }
    }

    fn navigator_key(&mut self, key: KeyEvent, up: bool, down: bool) -> Vec<Effect> {
        let rows = self.nav_rows();
        if up {
            self.nav_cursor = self.nav_cursor.saturating_sub(1);
        } else if down {
            self.nav_cursor = (self.nav_cursor + 1).min(rows.len().saturating_sub(1));
        } else {
            match key.code {
                KeyCode::Home | KeyCode::Char('g') => self.nav_cursor = 0,
                KeyCode::End | KeyCode::Char('G') => self.nav_cursor = rows.len().saturating_sub(1),
                KeyCode::Enter | KeyCode::Char(' ') => return self.select_row(self.nav_cursor),
                KeyCode::Right | KeyCode::Char('l') => {
                    if let Some(key) = rows
                        .get(self.nav_cursor)
                        .and_then(|r| Self::collapse_key(&r.node))
                    {
                        self.collapsed.remove(&key);
                    }
                }
                KeyCode::Left | KeyCode::Char('h') => {
                    if let Some(key) = rows
                        .get(self.nav_cursor)
                        .and_then(|r| Self::collapse_key(&r.node))
                    {
                        self.collapsed.insert(key);
                    }
                }
                _ => {}
            }
        }
        Vec::new()
    }

    fn main_key(&mut self, key: KeyEvent, up: bool, down: bool) -> Vec<Effect> {
        if up {
            self.move_cursor(-1);
        } else if down {
            self.move_cursor(1);
        } else {
            match (self.view, key.code) {
                (_, KeyCode::PageUp) => self.move_cursor(-10),
                (_, KeyCode::PageDown) => self.move_cursor(10),
                (_, KeyCode::Home | KeyCode::Char('g')) => {
                    let first = if self.view == View::Chat {
                        self.list_len(View::Chat).saturating_sub(1)
                    } else {
                        0
                    };
                    self.cursor.insert(self.view, first);
                }
                (_, KeyCode::End | KeyCode::Char('G')) => {
                    let last = if self.view == View::Chat {
                        0
                    } else {
                        self.list_len(self.view).saturating_sub(1)
                    };
                    self.cursor.insert(self.view, last);
                }
                (View::Terminals, KeyCode::Char('b')) => return self.run_intent(Intent::BindPane),
                (View::Terminals, KeyCode::Char('B')) => {
                    return self.run_intent(Intent::UnbindPane)
                }
                (View::Terminals, KeyCode::Enter) => {
                    if let Some(id) = self.agent_at(self.cursor_of(View::Terminals)) {
                        self.agent = Some(id.clone());
                        return vec![self.focus_effect(id)];
                    }
                }
                (View::Chat, KeyCode::Enter | KeyCode::Char('c')) => {
                    if matches!(self.scope, Scope::Team(_)) {
                        self.focus = Focus::Composer;
                    } else {
                        self.toast = Some("select one team to write in its chat".into());
                    }
                }
                (View::Decisions, KeyCode::Char('a')) => return self.open_modal(Action::Approve),
                (View::Decisions, KeyCode::Char('d')) => return self.open_modal(Action::Deny),
                (View::Decisions, KeyCode::Char('v')) => return self.open_modal(Action::Revoke),
                (View::Decisions, KeyCode::Char('x')) => return self.open_modal(Action::Resolve),
                _ => {}
            }
        }
        Vec::new()
    }

    fn open_modal(&mut self, action: Action) -> Vec<Effect> {
        if !self.is_owner() {
            self.toast = Some("only the Owner can decide this".into());
            return Vec::new();
        }
        let cards = self.cards();
        let Some(card) = cards.get(self.cursor_of(View::Decisions)).cloned() else {
            return Vec::new();
        };
        let snap = &self.snapshot;
        let (id, title, lines, input) = match (&action, card) {
            (Action::Approve | Action::Deny | Action::Revoke, Card::Approval(i)) => {
                let a = &snap.approvals[i];
                let live = matches!(tag(&a.status), "pending")
                    || (action == Action::Revoke && tag(&a.status) == "approved");
                if !live {
                    self.toast = Some(format!("{} is already {}", a.id, tag(&a.status)));
                    return Vec::new();
                }
                let verb = match action {
                    Action::Approve => "approve",
                    Action::Deny => "deny",
                    _ => "revoke",
                };
                let mut lines = vec![
                    format!("team     {}", snap.team_label(&a.team_id)),
                    format!("by       {}", a.requested_by),
                    format!("action   {} -> {}", a.action.kind, a.action.target),
                ];
                lines.push(format!("scope    {}", compact_json(&a.action.parameters)));
                lines.push(format!(
                    "digest   {}",
                    a.action_digest.chars().take(16).collect::<String>()
                ));
                lines.push(format!(
                    "valid    {} seconds once approved, single use",
                    a.ttl_seconds
                ));
                (
                    a.id.clone(),
                    format!("{verb} {}", a.id),
                    lines,
                    if action == Action::Approve {
                        "approved in Moo Teams UI".to_owned()
                    } else {
                        String::new()
                    },
                )
            }
            (Action::Resolve, Card::Conflict(i)) => {
                let c = &snap.conflicts[i];
                let lines = vec![
                    format!("kind     {}", c.kind),
                    format!(
                        "between  {}",
                        c.team_ids
                            .iter()
                            .map(|t| snap.team_label(t))
                            .collect::<Vec<_>>()
                            .join(" / ")
                    ),
                    format!("detail   {}", c.detail.join(", ")),
                    "resolving lets both integrations proceed to review".into(),
                ];
                (
                    c.id.clone(),
                    format!("resolve {}", c.id),
                    lines,
                    String::new(),
                )
            }
            _ => {
                self.toast = Some("that action does not apply to this card".into());
                return Vec::new();
            }
        };
        self.modal = Some(Modal {
            action,
            id,
            title,
            lines,
            input,
        });
        Vec::new()
    }

    fn modal_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        let Some(modal) = self.modal.as_mut() else {
            return Vec::new();
        };
        match key.code {
            KeyCode::Esc => self.modal = None,
            KeyCode::Backspace => {
                modal.input.pop();
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                if modal.input.chars().count() < 500 {
                    modal.input.push(c);
                }
            }
            KeyCode::Enter => {
                let rationale = modal.input.trim().to_owned();
                if rationale.is_empty() {
                    self.toast = Some("a reason is required".into());
                    return Vec::new();
                }
                let effect = match modal.action {
                    Action::Approve => Effect::Mutate {
                        command: json!({"kind": "decide_approval", "approval_id": modal.id, "approve": true, "rationale": rationale}),
                    },
                    Action::Deny => Effect::Mutate {
                        command: json!({"kind": "decide_approval", "approval_id": modal.id, "approve": false, "rationale": rationale}),
                    },
                    Action::Revoke => Effect::Mutate {
                        command: json!({"kind": "revoke_approval", "approval_id": modal.id, "rationale": rationale}),
                    },
                    Action::Resolve => Effect::Mutate {
                        command: json!({"kind": "resolve_conflict", "conflict_id": modal.id, "rationale": rationale}),
                    },
                    Action::Bind => {
                        let mut parts = rationale.split_whitespace();
                        let pane = parts.next().unwrap_or_default().to_owned();
                        let provider: Vec<&str> = parts.collect();
                        let mut command = json!({"kind": "bind_agent_pane", "actor_id": modal.id, "pane_id": pane});
                        if !provider.is_empty() {
                            command["provider"] = json!(provider.join(" "));
                        }
                        Effect::Mutate { command }
                    }
                    Action::Rename => Effect::Mutate {
                        command: json!({"kind": "rename_team", "team_id": modal.id, "name": rationale}),
                    },
                    Action::Suspend => Effect::Mutate {
                        command: json!({"kind": "suspend_team", "team_id": modal.id, "reason": rationale}),
                    },
                    Action::Goal => Effect::Propose { goal: rationale },
                };
                self.modal = None;
                return vec![effect];
            }
            _ => {}
        }
        Vec::new()
    }

    fn composer_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        match key.code {
            KeyCode::Esc => self.focus = Focus::Main,
            KeyCode::Backspace => {
                self.composer.pop();
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                if self.composer.len() < 4000 {
                    self.composer.push(c);
                }
            }
            KeyCode::Enter => {
                let body = self.composer.trim().to_owned();
                let Scope::Team(team_id) = self.scope.clone() else {
                    return Vec::new();
                };
                if body.is_empty() {
                    return Vec::new();
                }
                let audience = self.composer_audience();
                self.composer.clear();
                self.next_message += 1;
                self.scroll.insert(View::Chat, 0);
                return vec![Effect::Mutate {
                    command: json!({"kind": "append_chat", "team_id": team_id, "audience": audience,
                        "message_id": format!("ui-{}-{}", std::process::id(), self.next_message), "body": body}),
                }];
            }
            _ => {}
        }
        Vec::new()
    }

    pub fn mouse(&mut self, event: MouseEvent) -> Vec<Effect> {
        if self.modal.is_some() || self.help {
            return Vec::new();
        }
        if !matches!(event.kind, MouseEventKind::Down(MouseButton::Left)) {
            if let MouseEventKind::ScrollDown | MouseEventKind::ScrollUp = event.kind {
                self.move_cursor(if matches!(event.kind, MouseEventKind::ScrollDown) {
                    1
                } else {
                    -1
                });
            }
            return Vec::new();
        }
        let frame = self.frame();
        let at = |rect: Option<Rect>| rect.filter(|r| contains(*r, event.column, event.row));
        if let Some(nav) = at(frame.navigator) {
            self.focus = Focus::Navigator;
            let index =
                usize::from(event.row.saturating_sub(nav.y + 1)) + self.nav_offset(nav.height);
            if index < self.nav_rows().len() {
                return self.select_row(index);
            }
        } else if at(frame.inspector).is_some() {
            self.focus = Focus::Inspector;
        } else if contains(frame.main, event.column, event.row) {
            self.focus = Focus::Main;
            for (rect, view) in view_tabs(frame.main) {
                if contains(rect, event.column, event.row) {
                    self.view = view;
                }
            }
        }
        Vec::new()
    }

    /// First visible navigator row, so the cursor stays on screen.
    pub fn nav_offset(&self, height: u16) -> usize {
        let visible = usize::from(height.saturating_sub(2)).max(1);
        self.nav_cursor.saturating_sub(visible - 1)
    }
}

fn contains(rect: Rect, x: u16, y: u16) -> bool {
    x >= rect.x && x < rect.right() && y >= rect.y && y < rect.bottom()
}

/// The clickable tab strip across the top of the workspace.
pub fn view_tabs(main: Rect) -> Vec<(Rect, View)> {
    let mut x = main.x + 1;
    let mut tabs = Vec::new();
    for view in View::ALL.iter() {
        let width = (view.label().len() + 4) as u16;
        if x + width > main.right() {
            break;
        }
        tabs.push((Rect::new(x, main.y + 1, width, 1), *view));
        x += width + 1;
    }
    tabs
}

pub fn compact_json(value: &Value) -> String {
    match value {
        Value::Null => "-".into(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

const ROLE_ALIASES: [(&str, &str); 4] = [
    ("lead", "lead"),
    ("dev", "developer"),
    ("developer", "developer"),
    ("qa", "qa"),
];

impl Screen {
    /// The Effect for choosing an agent: its bound pane, when one was recorded.
    fn focus_effect(&self, actor_id: String) -> Effect {
        let pane_id = self.snapshot.binding(&actor_id).map(|b| b.pane_id.clone());
        Effect::FocusAgent { actor_id, pane_id }
    }

    /// Who the composer text is for. A leading `@lead`, `@dev`, `@qa` or `@<agent id>` addresses
    /// that role or agent; this only chooses the audience field, the server enforces it, and the
    /// text itself is never read as a command.
    pub fn composer_audience(&self) -> Value {
        let team = match &self.scope {
            Scope::Team(id) => id.as_str(),
            _ => return json!({"to": "team"}),
        };
        let Some(word) = self
            .composer
            .split_whitespace()
            .next()
            .and_then(|w| w.strip_prefix('@'))
        else {
            return json!({"to": "team"});
        };
        let word = word.trim_end_matches([',', ':', '.']);
        if let Some((_, role)) = ROLE_ALIASES.iter().find(|(alias, _)| *alias == word) {
            return json!({"to": "role", "role": role});
        }
        if word == "owner" || self.snapshot.team_agents(team).iter().any(|a| a.id == word) {
            return json!({"to": "actor", "actor_id": word});
        }
        json!({"to": "team"})
    }

    pub fn composer_audience_label(&self) -> Option<String> {
        self.snapshot.audience_label(&self.composer_audience())
    }

    pub fn view_named(name: &str) -> Option<View> {
        View::ALL.into_iter().find(|view| view.label() == name)
    }

    /// Opens on `view` across all teams, where waiting decisions are all visible.
    pub fn open_at(&mut self, view: View) {
        self.view = view;
        self.focus = Focus::Main;
        if view == View::Decisions {
            self.scope = Scope::All;
        }
    }

    pub fn palette_override(&self) -> Option<&str> {
        self.theme.as_deref()
    }

    fn cycle_theme(&mut self) {
        let names = crate::config::THEME_NAMES;
        let next = match self
            .theme
            .as_deref()
            .and_then(|t| names.iter().position(|n| *n == t))
        {
            Some(i) => (i + 1) % names.len(),
            None => 0,
        };
        self.theme = Some(names[next].to_owned());
    }

    // ---- command palette --------------------------------------------------------------------

    /// The commands that make sense right now, each with the label shown for it.
    pub fn palette_items(&self) -> Vec<(String, Intent)> {
        let mut items: Vec<(String, Intent)> = Vec::new();
        let owner = self.is_owner();
        let lead = self.snapshot.viewer.role == "lead";
        for view in View::ALL {
            items.push((format!("go to {}", view.label()), Intent::View(view)));
        }
        if self.view == View::Decisions {
            let cards = self.cards();
            match cards.get(self.cursor_of(View::Decisions)) {
                Some(Card::Approval(i)) => match tag(&self.snapshot.approvals[*i].status) {
                    "pending" => {
                        items.push((
                            "approve the selected approval".into(),
                            Intent::Decide(Action::Approve),
                        ));
                        items.push((
                            "deny the selected approval".into(),
                            Intent::Decide(Action::Deny),
                        ));
                    }
                    "approved" => items.push((
                        "revoke the selected approval".into(),
                        Intent::Decide(Action::Revoke),
                    )),
                    _ => {}
                },
                Some(Card::Conflict(_)) => items.push((
                    "resolve the selected conflict".into(),
                    Intent::Decide(Action::Resolve),
                )),
                _ => {}
            }
        }
        if self.view == View::Terminals {
            if let Some(agent) = self.agent.as_deref().or_else(|| {
                self.agent_at(self.cursor_of(View::Terminals))
                    .as_deref()
                    .map(|_| "")
            }) {
                let _ = agent;
            }
            if let Some(id) = self.selected_agent_id() {
                items.push((format!("bind a pane to {id}"), Intent::BindPane));
                if self.snapshot.binding(&id).is_some() {
                    items.push((format!("unbind the pane of {id}"), Intent::UnbindPane));
                }
            }
        }
        if let Scope::Team(id) = &self.scope {
            if let Some(team) = self.snapshot.team(id) {
                if owner || lead {
                    items.push((format!("rename team {}", team.name), Intent::RenameTeam));
                    match tag(&team.status) {
                        "active" => {
                            items.push((format!("suspend team {}", team.name), Intent::SuspendTeam))
                        }
                        "suspended" => {
                            items.push((format!("resume team {}", team.name), Intent::ResumeTeam))
                        }
                        _ => {}
                    }
                }
            }
        }
        if owner {
            items.push(("new team (manual builder)".into(), Intent::NewTeam));
            items.push(("propose a team from a goal".into(), Intent::ProposeTeam));
        }
        items.extend([
            ("toggle navigator".into(), Intent::ToggleNavigator),
            ("toggle inspector".into(), Intent::ToggleInspector),
            ("switch density".into(), Intent::Density),
            ("toggle ascii glyphs".into(), Intent::Ascii),
            ("toggle high-contrast focus".into(), Intent::Contrast),
            ("next theme (preview)".into(), Intent::CycleTheme),
            ("refresh".into(), Intent::Refresh),
            ("help".into(), Intent::Help),
            ("quit".into(), Intent::Quit),
        ]);
        items
    }

    pub fn palette_matches(&self) -> Vec<(String, Intent)> {
        let query = self
            .palette
            .as_ref()
            .map(|p| p.query.to_lowercase())
            .unwrap_or_default();
        self.palette_items()
            .into_iter()
            .filter(|(label, _)| {
                query
                    .split_whitespace()
                    .all(|word| label.to_lowercase().contains(word))
            })
            .collect()
    }

    fn palette_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        let matches = self.palette_matches();
        let Some(state) = self.palette.as_mut() else {
            return Vec::new();
        };
        match key.code {
            KeyCode::Esc => self.palette = None,
            KeyCode::Up => state.cursor = state.cursor.saturating_sub(1),
            KeyCode::Down => state.cursor = (state.cursor + 1).min(matches.len().saturating_sub(1)),
            KeyCode::Backspace => {
                state.query.pop();
                state.cursor = 0;
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                state.query.push(c);
                state.cursor = 0;
            }
            KeyCode::Enter => {
                let chosen = matches
                    .get(state.cursor.min(matches.len().saturating_sub(1)))
                    .map(|(_, i)| i.clone());
                self.palette = None;
                if let Some(intent) = chosen {
                    return self.run_intent(intent);
                }
            }
            _ => {}
        }
        Vec::new()
    }

    pub fn selected_agent_id(&self) -> Option<String> {
        if self.view == View::Terminals {
            self.agent_at(self.cursor_of(View::Terminals))
        } else {
            self.agent.clone()
        }
    }

    pub fn run_intent(&mut self, intent: Intent) -> Vec<Effect> {
        match intent {
            Intent::View(view) => {
                self.view = view;
                self.focus = Focus::Main;
            }
            Intent::ToggleNavigator => self.toggle(Area::Navigator),
            Intent::ToggleInspector => self.toggle(Area::Inspector),
            Intent::Density => {
                self.options.density = match self.options.density {
                    Density::Compact => Density::Comfortable,
                    Density::Comfortable => Density::Compact,
                };
            }
            Intent::Ascii => self.options.ascii = !self.options.ascii,
            Intent::Contrast => self.options.high_contrast = !self.options.high_contrast,
            Intent::CycleTheme => self.cycle_theme(),
            Intent::Refresh => return vec![Effect::Refresh],
            Intent::Help => self.help = true,
            Intent::Quit => return vec![Effect::Quit],
            Intent::Decide(action) => return self.open_modal(action),
            Intent::NewTeam => {
                self.builder = Some(Builder {
                    draft: Draft::blank(&self.snapshot),
                    step: BuilderStep::Edit,
                    field: 0,
                    preview: None,
                    checking: false,
                });
            }
            Intent::ProposeTeam => {
                self.modal = Some(Modal {
                    action: Action::Goal,
                    id: String::new(),
                    title: "propose a team from a goal".into(),
                    lines: vec![
                        "Describe what the team should achieve. You review and edit the".into(),
                        "proposal before anything is created.".into(),
                    ],
                    input: String::new(),
                });
            }
            Intent::BindPane => {
                let Some(id) = self.selected_agent_id() else {
                    return Vec::new();
                };
                let current = self
                    .snapshot
                    .binding(&id)
                    .map(|b| b.pane_id.clone())
                    .unwrap_or_default();
                self.modal = Some(Modal {
                    action: Action::Bind,
                    id: id.clone(),
                    title: format!("bind a pane to {id}"),
                    lines: vec![
                        "Pane id, then the provider if you want it shown: `pane-7 claude`.".into(),
                    ],
                    input: current,
                });
            }
            Intent::UnbindPane => {
                if let Some(id) = self.selected_agent_id() {
                    if self.snapshot.binding(&id).is_some() {
                        return vec![Effect::Mutate {
                            command: json!({"kind": "unbind_agent_pane", "actor_id": id}),
                        }];
                    }
                    self.toast = Some(format!("{id} has no bound pane"));
                }
            }
            Intent::RenameTeam => {
                if let Scope::Team(id) = self.scope.clone() {
                    self.modal = Some(Modal {
                        action: Action::Rename,
                        title: format!("rename {id}"),
                        lines: vec!["The team id stays the same; only its name changes.".into()],
                        input: self.snapshot.team_label(&id),
                        id,
                    });
                }
            }
            Intent::SuspendTeam => {
                if let Scope::Team(id) = self.scope.clone() {
                    self.modal = Some(Modal {
                        action: Action::Suspend,
                        title: format!("suspend {id}"),
                        lines: vec!["Agents keep running; the team cannot start or advance work until resumed.".into()],
                        input: String::new(),
                        id,
                    });
                }
            }
            Intent::ResumeTeam => {
                if let Scope::Team(id) = &self.scope {
                    return vec![Effect::Mutate {
                        command: json!({"kind": "resume_team", "team_id": id}),
                    }];
                }
            }
        }
        Vec::new()
    }

    // ---- team builder -----------------------------------------------------------------------

    /// Opens the builder on a draft that came from a proposal. It starts in edit mode: a proposal
    /// is a suggestion to review and change, never something launched.
    pub fn open_builder(&mut self, draft: Draft) {
        self.builder = Some(Builder {
            draft,
            step: BuilderStep::Edit,
            field: 0,
            preview: None,
            checking: false,
        });
    }

    /// The server's answer to `ValidateDraft`: review if the draft is valid, else say why not.
    pub fn apply_draft_check(&mut self, result: Result<Preview, String>) {
        let Some(builder) = self.builder.as_mut() else {
            return;
        };
        builder.checking = false;
        match result {
            Ok(preview) if preview.valid => {
                builder.preview = Some(preview);
                builder.step = BuilderStep::Review;
            }
            Ok(preview) => {
                self.toast = preview.problems.first().cloned();
                builder.preview = Some(preview);
            }
            Err(message) => self.toast = Some(message),
        }
    }

    fn builder_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        let projects: Vec<String> = self
            .snapshot
            .projects
            .iter()
            .map(|p| p.id.clone())
            .collect();
        let Some(builder) = self.builder.as_mut() else {
            return Vec::new();
        };
        match builder.step {
            BuilderStep::Review => match key.code {
                KeyCode::Enter => {
                    let draft = builder.draft.clone();
                    self.builder = None;
                    return vec![Effect::LaunchDraft { draft }];
                }
                KeyCode::Esc | KeyCode::Char('e') => builder.step = BuilderStep::Edit,
                _ => {}
            },
            BuilderStep::Edit => {
                let count = BUILDER_FIELDS.len();
                match key.code {
                    KeyCode::Esc => self.builder = None,
                    KeyCode::Tab | KeyCode::Down => builder.field = (builder.field + 1) % count,
                    KeyCode::BackTab | KeyCode::Up => {
                        builder.field = (builder.field + count - 1) % count
                    }
                    KeyCode::Left | KeyCode::Right if builder.field == 1 => {
                        let mut options = projects;
                        options.insert(0, String::new());
                        let at = options
                            .iter()
                            .position(|p| *p == builder.draft.project_id)
                            .unwrap_or(0);
                        let forward = key.code == KeyCode::Right;
                        let next =
                            (at + if forward { 1 } else { options.len() - 1 }) % options.len();
                        builder.draft.project_id = options[next].clone();
                    }
                    KeyCode::Backspace => {
                        builder.preview = None;
                        if builder.field == 0 {
                            let mut name = builder.draft.name.clone();
                            name.pop();
                            builder.draft.set_name(&name, &self.snapshot);
                        } else if let Some(text) =
                            Self::draft_text(&mut builder.draft, builder.field)
                        {
                            text.pop();
                            builder.draft.auto_ids = false;
                        }
                    }
                    KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                        builder.preview = None;
                        if builder.field == 0 {
                            if builder.draft.name.len() < 128 {
                                let name = format!("{}{c}", builder.draft.name);
                                builder.draft.set_name(&name, &self.snapshot);
                            }
                        } else if let Some(text) =
                            Self::draft_text(&mut builder.draft, builder.field)
                        {
                            if text.len() < 128 {
                                text.push(c);
                                builder.draft.auto_ids = false;
                            }
                        }
                    }
                    KeyCode::Enter => {
                        if builder.field + 1 < count {
                            builder.field += 1;
                        } else {
                            // Only the Enter pressed on the last field asks the server to check it.
                            if !builder.checking {
                                builder.checking = true;
                                return vec![Effect::ValidateDraft {
                                    draft: builder.draft.clone(),
                                }];
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        Vec::new()
    }

    fn draft_text(draft: &mut Draft, field: usize) -> Option<&mut String> {
        match field {
            0 => Some(&mut draft.name),
            2 => Some(&mut draft.lead),
            3 => Some(&mut draft.developer),
            4 => Some(&mut draft.qa),
            _ => None,
        }
    }
}
