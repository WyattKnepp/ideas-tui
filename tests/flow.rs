//! End to end: the real `App` driven by key presses against a small stateful
//! fake of the pipeline's client API. Covers capture, grill (single, multi,
//! Other and free-text answers), handoff, edit, send to laptop and droplet,
//! deploy/job display, and the ship / discard / rollback actions.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use ideas_tui::api::Client;
use ideas_tui::app::{App, Modal, Msg, Screen};
use ideas_tui::state::{can_send, grill_done, job_actions};
use ideas_tui::types::JobAction;
use ideas_tui::ui;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
use serde_json::{Value, json};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const TOKEN: &str = "flow-token";

#[derive(Default)]
struct Fake {
    idea: Option<Value>,
    grills: Vec<String>,
    sends: Vec<Value>,
    actions: Vec<String>,
    unauthorized: usize,
}

type Shared = Arc<Mutex<Fake>>;

fn auth_ok(req: &Request, f: &mut Fake) -> bool {
    let ok = req.headers.get("authorization").and_then(|v| v.to_str().ok()) == Some(&format!("Bearer {TOKEN}"));
    if !ok {
        f.unauthorized += 1;
    }
    ok
}

fn body(req: &Request) -> Value {
    serde_json::from_slice(&req.body).unwrap_or(Value::Null)
}

/// A job's pending action completes the next time the idea is read.
fn settle(idea: &mut Value) {
    let Some(job) = idea.get_mut("job").filter(|j| !j.is_null()) else { return };
    let Some(kind) = job["action"]["type"].as_str().map(str::to_string) else { return };
    job["status"] = json!(match kind.as_str() {
        "ship" => "live",
        "discard" => "discarded",
        _ => "rolled_back",
    });
    job["action"] = Value::Null;
}

fn route(f: &Shared, m: &'static str, p: &str, handler: impl Fn(&Request, &mut Fake) -> ResponseTemplate + Send + Sync + 'static) -> Mock {
    let f = f.clone();
    let b = Mock::given(method(m));
    let b = if p.contains('(') { b.and(path_regex(p)) } else { b.and(path(p)) };
    b.respond_with(move |req: &Request| {
        let mut fake = f.lock().unwrap();
        if !auth_ok(req, &mut fake) {
            return ResponseTemplate::new(401);
        }
        handler(req, &mut fake)
    })
}

fn turn(text: &str, questions: Value, done: bool) -> Value {
    json!({"role": "claude", "text": text, "at": format!("t{text}"), "questions": questions, "done": done})
}

