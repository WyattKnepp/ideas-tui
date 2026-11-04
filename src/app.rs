//! Application state and event handling. Rendering lives in `ui.rs`; HTTP in
//! `api.rs`. Every network call runs on a spawned task and reports back as a
//! [`Msg`], so the UI never blocks on the server.

use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::style::Style;
use ratatui::widgets::ListState;
use ratatui_textarea::{CursorMove, Input, TextArea};
use tokio::sync::mpsc::UnboundedSender;

use crate::api::{ApiResult, Client};
use crate::state::*;
use crate::types::*;

pub const LIST_REFRESH: Duration = Duration::from_secs(10);
pub const DETAIL_REFRESH: Duration = Duration::from_millis(2500);
const INFO_TTL: Duration = Duration::from_secs(5);
const ERROR_TTL: Duration = Duration::from_secs(12);

pub enum Msg {
    Key(KeyEvent),
    Tick,
    Redraw,
    Ideas { seq: u64, result: ApiResult<Vec<Idea>> },
    Fetched { seq: u64, id: String, result: ApiResult<Idea> },
    Projects(ApiResult<Vec<String>>),
    Mutated { id: String, what: &'static str, result: ApiResult<Idea> },
    Created(ApiResult<Idea>),
    Deleted { id: String, result: ApiResult<()> },
    Elaborated { id: String, qid: String, result: ApiResult<String> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    List,
    Detail,
}

pub struct Status {
    pub text: String,
    pub error: bool,
    pub until: Instant,
}

pub struct AnswerModal {
    pub id: String,
    /// `at` of the Claude message these answers belong to.
    pub turn_at: Option<String>,
    pub comment: String,
    pub form: TurnForm,
    /// Typing into the "Other" box of the row under the cursor.
    pub editing: bool,
    pub elab: HashMap<String, String>,
    pub elab_pending: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HField {
    Title,
    Project,
    Visibility,
    Brief,
}

impl HField {
    const ORDER: [HField; 4] = [HField::Title, HField::Project, HField::Visibility, HField::Brief];
    fn step(self, forward: bool) -> HField {
        let i = Self::ORDER.iter().position(|f| *f == self).unwrap_or(0);
        Self::ORDER[if forward { (i + 1) % 4 } else { (i + 3) % 4 }]
    }
}

pub struct HandoffEditor {
    pub id: String,
    pub title: TextArea<'static>,
    pub project: TextArea<'static>,
    pub public: bool,
    pub brief: TextArea<'static>,
    pub focus: HField,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dest {
    Laptop,
    Droplet,
}

pub struct SendDialog {
    pub id: String,
    pub handoff: Handoff,
    pub new_project: bool,
    pub dest: Dest,
    pub live: bool,
}

#[derive(Debug, Clone)]
pub enum ConfirmAction {
    Send { id: String, req: SendRequest },
    CancelGrill(String),
    Delete(String),
    Job { id: String, action: JobAction },
}

pub struct Confirm {
    pub prompt: String,
    pub action: ConfirmAction,
}

pub enum Modal {
    None,
    Help,
    Capture(Box<TextArea<'static>>),
    Reply { id: String, area: Box<TextArea<'static>> },
    Answer(Box<AnswerModal>),
    Handoff(Box<HandoffEditor>),
    Send(SendDialog),
    Confirm(Confirm),
}

#[derive(Default)]
pub struct DetailView {
    pub scroll: usize,
    pub show_log: bool,
    /// Scroll to the grill thread the first time the idea is drawn.
    pub jump_to_thread: bool,
}

pub struct App {
    client: Arc<Client>,
    tx: UnboundedSender<Msg>,
    pub screen: Screen,
    pub modal: Modal,
    pub ideas: Vec<Idea>,
    pub list_state: ListState,
    pub filter: Filter,
    pub current: Option<Idea>,
    pub detail: DetailView,
    pub projects: Vec<String>,
    pub projects_loaded: bool,
    pub status: Option<Status>,
    /// Ideas with a request in flight, and what we're waiting for.
    pub pending: HashMap<String, &'static str>,
    pub capture_busy: bool,
    pub loaded: bool,
    pub tick: u64,
    pub should_quit: bool,
    /// Unsent handoff edits, per idea. The server has no endpoint for saving
    /// them on their own: they go out with Send.
    pub drafts: HashMap<String, Handoff>,
    saved_answers: HashMap<String, Box<AnswerModal>>,
    last_list: Instant,
    last_detail: Instant,
    list_inflight: bool,
    detail_inflight: bool,
    /// Bumped on every mutation result; older fetches are dropped.
    mut_seq: u64,
}

fn text_area(lines: Vec<String>) -> TextArea<'static> {
    let mut ta = TextArea::new(lines);
    ta.set_cursor_line_style(Style::default());
    ta
}

/// A one-line field with the cursor after the existing text.
fn line_field(s: &str) -> TextArea<'static> {
    let mut ta = text_area(vec![s.to_string()]);
    ta.move_cursor(CursorMove::End);
    ta
}

fn lines_of(s: &str) -> Vec<String> {
    s.split('\n').map(str::to_string).collect()
}

impl App {
    pub fn new(client: Client, tx: UnboundedSender<Msg>) -> App {
        App {
            client: Arc::new(client),
            tx,
            screen: Screen::List,
            modal: Modal::None,
            ideas: vec![],
            list_state: ListState::default(),
            filter: Filter::Open,
            current: None,
            detail: DetailView::default(),
            projects: vec![],
            projects_loaded: false,
            status: None,
            pending: HashMap::new(),
            capture_busy: false,
            loaded: false,
            tick: 0,
            should_quit: false,
            drafts: HashMap::new(),
            saved_answers: HashMap::new(),
            last_list: Instant::now(),
            last_detail: Instant::now(),
            list_inflight: false,
            detail_inflight: false,
            mut_seq: 0,
        }
    }

    /// Kicks off the first loads.
    pub fn start(&mut self) {
        self.refresh_list();
        self.fetch_projects();
    }

    // ---------- helpers ----------

    fn spawn<F: Future<Output = Msg> + Send + 'static>(&self, f: F) {
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let _ = tx.send(f.await);
        });
    }

    pub fn info(&mut self, text: impl Into<String>) {
        self.status = Some(Status { text: text.into(), error: false, until: Instant::now() + INFO_TTL });
    }

    pub fn error(&mut self, text: impl Into<String>) {
        self.status = Some(Status { text: text.into(), error: true, until: Instant::now() + ERROR_TTL });
    }

    pub fn spinner(&self) -> char {
        const FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
        FRAMES[(self.tick as usize) % FRAMES.len()]
    }

    pub fn visible(&self) -> Vec<&Idea> {
        filtered(&self.ideas, self.filter)
    }

    pub fn selected_idea(&self) -> Option<&Idea> {
        self.list_state.selected().and_then(|i| self.visible().get(i).copied())
    }

    /// What the idea is waiting on locally (request in flight) or on the server.
    pub fn busy_label(&self, idea: &Idea) -> Option<&'static str> {
        self.pending.get(&idea.id).copied().or(idea.busy.then_some("Claude is thinking…"))
    }

