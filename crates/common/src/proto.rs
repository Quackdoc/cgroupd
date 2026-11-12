use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "path", rename_all = "snake_case")]
pub enum StdioSpec {
    Null,
    Inherit,
    File(String),
    Append(String),
    Truncate(String),
}

/// Client-chosen controller limit: bare file name (`cpu.weight`), never a path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limit {
    pub file: String,
    pub value: String,
}

/// Which cgroup subtree an id lives in: login sessions (`session/<sid>`,
/// cf. logind session scopes) or spawned apps (`app/<aid>`, cf. app.slice).
/// Defaults to `Session` so pre-scope clients keep working.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    #[default]
    Session,
    App,
    /// A long-running per-user service (`user@<uid>/services/<name>`), outside any
    /// login session so it survives logout.
    Service,
}

impl Scope {
    /// Human noun for messages (`session`/`app`).
    pub fn noun(self) -> &'static str {
        match self {
            Scope::Session => "session",
            Scope::App => "app",
            Scope::Service => "service",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
pub enum Request {
    /// `uid: None` means "caller" (resolved from SO_PEERCRED / bus peer).
    /// `Some(t)` is honored only when the peer is root (uid 0); otherwise
    /// it must equal the peer or the request is denied.
    EnsureUser {
        #[serde(default)]
        uid: Option<u32>,
    },
    CreateSession {
        #[serde(default)]
        uid: Option<u32>,
        #[serde(default)]
        scope: Scope,
        sid: String,
        pid: u32,
    },
    RemoveSession {
        #[serde(default)]
        uid: Option<u32>,
        #[serde(default)]
        scope: Scope,
        sid: String,
    },
    ListSessions {
        #[serde(default)]
        uid: Option<u32>,
    },
    ListApps {
        #[serde(default)]
        uid: Option<u32>,
    },
    SpawnApp {
        #[serde(default)]
        uid: Option<u32>,
        /// Login session to run the app in (flat layout); default: the user's only one.
        #[serde(default)]
        sid: Option<String>,
        aid: String,
        program: String,
        args: Vec<String>,
        env: Vec<String>,
        stdout: StdioSpec,
        stderr: StdioSpec,
        cwd: Option<String>,
        umask: Option<u32>,
        limits: Vec<Limit>,
        pre: Vec<Vec<String>>,
        post: Vec<Vec<String>>,
    },
    StopSession {
        #[serde(default)]
        uid: Option<u32>,
        #[serde(default)]
        scope: Scope,
        sid: String,
        grace_secs: u64,
    },
    GetState {
        #[serde(default)]
        uid: Option<u32>,
        #[serde(default)]
        scope: Scope,
        sid: String,
    },
    WaitSession {
        #[serde(default)]
        uid: Option<u32>,
        #[serde(default)]
        scope: Scope,
        sid: String,
        timeout_secs: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum Response {
    Ok { detail: String },
    Err { msg: String },
}

impl Response {
    pub fn ok(detail: impl Into<String>) -> Self {
        Response::Ok {
            detail: detail.into(),
        }
    }

    pub fn err(msg: impl Into<String>) -> Self {
        Response::Err { msg: msg.into() }
    }

    pub fn is_ok(&self) -> bool {
        matches!(self, Response::Ok { .. })
    }
}