async fn fake_server() -> (MockServer, Shared) {
    let s = MockServer::start().await;
    let f: Shared = Arc::default();
    let json_ok = |v: Value| ResponseTemplate::new(200).set_body_json(v);

    route(&f, "GET", "/client/api/ideas", move |_, f| {
        let list: Vec<Value> = f.idea.iter().cloned().map(|mut i| { settle(&mut i); i }).collect();
        ResponseTemplate::new(200).set_body_json(list)
    }).mount(&s).await;
    route(&f, "POST", "/client/api/ideas", move |req, f| {
        f.idea = Some(json!({"id": "i1", "text": body(req)["text"], "status": "inbox", "messages": [],
                             "createdAt": "2026-11-04T10:00:00Z", "updatedAt": "2026-11-04T10:00:00Z", "busy": false}));
        ResponseTemplate::new(200).set_body_json(f.idea.clone())
    }).mount(&s).await;
    route(&f, "GET", "/client/api/ideas/i1", move |_, f| {
        if let Some(i) = f.idea.as_mut() { settle(i); }
        ResponseTemplate::new(200).set_body_json(f.idea.clone())
    }).mount(&s).await;
    route(&f, "GET", "/client/api/projects", move |_, _| {
        ResponseTemplate::new(200).set_body_json(json!({"syncedAt": null, "projects": [{"name": "alpha"}, {"name": "beta"}]}))
    }).mount(&s).await;

    route(&f, "POST", "/client/api/ideas/i1/grill", move |req, f| {
        let msg = body(req)["message"].as_str().unwrap_or("").to_string();
        let n = f.grills.len();
        f.grills.push(msg.clone());
        let i = f.idea.as_mut().unwrap();
        i["grillSession"] = json!("sess");
        i["status"] = json!("grilling");
        let msgs = i["messages"].as_array_mut().unwrap();
        if !msg.is_empty() {
            msgs.push(json!({"role": "me", "text": msg, "at": "x"}));
        }
        msgs.push(match n {
            0 => turn(
                "Two things.",
                json!([
                    {"id": "q1", "text": "Who is it for?", "type": "single", "options": ["Me", "My team", "Everyone"]},
                    {"id": "q2", "text": "Which platforms?", "type": "multi", "options": ["Linux", "Mac", "Windows"]}
                ]),
                false,
            ),
            1 => json!({"role": "claude", "text": "Anything else I should know?", "at": "plain"}),
            _ => turn("That is enough.", json!([]), true),
        });
        ResponseTemplate::new(200).set_body_json(f.idea.clone())
    }).mount(&s).await;

    route(&f, "POST", "/client/api/ideas/i1/handoff", move |_, f| {
        let i = f.idea.as_mut().unwrap();
        i["status"] = json!("review");
        i["handoff"] = json!({"title": "Widget", "project": "alpha", "isNew": false, "visibility": "private", "brief": "Build the widget.\nWith care."});
        ResponseTemplate::new(200).set_body_json(f.idea.clone())
    }).mount(&s).await;

    route(&f, "POST", "/client/api/ideas/i1/send", move |req, f| {
        let b = body(req);
        f.sends.push(b.clone());
        let i = f.idea.as_mut().unwrap();
        i["handoff"] = json!({"title": b["title"], "project": b["project"], "isNew": false, "visibility": b["visibility"], "brief": b["brief"]});
        if b["destination"] == "droplet" {
            let live = b["mode"] == "live";
            i["status"] = json!("dispatched");
            i["dispatch"] = json!({"destination": "droplet", "jobId": 1, "mode": b["mode"]});
            i["job"] = json!({"id": 1, "status": if live { "live" } else { "preview" }, "mode": b["mode"],
                              "note": "Built", "preview_url": "https://preview.example", "action": null, "events": []});
        } else {
            i["status"] = json!("ready");
            i["dispatch"] = Value::Null;
        }
        ResponseTemplate::new(200).set_body_json(f.idea.clone())
    }).mount(&s).await;
    route(&f, "POST", "/client/api/ideas/i1/unsend", move |_, f| {
        f.idea.as_mut().unwrap()["status"] = json!("review");
        ResponseTemplate::new(200).set_body_json(f.idea.clone())
    }).mount(&s).await;
    route(&f, "POST", "/client/api/ideas/i1/job/actions", move |req, f| {
        let a = body(req)["action"].as_str().unwrap().to_string();
        f.actions.push(a.clone());
        f.idea.as_mut().unwrap()["job"]["action"] = json!({"type": a});
        ResponseTemplate::new(200).set_body_json(f.idea.clone())
    }).mount(&s).await;
    let _ = json_ok;
    (s, f)
}

fn k(code: KeyCode) -> KeyEvent {
    KeyEvent { code, modifiers: KeyModifiers::NONE, kind: KeyEventKind::Press, state: KeyEventState::NONE }
}
fn ctrl_s() -> KeyEvent {
    KeyEvent { modifiers: KeyModifiers::CONTROL, ..k(KeyCode::Char('s')) }
}

struct Rig {
    app: App,
    rx: UnboundedReceiver<Msg>,
}

impl Rig {
    fn key(&mut self, c: char) {
        self.app.on_key(k(KeyCode::Char(c)));
    }
    fn keys(&mut self, s: &str) {
        for c in s.chars() {
            self.key(c);
        }
    }
    fn press(&mut self, code: KeyCode) {
        self.app.on_key(k(code));
    }
    /// Applies incoming messages until `cond` holds (or fails after 5s).
    async fn until(&mut self, what: &str, cond: impl Fn(&App) -> bool) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while !cond(&self.app) {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            match tokio::time::timeout(left, self.rx.recv()).await {
                Ok(Some(m)) => self.app.update(m),
                _ => panic!("timed out waiting for: {what}\nstatus: {:?}", self.app.status.as_ref().map(|s| &s.text)),
            }
        }
    }
    async fn idle(&mut self) {
        self.until("requests settle", |a| a.pending.is_empty()).await;
    }
    fn idea(&self) -> &ideas_tui::types::Idea {
        self.app.current.as_ref().expect("an open idea")
    }
    fn screen(&mut self) -> String {
        let mut t = Terminal::new(TestBackend::new(100, 45)).unwrap();
        t.draw(|f| ui::draw(f, &mut self.app)).unwrap();
        let b = t.backend().buffer();
        (0..b.area.height).map(|y| (0..b.area.width).map(|x| b[(x, y)].symbol()).collect::<String>() + "\n").collect()
    }
}