    pub fn is_new_project(&self, h: &Handoff) -> bool {
        if self.projects_loaded { !self.projects.contains(&h.project) } else { h.is_new }
    }

    /// The handoff as it would be sent: local edits win over the server copy.
    pub fn effective_handoff(&self, idea: &Idea) -> Option<Handoff> {
        self.drafts.get(&idea.id).cloned().or_else(|| idea.handoff.clone())
    }

    fn clamp_selection(&mut self, keep: Option<&str>) {
        let vis: Vec<String> = self.visible().iter().map(|i| i.id.clone()).collect();
        let sel = match keep.and_then(|id| vis.iter().position(|v| v == id)) {
            Some(p) => Some(p),
            None if vis.is_empty() => None,
            None => Some(self.list_state.selected().unwrap_or(0).min(vis.len() - 1)),
        };
        self.list_state.select(sel);
    }

    fn apply_idea(&mut self, idea: Idea) {
        let keep = self.selected_idea().map(|i| i.id.clone());
        match self.ideas.iter_mut().find(|i| i.id == idea.id) {
            Some(slot) => *slot = idea.clone(),
            None => self.ideas.push(idea.clone()),
        }
        sort_ideas(&mut self.ideas);
        if self.current.as_ref().is_some_and(|c| c.id == idea.id) {
            self.current = Some(idea);
        }
        self.clamp_selection(keep.as_deref());
    }

    // ---------- requests ----------

    pub fn refresh_list(&mut self) {
        if self.list_inflight {
            return;
        }
        self.list_inflight = true;
        self.last_list = Instant::now();
        let (c, seq) = (self.client.clone(), self.mut_seq);
        self.spawn(async move { Msg::Ideas { seq, result: c.list_ideas().await } });
    }

    fn fetch_current(&mut self) {
        let Some(id) = self.current.as_ref().map(|c| c.id.clone()) else { return };
        if self.detail_inflight {
            return;
        }
        self.detail_inflight = true;
        self.last_detail = Instant::now();
        let (c, seq) = (self.client.clone(), self.mut_seq);
        self.spawn(async move { Msg::Fetched { seq, id: id.clone(), result: c.get_idea(&id).await } });
    }

    fn fetch_projects(&self) {
        let c = self.client.clone();
        self.spawn(async move { Msg::Projects(c.projects().await) });
    }

    /// Runs a request that returns the updated idea, marking it busy meanwhile.
    fn run_idea<Fut>(&mut self, id: &str, what: &'static str, busy: &'static str, fut: Fut)
    where
        Fut: Future<Output = ApiResult<Idea>> + Send + 'static,
    {
        if self.pending.contains_key(id) {
            self.error("Still waiting on the previous request for this idea");
            return;
        }
        self.pending.insert(id.to_string(), busy);
        let id = id.to_string();
        self.spawn(async move { Msg::Mutated { id, what, result: fut.await } });
    }

    // ---------- message handling ----------

