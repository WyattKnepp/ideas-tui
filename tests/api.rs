//! The typed API client against a mock HTTP server.

use ideas_tui::api::{ApiError, Client};
use ideas_tui::config::Config;
use ideas_tui::types::{JobAction, SendRequest};
use serde_json::json;
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn client(server: &MockServer) -> Client {
    Client::new(&Config { server_url: server.uri(), token: "secret".into(), prefix: "/client".into() })
}

fn ok(body: serde_json::Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(body)
}

#[tokio::test]
async fn sends_bearer_token_under_the_prefix() {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/client/api/ideas"))
        .and(header("authorization", "Bearer secret"))
        .respond_with(ok(json!([{"id": "a", "text": "hi", "status": "inbox", "unknown": 1}, {"id": "b", "status": null}])))
        .expect(1)
        .mount(&s)
        .await;
    let ideas = client(&s).list_ideas().await.unwrap();
    assert_eq!(ideas.len(), 2);
    assert_eq!(ideas[0].text, "hi");
    assert_eq!(ideas[1].status, "");
}

#[tokio::test]
async fn empty_prefix_talks_to_the_root() {
    let s = MockServer::start().await;
    Mock::given(method("GET")).and(path("/api/ideas")).respond_with(ok(json!([]))).expect(1).mount(&s).await;
    let c = Client::new(&Config { server_url: format!("{}/", s.uri()), token: "t".into(), prefix: String::new() });
    assert!(c.list_ideas().await.unwrap().is_empty());
}

#[tokio::test]
async fn errors_are_readable() {
    let s = MockServer::start().await;
    Mock::given(method("GET")).and(path("/client/api/ideas/x"))
        .respond_with(ResponseTemplate::new(401)).mount(&s).await;
    Mock::given(method("DELETE")).and(path("/client/api/ideas/busy"))
        .respond_with(ResponseTemplate::new(409).set_body_json(json!({"error": "Can't delete an idea that is grilling"})))
        .mount(&s).await;
    Mock::given(method("POST")).and(path("/client/api/ideas/boom/handoff"))
        .respond_with(ResponseTemplate::new(500).set_body_json(json!({"error": "claude exploded"}))).mount(&s).await;
    Mock::given(method("GET")).and(path("/client/api/ideas/login"))
        .respond_with(ResponseTemplate::new(302).insert_header("location", "https://accounts.example/login")).mount(&s).await;
    Mock::given(method("GET")).and(path("/client/api/projects"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html>Sign in</html>")).mount(&s).await;
    Mock::given(method("GET")).and(path("/client/api/ideas/proxy"))
        .respond_with(ResponseTemplate::new(502).set_body_string("<html>Bad gateway</html>")).mount(&s).await;

    let c = client(&s);
    let e = c.get_idea("x").await.unwrap_err();
    assert_eq!(e, ApiError::Unauthorized);
    assert!(e.to_string().contains("CLIENT_TOKEN") && e.to_string().contains("API_PREFIX"));

    let e = c.delete_idea("busy").await.unwrap_err();
    assert_eq!(e.status(), Some(409));
    assert_eq!(e.to_string(), "Can't delete an idea that is grilling");

    let e = c.write_handoff("boom").await.unwrap_err();
    assert_eq!(e.to_string(), "Server error (500): claude exploded");

    assert_eq!(c.get_idea("login").await.unwrap_err(), ApiError::Unauthorized, "a login redirect is not followed");
    assert!(matches!(c.projects().await.unwrap_err(), ApiError::Decode(_)));
    assert_eq!(c.get_idea("proxy").await.unwrap_err().to_string(), "Server error (502): Bad Gateway");
}

#[tokio::test]
async fn unreachable_server_is_a_network_error() {
    let c = Client::with_base("http://127.0.0.1:9".into(), "t".into());
    let e = c.list_ideas().await.unwrap_err();
    assert!(matches!(e, ApiError::Network(_)), "{e:?}");
    assert!(e.to_string().starts_with("Can't reach the server"));
}

#[tokio::test]
async fn request_bodies_match_the_contract() {
    let s = MockServer::start().await;
    let idea = json!({"id": "i1", "status": "inbox"});
    for (m, p, body) in [
        ("POST", "/client/api/ideas", json!({"text": "new thing"})),
        ("PATCH", "/client/api/ideas/i1", json!({"archived": true})),
        ("POST", "/client/api/ideas/i1/grill", json!({"message": "my answer"})),
        ("POST", "/client/api/ideas/i1/cancel-grill", json!({})),
        ("POST", "/client/api/ideas/i1/handoff", json!({})),
        ("POST", "/client/api/ideas/i1/unsend", json!({})),
        ("POST", "/client/api/ideas/i1/job/actions", json!({"action": "rollback"})),
        (
            "POST",
            "/client/api/ideas/i1/send",
            json!({"title": "T", "project": "p", "visibility": "private", "brief": "B", "destination": "droplet", "mode": "live"}),
        ),
    ] {
        Mock::given(method(m)).and(path(p)).and(body_json(body)).respond_with(ok(idea.clone())).expect(1).mount(&s).await;
    }
    Mock::given(method("POST")).and(path("/client/api/ideas/i1/elaborate")).and(body_json(json!({"qid": "q2"})))
        .respond_with(ok(json!({"text": "because"}))).expect(1).mount(&s).await;
    Mock::given(method("GET")).and(path("/client/api/projects"))
        .respond_with(ok(json!({"syncedAt": null, "projects": [{"name": "alpha", "description": "d"}, {"name": "beta"}]})))
        .expect(1).mount(&s).await;

    let c = client(&s);
    c.create_idea("new thing").await.unwrap();
    c.set_archived("i1", true).await.unwrap();
    c.grill("i1", "my answer").await.unwrap();
    c.cancel_grill("i1").await.unwrap();
    c.write_handoff("i1").await.unwrap();
    c.unsend("i1").await.unwrap();
    c.job_action("i1", JobAction::Rollback).await.unwrap();
    let req = SendRequest {
        title: "T".into(),
        project: "p".into(),
        visibility: "private".into(),
        brief: "B".into(),
        destination: "droplet".into(),
        mode: Some("live".into()),
    };
    c.send("i1", &req).await.unwrap();
    assert_eq!(c.elaborate("i1", "q2").await.unwrap(), "because");
    assert_eq!(c.projects().await.unwrap(), vec!["alpha", "beta"]);
}

#[tokio::test]
async fn laptop_send_omits_mode() {
    let s = MockServer::start().await;
    Mock::given(method("POST")).and(path("/client/api/ideas/i1/send"))
        .and(body_json(json!({"title": "T", "project": "p", "visibility": "public", "brief": "B", "destination": "laptop"})))
        .respond_with(ok(json!({"id": "i1", "status": "ready"}))).expect(1).mount(&s).await;
    let req = SendRequest {
        title: "T".into(), project: "p".into(), visibility: "public".into(), brief: "B".into(),
        destination: "laptop".into(), mode: None,
    };
    assert_eq!(client(&s).send("i1", &req).await.unwrap().status, "ready");
}

#[tokio::test]
async fn delete_succeeds_on_ok_true() {
    let s = MockServer::start().await;
    Mock::given(method("DELETE")).and(path("/client/api/ideas/i1")).respond_with(ok(json!({"ok": true}))).expect(1).mount(&s).await;
    client(&s).delete_idea("i1").await.unwrap();
}