#[tokio::test]
async fn whole_pipeline_flow() {
    let (server, fake) = fake_server().await;
    let client = Client::with_base(format!("{}/client", server.uri()), TOKEN.into());
    let (tx, rx) = unbounded_channel();
    let mut r = Rig { app: App::new(client, tx), rx };
    r.app.start();
    r.until("initial load", |a| a.loaded && a.projects_loaded).await;
    assert!(r.screen().contains("Nothing here"));

    // 1. Capture, with a multi-line idea.
    r.key('n');
    r.keys("Build a widget");
    r.press(KeyCode::Enter);
    r.keys("that spins");
    r.app.on_key(ctrl_s());
    r.until("idea created", |a| matches!(a.modal, Modal::None) && a.ideas.len() == 1).await;
    assert_eq!(fake.lock().unwrap().idea.as_ref().unwrap()["text"], "Build a widget\nthat spins");

    // 2. Open it and start grilling.
    r.press(KeyCode::Enter);
    assert_eq!(r.app.screen, Screen::Detail);
    r.key('g');
    r.idle().await;
    assert_eq!(r.idea().status, "grilling");
    assert!(r.screen().contains("Who is it for?"));

    // 3. Answer: single -> "My team"; multi -> Linux + Windows + Other text.
    r.key('a');
    r.keys("j "); // q1: My team
    r.press(KeyCode::Tab);
    r.key(' '); // q2: Linux
    r.keys("jj "); // Windows
    r.key('j');
    r.key(' '); // Other on, editing
    r.keys("BSD");
    r.press(KeyCode::Enter);
    r.app.on_key(ctrl_s());
    r.idle().await;
    assert_eq!(
        fake.lock().unwrap().grills[1],
        "Who is it for?\n→ My team\n\nWhich platforms?\n→ Linux; Windows; Other: BSD"
    );

    // 4. An unstructured turn takes a free-text reply.
    assert!(ideas_tui::state::open_turn(r.idea()).is_none());
    r.key('t');
    r.keys("Just me.");
    r.app.on_key(ctrl_s());
    r.idle().await;
    assert_eq!(fake.lock().unwrap().grills[2], "Just me.");
    assert!(grill_done(r.idea()), "done turn offers Write handoff");
    assert!(r.screen().contains("w write handoff"));

    // 5. Write the handoff.
    r.key('w');
    r.until("handoff written", |a| a.current.as_ref().is_some_and(|i| i.handoff.is_some())).await;
    r.idle().await;
    assert_eq!(r.idea().status, "review");

    // 6. Edit the title and the brief, keep the edits locally.
    r.key('e');
    r.keys(" v2"); // title -> "Widget v2"
    r.press(KeyCode::Tab); // project
    r.press(KeyCode::Tab); // visibility
    r.press(KeyCode::Tab); // brief
    r.keys("EDIT: ");
    r.app.on_key(ctrl_s());
    assert_eq!(r.app.drafts["i1"].title, "Widget v2");
    assert!(r.screen().contains("edited locally"));

    // 7. Send to the laptop (confirm required).
    r.key('s');
    r.press(KeyCode::Enter);
    assert!(matches!(r.app.modal, Modal::Confirm(_)));
    assert!(fake.lock().unwrap().sends.is_empty(), "nothing sent before confirming");
    r.key('y');
    r.idle().await;
    {
        let f = fake.lock().unwrap();
        let s = &f.sends[0];
        assert_eq!(s["destination"], "laptop");
        assert_eq!(s["title"], "Widget v2");
        assert_eq!(s["project"], "alpha");
        assert_eq!(s["brief"], "EDIT: Build the widget.\nWith care.");
        assert!(s.get("mode").is_none());
    }
    assert_eq!(r.idea().status, "ready");
    assert!(!can_send(r.idea()), "queued handoffs are locked");
    assert!(r.app.drafts.is_empty());

    // 8. Pull it back and send to the droplet as a preview.
    r.key('u');
    r.idle().await;
    assert_eq!(r.idea().status, "review");
    r.key('s');
    r.key('2');
    r.press(KeyCode::Enter);
    r.key('y');
    r.idle().await;
    assert_eq!(fake.lock().unwrap().sends[1]["mode"], "preview");
    let job = r.idea().job.clone().unwrap();
    assert_eq!(job.status, "preview");
    assert_eq!(job_actions(&job), vec![JobAction::Ship, JobAction::Discard]);
    let screen = r.screen();
    assert!(screen.contains("Preview ready") && screen.contains("https://preview.example"), "{screen}");

    // 9. Rollback isn't offered on a preview; ship is, after confirming.
    r.key('B');
    assert!(r.app.status.as_ref().unwrap().error && matches!(r.app.modal, Modal::None));
    r.key('S');
    assert!(matches!(r.app.modal, Modal::Confirm(_)));
    r.key('y');
    r.idle().await;
    assert_eq!(fake.lock().unwrap().actions, vec!["ship"]);
    assert!(job_actions(r.idea().job.as_ref().unwrap()).is_empty(), "no actions while one is pending");
    assert!(ideas_tui::state::needs_fast_poll(r.idea()), "busy job is polled");
    r.app.update(Msg::Tick);
    // The tick may not poll yet (interval); a manual refresh settles it either way.
    r.key('r');
    r.until("ship settled", |a| a.current.as_ref().is_some_and(|i| i.job.as_ref().is_some_and(|j| j.status == "live"))).await;
    assert_eq!(job_actions(r.idea().job.as_ref().unwrap()), vec![JobAction::Rollback]);

    // 10. Roll back; the handoff unlocks and can go out again, live this time.
    r.key('B');
    r.key('y');
    r.idle().await;
    r.key('r');
    r.until("rollback settled", |a| a.current.as_ref().is_some_and(|i| i.job.as_ref().is_some_and(|j| j.status == "rolled_back"))).await;
    assert_eq!(fake.lock().unwrap().actions, vec!["ship", "rollback"]);
    assert!(can_send(r.idea()), "a droplet job that ended badly unlocks a resend");
    r.key('s');
    r.key('2');
    r.key('v');
    r.press(KeyCode::Enter);
    assert!(matches!(&r.app.modal, Modal::Confirm(c) if c.prompt.contains("LIVE")));
    r.key('y');
    r.idle().await;
    assert_eq!(fake.lock().unwrap().sends[2]["mode"], "live");
    assert_eq!(r.idea().job.as_ref().unwrap().status, "live");

    // 11. Deploy info shows alongside, and a 401 would have been counted.
    r.app.current.as_mut().unwrap().deploy = Some(ideas_tui::types::Deploy {
        status: "failed".into(),
        message: Some("tests failed after deploy".into()),
        log: Some("npm test\nFAIL".into()),
        ..Default::default()
    });
    let screen = r.screen();
    assert!(screen.contains("Deploy failed") && screen.contains("tests failed after deploy"));
    assert_eq!(fake.lock().unwrap().unauthorized, 0, "every request carried the bearer token");
}

#[tokio::test]
async fn bad_token_shows_a_readable_error() {
    let (server, fake) = fake_server().await;
    let client = Client::with_base(format!("{}/client", server.uri()), "wrong".into());
    let (tx, rx) = unbounded_channel();
    let mut r = Rig { app: App::new(client, tx), rx };
    r.app.start();
    r.until("error shown", |a| a.status.as_ref().is_some_and(|s| s.error)).await;
    let text = r.app.status.as_ref().unwrap().text.clone();
    assert!(text.contains("CLIENT_TOKEN") && text.contains("API_PREFIX"), "{text}");
    assert!(fake.lock().unwrap().unauthorized >= 1);
    // The app keeps running.
    r.key('?');
    assert!(r.screen().contains("Keys"));
}