    pub fn update(&mut self, msg: Msg) {
        match msg {
            Msg::Key(k) => self.on_key(k),
            Msg::Tick => self.on_tick(),
            Msg::Redraw => {}
            Msg::Ideas { seq, result } => {
                self.list_inflight = false;
                self.loaded = true;
                match result {
                    Ok(list) if seq >= self.mut_seq => {
                        let keep = self.selected_idea().map(|i| i.id.clone());
                        self.ideas = list;
                        sort_ideas(&mut self.ideas);
                        if let Some(cur) = self.current.as_ref().map(|c| c.id.clone())
                            && let Some(fresh) = self.ideas.iter().find(|i| i.id == cur)
                        {
                            self.current = Some(fresh.clone());
                        }
                        self.clamp_selection(keep.as_deref());
                    }
                    Ok(_) => {}
                    Err(e) => self.error(e.to_string()),
                }
            }
            Msg::Fetched { seq, id, result } => {
                self.detail_inflight = false;
                match result {
                    Ok(idea) if seq >= self.mut_seq => self.apply_idea(idea),
                    Ok(_) => {}
                    Err(e) => {
                        let gone = e.status() == Some(404);
                        self.error(e.to_string());
                        if gone && self.current.as_ref().is_some_and(|c| c.id == id) {
                            self.close_detail();
                            self.refresh_list();
                        }
                    }
                }
            }
            Msg::Projects(result) => match result {
                Ok(p) => {
                    self.projects = p;
                    self.projects_loaded = true;
                }
                Err(e) => self.error(format!("Couldn't load projects: {e}")),
            },
            Msg::Mutated { id, what, result } => {
                self.pending.remove(&id);
                self.mut_seq += 1;
                match result {
                    Ok(idea) => {
                        match what {
                            "send" => {
                                self.drafts.remove(&id);
                                let to = if is_droplet(&idea) { "droplet" } else { "laptop" };
                                self.info(format!("Sent to the {to}"));
                            }
                            "handoff" => {
                                self.drafts.remove(&id);
                                self.info("Handoff written — e to edit, s to send");
                            }
                            "grill" => {
                                let msg = if grill_done(&idea) {
                                    "Claude has enough — w to write the handoff"
                                } else if open_turn(&idea).is_some() {
                                    "Claude replied — a to answer"
                                } else {
                                    "Claude replied — t to reply"
                                };
                                self.info(msg);
                            }
                            "cancel-grill" => {
                                self.saved_answers.remove(&id);
                                self.drafts.remove(&id);
                                self.info("Grill cancelled; back in the inbox");
                            }
                            "archive" => self.info(if idea.status == "archived" { "Archived" } else { "Restored" }),
                            "unsend" => self.info("Pulled back for editing"),
                            "job" => self.info("Requested; the runner will pick it up"),
                            _ => {}
                        }
                        self.apply_idea(idea);
                    }
                    Err(e) => {
                        self.error(e.to_string());
                        // Our view may be stale (409): reload.
                        if matches!(e.status(), Some(404 | 409)) {
                            self.refresh_list();
                            self.fetch_current();
                        }
                    }
                }
            }
            Msg::Created(result) => {
                self.capture_busy = false;
                self.mut_seq += 1;
                match result {
                    Ok(idea) => {
                        self.modal = Modal::None;
                        let id = idea.id.clone();
                        self.apply_idea(idea);
                        if !self.filter.matches(&self.ideas.iter().find(|i| i.id == id).cloned().unwrap_or_default()) {
                            self.filter = Filter::Open;
                        }
                        self.clamp_selection(Some(&id));
                        self.info("Captured in the inbox");
                    }
                    // The modal stays open with the text intact.
                    Err(e) => self.error(format!("Not saved: {e}")),
                }
            }
            Msg::Deleted { id, result } => {
                self.pending.remove(&id);
                self.mut_seq += 1;
                match result {
                    Ok(()) => {
                        let keep = self.selected_idea().map(|i| i.id.clone()).filter(|k| *k != id);
                        self.ideas.retain(|i| i.id != id);
                        self.drafts.remove(&id);
                        self.saved_answers.remove(&id);
                        if self.current.as_ref().is_some_and(|c| c.id == id) {
                            self.close_detail();
                        }
                        self.clamp_selection(keep.as_deref());
                        self.info("Deleted");
                    }
                    Err(e) => {
                        self.error(e.to_string());
                        if matches!(e.status(), Some(404 | 409)) {
                            self.refresh_list();
                            self.fetch_current();
                        }
                    }
                }
            }
            Msg::Elaborated { id, qid, result } => {
                if let Modal::Answer(a) = &mut self.modal
                    && a.id == id
                {
                    a.elab_pending.retain(|q| *q != qid);
                    match result {
                        Ok(t) => {
                            a.elab.insert(qid, t);
                        }
                        Err(e) => self.error(format!("Couldn't explain that: {e}")),
                    }
                }
            }
        }
    }

    fn on_tick(&mut self) {
        self.tick = self.tick.wrapping_add(1);
        if self.status.as_ref().is_some_and(|s| Instant::now() >= s.until) {
            self.status = None;
        }
        if self.last_list.elapsed() >= LIST_REFRESH {
            self.refresh_list();
        }
        if self.screen == Screen::Detail
            && self.current.as_ref().is_some_and(needs_fast_poll)
            && self.last_detail.elapsed() >= DETAIL_REFRESH
        {
            self.fetch_current();
        }
    }

    // ---------- navigation ----------

    fn open_selected(&mut self) {
        let Some(idea) = self.selected_idea().cloned() else { return };
        self.open_idea(idea);
    }

    pub fn open_idea(&mut self, idea: Idea) {
        self.detail = DetailView { scroll: 0, show_log: false, jump_to_thread: idea.status == "grilling" };
        self.current = Some(idea);
        self.screen = Screen::Detail;
        self.detail_inflight = false;
        self.fetch_current();
        self.fetch_projects();
    }

    fn close_detail(&mut self) {
        self.screen = Screen::List;
        self.current = None;
        self.modal = Modal::None;
    }

    // ---------- keys ----------

    pub fn on_key(&mut self, key: KeyEvent) {
        if key.kind == KeyEventKind::Release {
            return;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }
        // A fresh keypress dismisses a lingering error.
        if self.status.as_ref().is_some_and(|s| s.error) && !matches!(self.modal, Modal::None) {
            // keep errors visible while a dialog is open (e.g. a failed capture)
        } else {
            self.status = None;
        }
        match std::mem::replace(&mut self.modal, Modal::None) {
            Modal::None => match self.screen {
                Screen::List => self.key_list(key),
                Screen::Detail => self.key_detail(key),
            },
            Modal::Help => {}
            Modal::Capture(ta) => self.key_capture(ta, key),
            Modal::Reply { id, area } => self.key_reply(id, area, key),
            Modal::Answer(a) => self.key_answer(a, key),
            Modal::Handoff(h) => self.key_handoff(h, key),
            Modal::Send(s) => self.key_send(s, key),
            Modal::Confirm(c) => self.key_confirm(c, key),
        }
    }

