//! Pure logic with no IO or rendering: answer collection for grilling turns,
//! which actions a given idea/job state allows, list filtering.

use crate::types::*;

// ---------- grilling answers ----------

#[derive(Debug, Clone, Default, PartialEq)]
pub struct QAnswer {
    pub sel: Vec<usize>,
    pub other: bool,
    pub text: String,
}

impl QAnswer {
    pub fn is_answered(&self) -> bool {
        !self.sel.is_empty() || (self.other && !self.text.trim().is_empty())
    }
}

/// One selectable row of the answer form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row {
    pub q: usize,
    /// `Some(i)` is option `i`; `None` is the free-text "Other" row.
    pub opt: Option<usize>,
}

/// The answers being collected for one structured turn.
#[derive(Debug, Clone, PartialEq)]
pub struct TurnForm {
    pub questions: Vec<Question>,
    pub answers: Vec<QAnswer>,
    /// Index into `rows()`.
    pub cursor: usize,
}

impl TurnForm {
    pub fn new(questions: Vec<Question>) -> TurnForm {
        let answers = vec![QAnswer::default(); questions.len()];
        TurnForm { questions, answers, cursor: 0 }
    }

    pub fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        for (q, question) in self.questions.iter().enumerate() {
            for i in 0..question.options.len() {
                rows.push(Row { q, opt: Some(i) });
            }
            rows.push(Row { q, opt: None });
        }
        rows
    }

    pub fn current(&self) -> Option<Row> {
        self.rows().get(self.cursor).copied()
    }

    pub fn move_cursor(&mut self, delta: isize) {
        let n = self.rows().len();
        if n > 0 {
            self.cursor = (self.cursor as isize + delta).clamp(0, n as isize - 1) as usize;
        }
    }

    /// Jumps to the first row of the next (or previous) question.
    pub fn move_question(&mut self, forward: bool) {
        let rows = self.rows();
        let Some(cur) = rows.get(self.cursor).map(|r| r.q) else { return };
        let target = if forward { (cur + 1).min(self.questions.len() - 1) } else { cur.saturating_sub(1) };
        if let Some(i) = rows.iter().position(|r| r.q == target) {
            self.cursor = i;
        }
    }

    /// Toggles the row under the cursor. Single questions behave like radio
    /// buttons (picking an option clears "Other" and vice versa); multi
    /// questions toggle independently. Returns true if "Other" was switched on
    /// so the caller can start text entry.
    pub fn toggle_current(&mut self) -> bool {
        let Some(row) = self.current() else { return false };
        let multi = self.questions[row.q].is_multi();
        let a = &mut self.answers[row.q];
        match row.opt {
            Some(i) => {
                if multi {
                    match a.sel.iter().position(|&s| s == i) {
                        Some(p) => {
                            a.sel.remove(p);
                        }
                        None => a.sel.push(i),
                    }
                } else {
                    a.sel = vec![i];
                    a.other = false;
                }
                false
            }
            None => {
                a.other = !a.other;
                if a.other && !multi {
                    a.sel.clear();
                }
                a.other
            }
        }
    }

    pub fn other_text_mut(&mut self) -> Option<&mut String> {
        let row = self.current()?;
        (row.opt.is_none()).then(|| &mut self.answers[row.q].text)
    }

    pub fn answered_count(&self) -> usize {
        self.answers.iter().filter(|a| a.is_answered()).count()
    }

    pub fn all_answered(&self) -> bool {
        !self.questions.is_empty() && self.answered_count() == self.questions.len()
    }

    /// The reply sent to Claude and shown in the transcript: each question,
    /// then `→ ` and the picks (same format as the web app).
    pub fn compose(&self) -> String {
        self.questions
            .iter()
            .zip(&self.answers)
            .map(|(q, a)| {
                let mut sel = a.sel.clone();
                sel.sort_unstable();
                let mut picks: Vec<String> = sel.iter().filter_map(|&i| q.options.get(i).cloned()).collect();
                if a.other && !a.text.trim().is_empty() {
                    picks.push(format!("Other: {}", a.text.trim()));
                }
                format!("{}\n→ {}", q.text, picks.join("; "))
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

// ---------- idea state ----------

pub const ALL_STATUSES: [&str; 8] = ["inbox", "grilling", "review", "ready", "dispatching", "dispatched", "failed", "archived"];

/// The last message, if it is a structured Claude turn with questions to answer.
pub fn open_turn(idea: &Idea) -> Option<&Message> {
    idea.messages.last().filter(|m| m.role == "claude" && m.questions.as_ref().is_some_and(|q| !q.is_empty()))
}

pub fn has_session(idea: &Idea) -> bool {
    idea.grill_session.is_some() || !idea.messages.is_empty()
}

/// Statuses where the grill conversation can continue.
pub fn can_chat(idea: &Idea) -> bool {
    matches!(idea.status.as_str(), "inbox" | "grilling" | "review" | "failed") && has_session(idea) && !idea.busy
}

pub fn can_start_grill(idea: &Idea) -> bool {
    idea.status == "inbox" && !has_session(idea) && !idea.busy
}

pub fn can_cancel_grill(idea: &Idea) -> bool {
    idea.status == "grilling" || (idea.status == "inbox" && idea.busy)
}

/// Claude marked the grill finished: offer "Write handoff".
pub fn grill_done(idea: &Idea) -> bool {
    can_chat(idea) && idea.messages.last().is_some_and(|m| m.role == "claude" && m.done == Some(true))
}

pub fn can_write_handoff(idea: &Idea) -> bool {
    matches!(idea.status.as_str(), "grilling" | "review") && has_session(idea) && !idea.busy
}

pub fn can_delete(idea: &Idea) -> bool {
    idea.status == "archived" || (idea.status == "inbox" && !idea.busy)
}

pub fn can_archive(idea: &Idea) -> bool {
    idea.status != "archived" && idea.status != "dispatching"
}

pub fn can_unsend(idea: &Idea) -> bool {
    idea.status == "ready"
}

pub const JOB_BAD_END: [&str; 4] = ["failed", "rejected", "discarded", "rolled_back"];

pub fn is_droplet(idea: &Idea) -> bool {
    idea.dispatch.as_ref().and_then(|d| d.destination.as_deref()) == Some("droplet")
}

/// A sent handoff can't be edited while it is queued or running, unless it
/// went to the droplet and that job ended badly.
pub fn handoff_locked(idea: &Idea) -> bool {
    let sent = matches!(idea.status.as_str(), "ready" | "dispatching" | "dispatched");
    let reopened = is_droplet(idea) && idea.job.as_ref().is_some_and(|j| JOB_BAD_END.contains(&j.status.as_str()));
    sent && !reopened
}

pub fn can_edit_handoff(idea: &Idea) -> bool {
    idea.handoff.is_some() && idea.status != "archived" && idea.status != "inbox" && !handoff_locked(idea) && !idea.busy
}

pub fn can_send(idea: &Idea) -> bool {
    can_edit_handoff(idea)
}

/// Ship, discard and rollback exactly where the server accepts them.
pub fn job_actions(job: &Job) -> Vec<JobAction> {
    if job.action.is_some() {
        return vec![];
    }
    match job.status.as_str() {
        "preview" => vec![JobAction::Ship, JobAction::Discard],
        "failed" => vec![JobAction::Discard],
        "live" => vec![JobAction::Rollback],
        _ => vec![],
    }
}

pub fn job_busy(job: &Job) -> bool {
    job.action.is_some() || matches!(job.status.as_str(), "queued" | "running" | "testing" | "deploying" | "rolling_back")
}

/// Should the open idea be re-fetched every couple of seconds?
pub fn needs_fast_poll(idea: &Idea) -> bool {
    idea.status == "grilling"
        || idea.busy
        || matches!(idea.status.as_str(), "ready" | "dispatching")
        || idea.job.as_ref().is_some_and(job_busy)
}

pub fn job_label(status: &str) -> &str {
    match status {
        "queued" => "Waiting for the droplet runner",
        "running" => "Claude Code is working on it",
        "testing" => "Running the tests",
        "preview" => "Preview ready",
        "deploying" => "Deploying",
        "live" => "Live",
        "rolling_back" => "Rolling back",
        "rolled_back" => "Rolled back",
        "discarded" => "Discarded",
        "failed" => "Failed",
        "rejected" => "Not allowed on the droplet",
        other => other,
    }
}

/// Mirrors the server's PROJECT_NAME check.
pub fn valid_project_name(name: &str) -> bool {
    let mut chars = name.chars();
    name.len() <= 64
        && chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

// ---------- list ----------

pub fn display_title(idea: &Idea) -> String {
    let t = idea.handoff.as_ref().map(|h| h.title.trim()).filter(|t| !t.is_empty());
    t.map(str::to_string).unwrap_or_else(|| {
        idea.text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("(empty)").to_string()
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    /// Everything except archived.
    Open,
    Inbox,
    Grilling,
    Sent,
    Archived,
    /// Everything, archived included.
    All,
}

impl Filter {
    pub const ORDER: [Filter; 6] = [Filter::Open, Filter::Inbox, Filter::Grilling, Filter::Sent, Filter::Archived, Filter::All];

    pub fn label(self) -> &'static str {
        match self {
            Filter::Open => "Open",
            Filter::Inbox => "Inbox",
            Filter::Grilling => "Grilling",
            Filter::Sent => "Sent",
            Filter::Archived => "Archived",
            Filter::All => "All",
        }
    }

    pub fn matches(self, idea: &Idea) -> bool {
        let s = idea.status.as_str();
        match self {
            Filter::Open => s != "archived",
            Filter::Inbox => s == "inbox",
            Filter::Grilling => matches!(s, "grilling" | "review"),
            Filter::Sent => matches!(s, "ready" | "dispatching" | "dispatched" | "failed"),
            Filter::Archived => s == "archived",
            Filter::All => true,
        }
    }

    pub fn next(self, forward: bool) -> Filter {
        let i = Filter::ORDER.iter().position(|f| *f == self).unwrap_or(0);
        let n = Filter::ORDER.len();
        Filter::ORDER[if forward { (i + 1) % n } else { (i + n - 1) % n }]
    }
}

/// Newest activity first.
pub fn sort_ideas(ideas: &mut [Idea]) {
    ideas.sort_by(|a, b| {
        let key = |i: &Idea| i.updated_at.clone().or_else(|| i.created_at.clone()).unwrap_or_default();
        key(b).cmp(&key(a))
    });
}

pub fn filtered(ideas: &[Idea], filter: Filter) -> Vec<&Idea> {
    ideas.iter().filter(|i| filter.matches(i)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(id: &str, kind: &str, opts: &[&str]) -> Question {
        Question { id: id.into(), text: format!("Question {id}?"), kind: kind.into(), options: opts.iter().map(|s| s.to_string()).collect() }
    }

    fn idea(status: &str) -> Idea {
        Idea { id: "i1".into(), status: status.into(), text: "hello\nworld".into(), ..Default::default() }
    }

    fn job(status: &str) -> Job {
        Job { status: status.into(), ..Default::default() }
    }

    #[test]
    fn single_question_is_radio() {
        let mut f = TurnForm::new(vec![q("q1", "single", &["a", "b", "c"])]);
        f.cursor = 0;
        f.toggle_current();
        f.cursor = 2;
        f.toggle_current();
        assert_eq!(f.answers[0].sel, vec![2]);
        assert!(f.all_answered());
        assert_eq!(f.compose(), "Question q1?\n→ c");
    }

    #[test]
    fn multi_question_toggles_and_sorts() {
        let mut f = TurnForm::new(vec![q("q1", "multi", &["a", "b", "c"])]);
        f.cursor = 2;
        f.toggle_current();
        f.cursor = 0;
        f.toggle_current();
        f.cursor = 2;
        f.toggle_current(); // off again
        f.cursor = 1;
        f.toggle_current();
        assert_eq!(f.compose(), "Question q1?\n→ a; b");
    }

    #[test]
    fn other_text_joins_picks_and_clears_single() {
        let mut f = TurnForm::new(vec![q("q1", "single", &["a", "b"]), q("q2", "multi", &["x", "y"])]);
        // q1: pick a, then switch to Other: the pick is cleared.
        f.toggle_current();
        f.cursor = 2; // q1 Other
        assert!(f.toggle_current());
        f.other_text_mut().unwrap().push_str("  something else ");
        assert!(f.answers[0].sel.is_empty());
        // q2: Other alongside a pick.
        f.cursor = 3;
        f.toggle_current();
        f.cursor = 5; // q2 Other
        f.toggle_current();
        f.other_text_mut().unwrap().push('z');
        assert_eq!(f.compose(), "Question q1?\n→ Other: something else\n\nQuestion q2?\n→ x; Other: z");
    }

    #[test]
    fn empty_other_does_not_count_as_answered() {
        let mut f = TurnForm::new(vec![q("q1", "single", &["a"])]);
        f.cursor = 1;
        f.toggle_current();
        assert!(!f.all_answered());
        f.other_text_mut().unwrap().push_str("hi");
        assert!(f.all_answered());
        assert_eq!(f.answered_count(), 1);
    }

    #[test]
    fn cursor_moves_within_bounds_and_between_questions() {
        let mut f = TurnForm::new(vec![q("q1", "single", &["a", "b"]), q("q2", "single", &["c"])]);
        assert_eq!(f.rows().len(), 5);
        f.move_cursor(-3);
        assert_eq!(f.cursor, 0);
        f.move_question(true);
        assert_eq!(f.current(), Some(Row { q: 1, opt: Some(0) }));
        f.move_question(true);
        assert_eq!(f.current().unwrap().q, 1);
        f.move_question(false);
        assert_eq!(f.cursor, 0);
        f.move_cursor(99);
        assert_eq!(f.current(), Some(Row { q: 1, opt: None }));
    }

    #[test]
    fn job_actions_follow_server_rules() {
        assert_eq!(job_actions(&job("preview")), vec![JobAction::Ship, JobAction::Discard]);
        assert_eq!(job_actions(&job("failed")), vec![JobAction::Discard]);
        assert_eq!(job_actions(&job("live")), vec![JobAction::Rollback]);
        for s in ["queued", "running", "testing", "deploying", "rolling_back", "rolled_back", "discarded", "rejected"] {
            assert!(job_actions(&job(s)).is_empty(), "{s}");
        }
        let mut j = job("preview");
        j.action = Some(JobPending { kind: "ship".into() });
        assert!(job_actions(&j).is_empty());
        assert!(job_busy(&j));
        assert!(job_busy(&job("running")));
        assert!(!job_busy(&job("live")));
    }

    #[test]
    fn handoff_lock_rules() {
        let mut i = idea("review");
        i.handoff = Some(Handoff { project: "p".into(), brief: "b".into(), ..Default::default() });
        assert!(can_edit_handoff(&i) && can_send(&i));
        i.status = "ready".into();
        assert!(handoff_locked(&i) && !can_send(&i));
        i.status = "failed".into();
        assert!(can_send(&i), "a failed dispatch can be edited and resent");
        i.status = "dispatched".into();
        assert!(!can_send(&i));
        i.dispatch = Some(Dispatch { destination: Some("droplet".into()), ..Default::default() });
        i.job = Some(job("live"));
        assert!(!can_send(&i));
        i.job = Some(job("rolled_back"));
        assert!(can_send(&i), "a droplet job that ended badly unlocks a resend");
        // The same job state on a laptop dispatch does not.
        i.dispatch = Some(Dispatch { destination: Some("laptop".into()), ..Default::default() });
        assert!(!can_send(&i));
    }

    #[test]
    fn delete_only_inbox_or_archived() {
        assert!(can_delete(&idea("inbox")));
        assert!(can_delete(&idea("archived")));
        for s in ["grilling", "review", "ready", "dispatching", "dispatched", "failed"] {
            assert!(!can_delete(&idea(s)), "{s}");
        }
        let mut busy = idea("inbox");
        busy.busy = true;
        assert!(!can_delete(&busy));
        assert!(can_cancel_grill(&busy));
    }

    #[test]
    fn grill_flow_gates() {
        let mut i = idea("inbox");
        assert!(can_start_grill(&i));
        i.status = "grilling".into();
        i.grill_session = Some("s".into());
        i.messages = vec![Message {
            role: "claude".into(),
            text: "ok".into(),
            questions: Some(vec![q("q1", "single", &["a", "b"])]),
            done: Some(false),
            ..Default::default()
        }];
        assert!(!can_start_grill(&i) && can_chat(&i) && can_cancel_grill(&i));
        assert!(open_turn(&i).is_some());
        assert!(!grill_done(&i));
        i.messages.last_mut().unwrap().done = Some(true);
        assert!(grill_done(&i) && can_write_handoff(&i));
        i.messages.push(Message { role: "me".into(), text: "x".into(), ..Default::default() });
        assert!(open_turn(&i).is_none());
        i.busy = true;
        assert!(!can_chat(&i) && !can_write_handoff(&i));
    }

    #[test]
    fn project_names() {
        assert!(valid_project_name("my-app_1.x"));
        assert!(!valid_project_name(""));
        assert!(!valid_project_name("-lead"));
        assert!(!valid_project_name("has space"));
        assert!(!valid_project_name(&"a".repeat(65)));
    }

    #[test]
    fn titles_and_filters() {
        let mut i = idea("inbox");
        i.text = "\n  First line \nsecond".into();
        assert_eq!(display_title(&i), "First line");
        i.handoff = Some(Handoff { title: "Handoff title".into(), ..Default::default() });
        assert_eq!(display_title(&i), "Handoff title");

        let ideas: Vec<Idea> = ALL_STATUSES.iter().map(|s| idea(s)).collect();
        let n = |f| filtered(&ideas, f).len();
        assert_eq!(n(Filter::Open), 7);
        assert_eq!(n(Filter::Inbox), 1);
        assert_eq!(n(Filter::Grilling), 2);
        assert_eq!(n(Filter::Sent), 4);
        assert_eq!(n(Filter::Archived), 1);
        assert_eq!(n(Filter::All), 8);
        assert_eq!(Filter::All.next(true), Filter::Open);
        assert_eq!(Filter::Open.next(false), Filter::All);
    }

    #[test]
    fn polling_rules() {
        assert!(needs_fast_poll(&idea("grilling")));
        assert!(!needs_fast_poll(&idea("review")));
        let mut i = idea("dispatched");
        i.job = Some(job("running"));
        assert!(needs_fast_poll(&i));
        i.job = Some(job("live"));
        assert!(!needs_fast_poll(&i));
    }
}
