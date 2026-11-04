//! Wire types. Deliberately lenient: every field is optional or defaulted and
//! `null` is accepted wherever a default exists, so a server that adds,
//! drops or nulls a field never crashes the client.

use serde::{Deserialize, Deserializer, Serialize};

/// `null` or missing -> `T::default()`.
fn nz<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct Idea {
    #[serde(deserialize_with = "nz")]
    pub id: String,
    #[serde(deserialize_with = "nz")]
    pub text: String,
    #[serde(deserialize_with = "nz")]
    pub status: String,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    #[serde(deserialize_with = "nz")]
    pub messages: Vec<Message>,
    pub handoff: Option<Handoff>,
    pub dispatch: Option<Dispatch>,
    pub deploy: Option<Deploy>,
    pub job: Option<Job>,
    #[serde(deserialize_with = "nz")]
    pub busy: bool,
    pub grill_session: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Message {
    #[serde(deserialize_with = "nz")]
    pub role: String,
    #[serde(deserialize_with = "nz")]
    pub text: String,
    pub at: Option<String>,
    pub questions: Option<Vec<Question>>,
    pub done: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Question {
    #[serde(deserialize_with = "nz")]
    pub id: String,
    #[serde(deserialize_with = "nz")]
    pub text: String,
    /// `single` or `multi`.
    #[serde(rename = "type", deserialize_with = "nz")]
    pub kind: String,
    #[serde(deserialize_with = "nz")]
    pub options: Vec<String>,
}

impl Question {
    pub fn is_multi(&self) -> bool {
        self.kind == "multi"
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct Handoff {
    #[serde(deserialize_with = "nz")]
    pub title: String,
    #[serde(deserialize_with = "nz")]
    pub project: String,
    #[serde(deserialize_with = "nz")]
    pub is_new: bool,
    /// `private` or `public`.
    #[serde(deserialize_with = "nz")]
    pub visibility: String,
    #[serde(deserialize_with = "nz")]
    pub brief: String,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct Dispatch {
    pub destination: Option<String>,
    pub job_id: Option<i64>,
    pub mode: Option<String>,
    pub error: Option<String>,
    pub workspace_label: Option<String>,
    pub tab_label: Option<String>,
    pub agent: Option<String>,
    pub note: Option<String>,
    pub at: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Deploy {
    /// `ok` or `failed`.
    #[serde(deserialize_with = "nz")]
    pub status: String,
    pub commit: Option<String>,
    pub message: Option<String>,
    pub log: Option<String>,
    pub at: Option<String>,
}

impl Deploy {
    pub fn ok(&self) -> bool {
        self.status == "ok"
    }
}

/// The droplet job summary carried on an idea (snake_case on the wire).
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Job {
    pub id: Option<i64>,
    #[serde(deserialize_with = "nz")]
    pub status: String,
    pub mode: Option<String>,
    pub note: Option<String>,
    pub branch: Option<String>,
    pub commit: Option<String>,
    pub preview_url: Option<String>,
    /// A requested but unfinished action, e.g. `{type: "ship"}`.
    pub action: Option<JobPending>,
    #[serde(deserialize_with = "nz")]
    pub events: Vec<JobEvent>,
    pub tests: Option<JobTests>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct JobPending {
    #[serde(rename = "type", deserialize_with = "nz")]
    pub kind: String,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct JobEvent {
    pub at: Option<String>,
    pub status: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct JobTests {
    pub ok: Option<bool>,
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Projects {
    #[serde(deserialize_with = "nz")]
    pub projects: Vec<Project>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Project {
    #[serde(deserialize_with = "nz")]
    pub name: String,
    pub description: Option<String>,
}

/// Body of `POST .../send`.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SendRequest {
    pub title: String,
    pub project: String,
    pub visibility: String,
    pub brief: String,
    pub destination: String,
    /// `preview` or `live`; only sent for the droplet.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobAction {
    Ship,
    Discard,
    Rollback,
}

impl JobAction {
    pub fn as_str(self) -> &'static str {
        match self {
            JobAction::Ship => "ship",
            JobAction::Discard => "discard",
            JobAction::Rollback => "rollback",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tolerates_nulls_and_unknown_fields() {
        let i: Idea = serde_json::from_str(
            r#"{"id":"a1","text":null,"status":"inbox","messages":null,"mystery":[1,2],
                "handoff":{"title":"T","project":"p","isNew":null,"brief":"b","extra":1},
                "job":{"id":3,"status":"preview","action":null,"events":null}}"#,
        )
        .unwrap();
        assert_eq!(i.id, "a1");
        assert_eq!(i.text, "");
        assert!(i.messages.is_empty());
        assert!(!i.handoff.unwrap().is_new);
        assert_eq!(i.job.unwrap().status, "preview");
    }

    #[test]
    fn parses_structured_turn() {
        let m: Message = serde_json::from_str(
            r#"{"role":"claude","text":"hi","questions":[{"id":"q1","text":"Which?","type":"multi","options":["a","b"]}],"done":false}"#,
        )
        .unwrap();
        let q = &m.questions.unwrap()[0];
        assert!(q.is_multi());
        assert_eq!(q.options, vec!["a", "b"]);
    }
}