    fn key_list(&mut self, key: KeyEvent) {
        let n = self.visible().len();
        let sel = self.list_state.selected().unwrap_or(0);
        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('j') | KeyCode::Down if n > 0 => self.list_state.select(Some((sel + 1).min(n - 1))),
            KeyCode::Char('k') | KeyCode::Up if n > 0 => self.list_state.select(Some(sel.saturating_sub(1))),
            KeyCode::Char('g') | KeyCode::Home if n > 0 => self.list_state.select(Some(0)),
            KeyCode::Char('G') | KeyCode::End if n > 0 => self.list_state.select(Some(n - 1)),
            KeyCode::PageDown if n > 0 => self.list_state.select(Some((sel + 10).min(n - 1))),
            KeyCode::PageUp if n > 0 => self.list_state.select(Some(sel.saturating_sub(10))),
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => self.open_selected(),
            KeyCode::Char('n') => self.modal = Modal::Capture(Box::new(text_area(vec![String::new()]))),
            KeyCode::Char('r') => {
                self.list_inflight = false;
                self.refresh_list();
                self.info("Refreshing…");
            }
            KeyCode::Tab => self.set_filter(self.filter.next(true)),
            KeyCode::BackTab => self.set_filter(self.filter.next(false)),
            KeyCode::Char('A') => {
                self.set_filter(if self.filter == Filter::Archived { Filter::Open } else { Filter::Archived })
            }
            KeyCode::Char('?') => self.modal = Modal::Help,
            _ => {}
        }
    }

    fn set_filter(&mut self, f: Filter) {
        self.filter = f;
        self.list_state.select(None);
        self.clamp_selection(None);
    }

    fn key_capture(&mut self, mut ta: Box<TextArea<'static>>, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => return,
            KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if self.capture_busy {
                    self.modal = Modal::Capture(ta);
                    return;
                }
                let text = ta.lines().join("\n").trim().to_string();
                if text.is_empty() {
                    self.error("Idea is empty");
                } else {
                    self.capture_busy = true;
                    let c = self.client.clone();
                    self.spawn(async move { Msg::Created(c.create_idea(&text).await) });
                }
            }
            _ => {
                ta.input(Input::from(key));
            }
        }
        self.modal = Modal::Capture(ta);
    }

    fn key_reply(&mut self, id: String, mut ta: Box<TextArea<'static>>, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => return,
            KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                let text = ta.lines().join("\n").trim().to_string();
                if text.is_empty() {
                    self.error("Reply is empty");
                } else if self.pending.contains_key(&id) {
                    self.error("Still waiting on Claude");
                } else {
                    let c = self.client.clone();
                    let i = id.clone();
                    self.run_idea(&id, "grill", "Claude is thinking…", async move { c.grill(&i, &text).await });
                    return;
                }
            }
            _ => {
                ta.input(Input::from(key));
            }
        }
        self.modal = Modal::Reply { id, area: ta };
    }

    // ---------- answering a structured turn ----------

    fn open_answer(&mut self, idea: &Idea) {
        let Some(turn) = open_turn(idea) else { return };
        let at = turn.at.clone();
        let modal = match self.saved_answers.remove(&idea.id) {
            Some(saved) if saved.turn_at == at => saved,
            _ => Box::new(AnswerModal {
                id: idea.id.clone(),
                turn_at: at,
                comment: turn.text.clone(),
                form: TurnForm::new(turn.questions.clone().unwrap_or_default()),
                editing: false,
                elab: HashMap::new(),
                elab_pending: vec![],
            }),
        };
        self.modal = Modal::Answer(modal);
    }

    fn key_answer(&mut self, mut a: Box<AnswerModal>, key: KeyEvent) {
        if a.editing {
            match key.code {
                KeyCode::Esc | KeyCode::Enter => a.editing = false,
                KeyCode::Backspace => {
                    if let Some(t) = a.form.other_text_mut() {
                        t.pop();
                    }
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    if let Some(t) = a.form.other_text_mut() {
                        t.clear();
                    }
                }
                KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    a.editing = false;
                    return self.submit_answers(a);
                }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    if let Some(t) = a.form.other_text_mut() {
                        t.push(c);
                    }
                }
                _ => {}
            }
            self.modal = Modal::Answer(a);
            return;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.saved_answers.insert(a.id.clone(), a);
                return;
            }
            KeyCode::Char('j') | KeyCode::Down => a.form.move_cursor(1),
            KeyCode::Char('k') | KeyCode::Up => a.form.move_cursor(-1),
            KeyCode::Tab => a.form.move_question(true),
            KeyCode::BackTab => a.form.move_question(false),
            KeyCode::Char(' ') | KeyCode::Enter => {
                if a.form.toggle_current() {
                    a.editing = true;
                }
            }
            KeyCode::Char('i') | KeyCode::Char('o') => {
                if a.form.current().is_some_and(|r| r.opt.is_none()) {
                    let row = a.form.current().unwrap();
                    a.form.answers[row.q].other = true;
                    if !a.form.questions[row.q].is_multi() {
                        a.form.answers[row.q].sel.clear();
                    }
                    a.editing = true;
                }
            }
            KeyCode::Char('e') => {
                if let Some(row) = a.form.current() {
                    let qid = a.form.questions[row.q].id.clone();
                    if !a.elab.contains_key(&qid) && !a.elab_pending.contains(&qid) {
                        a.elab_pending.push(qid.clone());
                        let (c, id) = (self.client.clone(), a.id.clone());
                        self.spawn(async move {
                            let result = c.elaborate(&id, &qid).await;
                            Msg::Elaborated { id, qid, result }
                        });
                    }
                }
            }
            KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => return self.submit_answers(a),
            KeyCode::Char('?') => {}
            _ => {}
        }
        self.modal = Modal::Answer(a);
    }

    fn submit_answers(&mut self, a: Box<AnswerModal>) {
        if !a.form.all_answered() {
            let msg = format!("Answer every question first ({} of {})", a.form.answered_count(), a.form.questions.len());
            self.error(msg);
            self.modal = Modal::Answer(a);
            return;
        }
        let message = a.form.compose();
        let (c, id) = (self.client.clone(), a.id.clone());
        let i = id.clone();
        self.run_idea(&id, "grill", "Claude is thinking…", async move { c.grill(&i, &message).await });
        if self.pending.contains_key(&id) {
            self.saved_answers.remove(&id);
        } else {
            self.modal = Modal::Answer(a);
        }
    }

    // ---------- handoff editor ----------

    fn open_handoff_editor(&mut self, idea: &Idea) {
        let Some(h) = self.effective_handoff(idea) else { return };
        self.modal = Modal::Handoff(Box::new(HandoffEditor {
            id: idea.id.clone(),
            title: line_field(&h.title),
            project: line_field(&h.project),
            public: h.visibility == "public",
            brief: text_area(lines_of(&h.brief)),
            focus: HField::Title,
        }));
    }

    fn key_handoff(&mut self, mut h: Box<HandoffEditor>, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => {
                self.info("Edits discarded");
                return;
            }
            KeyCode::Char('s') if ctrl => {
                let draft = Handoff {
                    title: h.title.lines().join(" ").trim().to_string(),
                    project: h.project.lines().join("").trim().to_string(),
                    is_new: false,
                    visibility: if h.public { "public" } else { "private" }.to_string(),
                    brief: h.brief.lines().join("\n"),
                };
                self.drafts.insert(h.id.clone(), draft);
                self.info("Edits kept — s to send them");
                return;
            }
            KeyCode::Tab => h.focus = h.focus.step(true),
            KeyCode::BackTab => h.focus = h.focus.step(false),
            _ => match h.focus {
                HField::Title => match key.code {
                    KeyCode::Enter => h.focus = h.focus.step(true),
                    _ => {
                        h.title.input(Input::from(key));
                    }
                },
                HField::Project => match key.code {
                    KeyCode::Enter => h.focus = h.focus.step(true),
                    KeyCode::Char('n' | 'p') if ctrl => {
                        let cur = h.project.lines().join("");
                        let forward = key.code == KeyCode::Char('n');
                        if !self.projects.is_empty() {
                            let n = self.projects.len();
                            let next = match self.projects.iter().position(|p| *p == cur.trim()) {
                                Some(i) => (if forward { i + 1 } else { i + n - 1 }) % n,
                                None => 0,
                            };
                            h.project = line_field(&self.projects[next]);
                        }
                    }
                    _ => {
                        h.project.input(Input::from(key));
                    }
                },
                HField::Visibility => {
                    if matches!(key.code, KeyCode::Char(' ' | 'h' | 'l') | KeyCode::Enter | KeyCode::Left | KeyCode::Right) {
                        h.public = !h.public;
                    }
                }
                HField::Brief => {
                    h.brief.input(Input::from(key));
                }
            },
        }
        self.modal = Modal::Handoff(h);
    }

    // ---------- send ----------

    fn open_send(&mut self, idea: &Idea) {
        let Some(h) = self.effective_handoff(idea) else { return };
        if !valid_project_name(h.project.trim()) {
            self.error("Project must be a plain folder name (letters, digits, . _ -); press e to edit");
            return;
        }
        if h.brief.trim().is_empty() {
            self.error("The brief is empty; press e to edit");
            return;
        }
        let new_project = self.is_new_project(&h);
        let droplet = is_droplet(idea) && !new_project;
        let live = idea.dispatch.as_ref().and_then(|d| d.mode.as_deref()) == Some("live");
        self.modal = Modal::Send(SendDialog {
            id: idea.id.clone(),
            handoff: h,
            new_project,
            dest: if droplet { Dest::Droplet } else { Dest::Laptop },
            live,
        });
    }

    fn key_send(&mut self, mut s: SendDialog, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => return,
            KeyCode::Char('1') | KeyCode::Char('l') => s.dest = Dest::Laptop,
            KeyCode::Char('2') | KeyCode::Char('d') => {
                if s.new_project {
                    self.error("The droplet only builds existing projects; new ones go to the laptop");
                } else {
                    s.dest = Dest::Droplet;
                }
            }
            KeyCode::Char('p') => s.live = false,
            KeyCode::Char('v') => s.live = true,
            KeyCode::Tab | KeyCode::Left | KeyCode::Right => {
                if s.dest == Dest::Droplet {
                    s.live = !s.live;
                }
            }
            KeyCode::Enter => {
                let h = &s.handoff;
                let (dest_s, mode) = match s.dest {
                    Dest::Laptop => ("laptop", None),
                    Dest::Droplet => ("droplet", Some(if s.live { "live" } else { "preview" })),
                };
                let req = SendRequest {
                    title: h.title.clone(),
                    project: h.project.trim().to_string(),
                    visibility: if h.visibility == "public" { "public" } else { "private" }.to_string(),
                    brief: h.brief.clone(),
                    destination: dest_s.to_string(),
                    mode: mode.map(str::to_string),
                };
                let where_ = match mode {
                    None => "your laptop".to_string(),
                    Some("live") => "the droplet and go LIVE".to_string(),
                    Some(_) => "the droplet as a preview".to_string(),
                };
                let new = if s.new_project { " (new project)" } else { "" };
                self.modal = Modal::Confirm(Confirm {
                    prompt: format!("Send \"{}\" to {}{}, for {}?", req.title, req.project, new, where_),
                    action: ConfirmAction::Send { id: s.id.clone(), req },
                });
                return;
            }
            _ => {}
        }
        self.modal = Modal::Send(s);
    }

    // ---------- confirm ----------

    fn key_confirm(&mut self, c: Confirm, key: KeyEvent) {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => self.perform(c.action),
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc | KeyCode::Char('q') => {}
            _ => self.modal = Modal::Confirm(c),
        }
    }

    fn perform(&mut self, action: ConfirmAction) {
        let c = self.client.clone();
        match action {
            ConfirmAction::Send { id, req } => {
                let i = id.clone();
                self.run_idea(&id, "send", "Sending…", async move { c.send(&i, &req).await });
            }
            ConfirmAction::CancelGrill(id) => {
                let i = id.clone();
                self.run_idea(&id, "cancel-grill", "Cancelling…", async move { c.cancel_grill(&i).await });
            }
            ConfirmAction::Job { id, action } => {
                let i = id.clone();
                self.run_idea(&id, "job", "Requesting…", async move { c.job_action(&i, action).await });
            }
            ConfirmAction::Delete(id) => {
                if self.pending.contains_key(&id) {
                    return;
                }
                self.pending.insert(id.clone(), "Deleting…");
                self.spawn(async move {
                    let result = c.delete_idea(&id).await;
                    Msg::Deleted { id, result }
                });
            }
        }
    }

    // ---------- detail screen ----------

    fn key_detail(&mut self, key: KeyEvent) {
        let Some(idea) = self.current.clone() else {
            self.screen = Screen::List;
            return;
        };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('h') | KeyCode::Left => {
                self.close_detail();
                self.refresh_list();
            }
            KeyCode::Char('j') | KeyCode::Down => self.detail.scroll += 1,
            KeyCode::Char('k') | KeyCode::Up => self.detail.scroll = self.detail.scroll.saturating_sub(1),
            KeyCode::Char('d') if ctrl => self.detail.scroll += 10,
            KeyCode::Char('u') if ctrl => self.detail.scroll = self.detail.scroll.saturating_sub(10),
            KeyCode::PageDown | KeyCode::Char(' ') => self.detail.scroll += 10,
            KeyCode::PageUp => self.detail.scroll = self.detail.scroll.saturating_sub(10),
            KeyCode::Home => self.detail.scroll = 0,
            KeyCode::End | KeyCode::Char('G') => self.detail.scroll = usize::MAX / 2,
            KeyCode::Char('?') => self.modal = Modal::Help,
            KeyCode::Char('r') => {
                self.detail_inflight = false;
                self.fetch_current();
                self.info("Refreshing…");
            }
            KeyCode::Char('l') => self.detail.show_log = !self.detail.show_log,
            KeyCode::Char('g') => {
                if can_start_grill(&idea) {
                    let (c, id) = (self.client.clone(), idea.id.clone());
                    self.run_idea(&idea.id, "grill", "Claude is thinking…", async move { c.grill(&id, "").await });
                } else {
                    self.error("Only an untouched inbox idea can be grilled");
                }
            }
            KeyCode::Char('a') => {
                if self.busy_label(&idea).is_some() {
                    self.error("Wait for Claude to finish first");
                } else if can_chat(&idea) && open_turn(&idea).is_some() {
                    self.open_answer(&idea);
                } else if can_chat(&idea) {
                    self.modal = Modal::Reply { id: idea.id.clone(), area: Box::new(text_area(vec![String::new()])) };
                } else {
                    self.error("Nothing to answer right now");
                }
            }
            KeyCode::Char('t') => {
                if self.busy_label(&idea).is_none() && can_chat(&idea) {
                    self.modal = Modal::Reply { id: idea.id.clone(), area: Box::new(text_area(vec![String::new()])) };
                } else {
                    self.error("Can't reply to this idea right now");
                }
            }
            KeyCode::Char('w') => {
                if can_write_handoff(&idea) && self.busy_label(&idea).is_none() {
                    let (c, id) = (self.client.clone(), idea.id.clone());
                    self.run_idea(&idea.id, "handoff", "Claude is writing the handoff…", async move {
                        c.write_handoff(&id).await
                    });
                } else {
                    self.error("Grill the idea first, then write the handoff");
                }
            }
            KeyCode::Char('e') => {
                if can_edit_handoff(&idea) {
                    self.open_handoff_editor(&idea);
                } else {
                    self.error("There is no editable handoff");
                }
            }
            KeyCode::Char('s') => {
                if can_send(&idea) && self.busy_label(&idea).is_none() {
                    self.open_send(&idea);
                } else {
                    self.error("There is no handoff to send");
                }
            }
            KeyCode::Char('C') => {
                if can_cancel_grill(&idea) {
                    self.modal = Modal::Confirm(Confirm {
                        prompt: "Cancel the grill? The conversation is discarded and the idea goes back to the inbox.".into(),
                        action: ConfirmAction::CancelGrill(idea.id.clone()),
                    });
                } else {
                    self.error("This idea isn't being grilled");
                }
            }
            KeyCode::Char('z') => {
                if can_archive(&idea) || idea.status == "archived" {
                    let (c, id) = (self.client.clone(), idea.id.clone());
                    let archive = idea.status != "archived";
                    self.run_idea(&idea.id, "archive", "Updating…", async move { c.set_archived(&id, archive).await });
                } else {
                    self.error("Can't archive an idea that is launching");
                }
            }
            KeyCode::Char('D') => {
                if can_delete(&idea) {
                    self.modal = Modal::Confirm(Confirm {
                        prompt: "Delete this idea permanently?".into(),
                        action: ConfirmAction::Delete(idea.id.clone()),
                    });
                } else {
                    self.error(format!("Can't delete an idea that is {}; archive it first", idea.status));
                }
            }
            KeyCode::Char('u') => {
                if can_unsend(&idea) {
                    let (c, id) = (self.client.clone(), idea.id.clone());
                    self.run_idea(&idea.id, "unsend", "Pulling back…", async move { c.unsend(&id).await });
                } else {
                    self.error("Only a queued idea can be pulled back");
                }
            }
            KeyCode::Char(ch @ ('S' | 'X' | 'B')) => {
                let want = match ch {
                    'S' => JobAction::Ship,
                    'X' => JobAction::Discard,
                    _ => JobAction::Rollback,
                };
                match idea.job.as_ref().map(job_actions) {
                    Some(av) if av.contains(&want) => {
                        let prompt = match want {
                            JobAction::Ship => "Ship the preview live?",
                            JobAction::Discard => "Discard this droplet change?",
                            JobAction::Rollback => "Roll back the live change?",
                        };
                        self.modal = Modal::Confirm(Confirm {
                            prompt: prompt.into(),
                            action: ConfirmAction::Job { id: idea.id.clone(), action: want },
                        });
                    }
                    _ => self.error(format!("{} isn't available for this job right now", want.as_str())),
                }
            }
            _ => {}
        }
    }
}

/// Short sentence on what to do next with an idea.
pub fn next_step(idea: &Idea) -> String {
    match idea.status.as_str() {
        "inbox" if idea.busy => "Claude is reading the idea…".into(),
        "inbox" => "Press g to have Claude grill this idea.".into(),
        "grilling" if grill_done(idea) => "Claude has enough. Press w to write the handoff.".into(),
        "grilling" => match open_turn(idea) {
            Some(m) => format!("Claude asked {} question(s). Press a to answer (t to type a reply).", m.questions.as_ref().map_or(0, Vec::len)),
            None => "Press t to reply to Claude.".into(),
        },
        "review" => "Handoff ready. Press e to edit it, s to send it.".into(),
        "ready" => "Queued; your laptop picks it up shortly. Press u to pull it back.".into(),
        "dispatching" => "Launching on your laptop…".into(),
        "dispatched" => match &idea.job {
            Some(j) if !job_actions(j).is_empty() => "Droplet job needs a decision (see the actions below).".into(),
            Some(j) if JOB_BAD_END.contains(&j.status.as_str()) => "Job ended. Edit the handoff (e) and send again (s).".into(),
            Some(_) => "Droplet job in progress.".into(),
            None => "Running on your laptop.".into(),
        },
        "failed" => "Dispatch failed. Fix the handoff (e) and send again (s).".into(),
        "archived" => "Archived. Press z to restore, D to delete.".into(),
        _ => String::new(),
    }
}

/// Key hints for the footer given the current state.
pub fn hints(app: &App) -> Vec<(&'static str, &'static str)> {
    match &app.modal {
        Modal::Help => vec![("any key", "close")],
        Modal::Capture(_) => vec![("Ctrl+S", "save idea"), ("Esc", "cancel")],
        Modal::Reply { .. } => vec![("Ctrl+S", "send reply"), ("Esc", "cancel")],
        Modal::Answer(a) if a.editing => vec![("type", "your answer"), ("Enter", "done"), ("Ctrl+S", "submit")],
        Modal::Answer(_) => vec![
            ("j/k", "move"),
            ("Space", "select"),
            ("Tab", "next question"),
            ("e", "explain"),
            ("Ctrl+S", "submit"),
            ("Esc", "close"),
        ],
        Modal::Handoff(_) => vec![("Tab", "next field"), ("Ctrl+N/P", "project"), ("Space", "visibility"), ("Ctrl+S", "keep"), ("Esc", "discard")],
        Modal::Send(s) => {
            let mut v = vec![("1", "laptop")];
            if !s.new_project {
                v.push(("2", "droplet"));
            }
            if s.dest == Dest::Droplet {
                v.push(("p/v", "preview/live"));
            }
            v.push(("Enter", "continue"));
            v.push(("Esc", "cancel"));
            v
        }
        Modal::Confirm(_) => vec![("y", "yes"), ("n", "no")],
        Modal::None => match (app.screen, &app.current) {
            (Screen::Detail, Some(i)) => detail_hints(i),
            _ => vec![
                ("j/k", "move"),
                ("Enter", "open"),
                ("n", "new idea"),
                ("Tab", "filter"),
                ("A", "archived"),
                ("r", "refresh"),
                ("?", "help"),
                ("q", "quit"),
            ],
        },
    }
}

fn detail_hints(i: &Idea) -> Vec<(&'static str, &'static str)> {
    let mut v = vec![("j/k", "scroll")];
    if can_start_grill(i) {
        v.push(("g", "grill me"));
    }
    if can_chat(i) {
        if open_turn(i).is_some() {
            v.push(("a", "answer"));
        }
        v.push(("t", "reply"));
    }
    if can_write_handoff(i) {
        v.push(("w", "write handoff"));
    }
    if can_edit_handoff(i) {
        v.push(("e", "edit"));
    }
    if can_send(i) {
        v.push(("s", "send"));
    }
    if can_unsend(i) {
        v.push(("u", "pull back"));
    }
    if let Some(j) = &i.job {
        for a in job_actions(j) {
            v.push(match a {
                JobAction::Ship => ("S", "ship"),
                JobAction::Discard => ("X", "discard"),
                JobAction::Rollback => ("B", "rollback"),
            });
        }
    }
    if can_cancel_grill(i) {
        v.push(("C", "cancel grill"));
    }
    if i.status == "archived" {
        v.push(("z", "restore"));
    } else if can_archive(i) {
        v.push(("z", "archive"));
    }
    if can_delete(i) {
        v.push(("D", "delete"));
    }
    if i.deploy.as_ref().is_some_and(|d| d.log.is_some()) {
        v.push(("l", "deploy log"));
    }
    v.extend([("r", "refresh"), ("?", "help"), ("Esc", "back")]);
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::ApiError;
    use ratatui::crossterm::event::KeyEventState;
    use tokio::sync::mpsc::unbounded_channel;

    fn key(c: KeyCode) -> KeyEvent {
        KeyEvent { code: c, modifiers: KeyModifiers::NONE, kind: KeyEventKind::Press, state: KeyEventState::NONE }
    }
    fn ctrl(c: char) -> KeyEvent {
        KeyEvent { code: KeyCode::Char(c), modifiers: KeyModifiers::CONTROL, kind: KeyEventKind::Press, state: KeyEventState::NONE }
    }
    fn app() -> (App, tokio::sync::mpsc::UnboundedReceiver<Msg>) {
        let (tx, rx) = unbounded_channel();
        // Nothing listens here: requests fail fast and only matter if a test awaits them.
        (App::new(Client::with_base("http://127.0.0.1:9".into(), "t".into()), tx), rx)
    }
    fn idea(id: &str, status: &str, updated: &str) -> Idea {
        Idea { id: id.into(), status: status.into(), text: format!("idea {id}"), updated_at: Some(updated.into()), ..Default::default() }
    }

    #[tokio::test]
    async fn list_navigation_and_filter() {
        let (mut a, _rx) = app();
        a.update(Msg::Ideas {
            seq: 0,
            result: Ok(vec![idea("a", "inbox", "1"), idea("b", "archived", "2"), idea("c", "review", "3")]),
        });
        assert_eq!(a.visible().len(), 2, "archived hidden by default");
        assert_eq!(a.selected_idea().unwrap().id, "c", "newest first");
        a.on_key(key(KeyCode::Char('j')));
        assert_eq!(a.selected_idea().unwrap().id, "a");
        a.on_key(key(KeyCode::Char('j')));
        assert_eq!(a.selected_idea().unwrap().id, "a", "clamped");
        a.on_key(key(KeyCode::Char('A')));
        assert_eq!(a.filter, Filter::Archived);
        assert_eq!(a.selected_idea().unwrap().id, "b");
        a.on_key(key(KeyCode::Char('A')));
        assert_eq!(a.filter, Filter::Open);
    }

    #[tokio::test]
    async fn capture_keeps_text_on_failure_and_rejects_empty() {
        let (mut a, _rx) = app();
        a.on_key(key(KeyCode::Char('n')));
        assert!(matches!(a.modal, Modal::Capture(_)));
        a.on_key(ctrl('s'));
        assert!(a.status.as_ref().unwrap().error, "empty idea rejected");
        for c in "hi".chars() {
            a.on_key(key(KeyCode::Char(c)));
        }
        a.on_key(key(KeyCode::Enter));
        for c in "there".chars() {
            a.on_key(key(KeyCode::Char(c)));
        }
        a.update(Msg::Created(Err(ApiError::Network("down".into()))));
        match &a.modal {
            Modal::Capture(t) => assert_eq!(t.lines(), ["hi", "there"]),
            _ => panic!("capture closed on failure"),
        }
        assert!(a.status.as_ref().unwrap().text.contains("Not saved"));
        a.update(Msg::Created(Ok(idea("n1", "inbox", "9"))));
        assert!(matches!(a.modal, Modal::None));
        assert_eq!(a.selected_idea().unwrap().id, "n1");
    }

    #[tokio::test]
    async fn answering_a_turn_with_keys() {
        let (mut a, _rx) = app();
        let mut i = idea("g", "grilling", "1");
        i.grill_session = Some("s".into());
        i.messages = vec![Message {
            role: "claude".into(),
            text: "Let's go".into(),
            at: Some("t1".into()),
            questions: Some(vec![
                Question { id: "q1".into(), text: "Who?".into(), kind: "single".into(), options: vec!["me".into(), "you".into()] },
                Question { id: "q2".into(), text: "What?".into(), kind: "multi".into(), options: vec!["x".into(), "y".into()] },
            ]),
            done: Some(false),
        }];
        a.open_idea(i);
        a.on_key(key(KeyCode::Char('a')));
        assert!(matches!(a.modal, Modal::Answer(_)));
        // q1: pick "you"
        a.on_key(key(KeyCode::Char('j')));
        a.on_key(key(KeyCode::Char(' ')));
        // submit too early
        a.on_key(ctrl('s'));
        assert!(a.status.as_ref().unwrap().text.contains("1 of 2"));
        // q2: x + Other text
        a.on_key(key(KeyCode::Tab));
        a.on_key(key(KeyCode::Char(' ')));
        a.on_key(key(KeyCode::Char('j')));
        a.on_key(key(KeyCode::Char('j')));
        a.on_key(key(KeyCode::Char(' '))); // Other on -> editing
        for c in "z!".chars() {
            a.on_key(key(KeyCode::Char(c)));
        }
        a.on_key(key(KeyCode::Enter));
        let Modal::Answer(m) = &a.modal else { panic!() };
        assert_eq!(m.form.compose(), "Who?\n→ you\n\nWhat?\n→ x; Other: z!");
        // Esc keeps the answers for next time.
        a.on_key(key(KeyCode::Esc));
        a.on_key(key(KeyCode::Char('a')));
        let Modal::Answer(m) = &a.modal else { panic!() };
        assert!(m.form.all_answered());
    }

    #[tokio::test]
    async fn destructive_keys_respect_state() {
        let (mut a, _rx) = app();
        a.open_idea(idea("d", "grilling", "1"));
        a.on_key(key(KeyCode::Char('D')));
        assert!(matches!(a.modal, Modal::None));
        assert!(a.status.as_ref().unwrap().text.contains("Can't delete"));
        a.update(Msg::Mutated {
            id: "d".into(),
            what: "archive",
            result: Err(ApiError::Http { status: 409, message: "Can't delete an idea that is grilling".into() }),
        });
        assert_eq!(a.status.as_ref().unwrap().text, "Can't delete an idea that is grilling");
        a.current.as_mut().unwrap().status = "inbox".into();
        a.on_key(key(KeyCode::Char('D')));
        assert!(matches!(a.modal, Modal::Confirm(_)));
        a.on_key(key(KeyCode::Char('n')));
        assert!(matches!(a.modal, Modal::None));
    }

    #[tokio::test]
    async fn handoff_edits_become_a_draft_and_drive_the_send_dialog() {
        let (mut a, _rx) = app();
        a.projects = vec!["alpha".into(), "beta".into()];
        a.projects_loaded = true;
        let mut i = idea("h", "review", "1");
        i.handoff = Some(Handoff { title: "T".into(), project: "alpha".into(), brief: "line1\nline2".into(), visibility: "private".into(), is_new: false });
        a.open_idea(i);
        a.on_key(key(KeyCode::Char('e')));
        assert!(matches!(a.modal, Modal::Handoff(_)));
        a.on_key(key(KeyCode::Char('!'))); // title -> "T!"
        a.on_key(key(KeyCode::Tab));
        a.on_key(ctrl('n')); // alpha -> beta
        a.on_key(ctrl('s'));
        let d = &a.drafts["h"];
        assert_eq!((d.title.as_str(), d.project.as_str(), d.brief.as_str()), ("T!", "beta", "line1\nline2"));
        a.on_key(key(KeyCode::Char('s')));
        let Modal::Send(s) = &a.modal else { panic!("send dialog") };
        assert!(!s.new_project);
        a.on_key(key(KeyCode::Char('2')));
        a.on_key(key(KeyCode::Char('v')));
        a.on_key(key(KeyCode::Enter));
        let Modal::Confirm(c) = &a.modal else { panic!("confirm") };
        let ConfirmAction::Send { req, .. } = &c.action else { panic!() };
        assert_eq!((req.destination.as_str(), req.mode.as_deref(), req.project.as_str()), ("droplet", Some("live"), "beta"));
    }

    #[tokio::test]
    async fn new_projects_cannot_go_to_the_droplet() {
        let (mut a, _rx) = app();
        a.projects = vec!["alpha".into()];
        a.projects_loaded = true;
        let mut i = idea("h", "review", "1");
        i.handoff = Some(Handoff { title: "T".into(), project: "brand-new".into(), brief: "b".into(), ..Default::default() });
        a.open_idea(i);
        a.on_key(key(KeyCode::Char('s')));
        a.on_key(key(KeyCode::Char('2')));
        let Modal::Send(s) = &a.modal else { panic!() };
        assert!(s.new_project && s.dest == Dest::Laptop);
        assert!(a.status.as_ref().unwrap().error);
    }

    #[tokio::test]
    async fn stale_fetches_are_dropped_after_a_mutation() {
        let (mut a, _rx) = app();
        a.update(Msg::Ideas { seq: 0, result: Ok(vec![idea("a", "inbox", "1")]) });
        a.update(Msg::Mutated { id: "a".into(), what: "grill", result: Ok(idea("a", "grilling", "2")) });
        a.update(Msg::Ideas { seq: 0, result: Ok(vec![idea("a", "inbox", "1")]) });
        assert_eq!(a.ideas[0].status, "grilling");
    }

    #[test]
    fn hints_follow_state() {
        let mut i = idea("x", "dispatched", "1");
        i.dispatch = Some(Dispatch { destination: Some("droplet".into()), ..Default::default() });
        i.job = Some(Job { status: "preview".into(), ..Default::default() });
        let keys: Vec<_> = detail_hints(&i).iter().map(|h| h.0).collect();
        assert!(keys.contains(&"S") && keys.contains(&"X") && !keys.contains(&"B"));
        assert!(next_step(&i).contains("decision"));
    }
}
