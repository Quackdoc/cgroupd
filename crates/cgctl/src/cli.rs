use usage_rs::{Args, Cli, Subcommands};

/// cgctl — client for cgroupd.
#[derive(Cli)]
#[usage(bin = "cgctl", version, about = "Client for cgroupd")]
pub(crate) struct Ctl {
    /// Socket path.
    #[usage(long, global, value_name = "PATH")]
    pub(crate) socket: Option<String>,

    #[usage(subcommand)]
    pub(crate) command: Commands,
}

#[derive(Subcommands)]
pub(crate) enum Commands {
    /// Ensure user slice exists.
    EnsureUser(EnsureUser),
    /// Create a session.
    CreateSession(CreateSession),
    /// Remove a session.
    RemoveSession(RemoveSession),
    /// List login sessions for a user.
    ListSessions(ListSessions),
    /// List spawned apps for a user.
    ListApps(ListApps),
    /// Spawn a program as a transient app.
    Spawn(Box<Spawn>),
    /// Stop a session or app.
    Stop(Stop),
    /// Show session/app state.
    State(State),
    /// Wait for a session/app.
    Wait(Wait),
    /// List pids in a session/app.
    Ps(Ps),
    /// Join a cgroup (write own pid to cgroup.procs).
    Enter(Enter),
    /// Run a program attached in a fresh app cgroup (like systemd-run --scope).
    Scope(ScopeRun),
    /// PAM session hook for pam_exec.so (always exits 0).
    Pam,
}

#[derive(Args)]
pub(crate) struct EnsureUser {
    /// Act as UID (root only; otherwise daemon resolves peer via SO_PEERCRED).
    #[usage(long = "as", value_name = "UID")]
    pub(crate) as_uid: Option<u32>,
}

#[derive(Args)]
pub(crate) struct CreateSession {
    /// Session id (or app id with --app, service name with --service).
    pub(crate) sid: String,
    /// Operate on an app instead of a session.
    #[usage(long = "app")]
    pub(crate) app: bool,
    /// Operate on a per-user service (`user@<uid>/services/<name>`) instead.
    #[usage(long = "service")]
    pub(crate) service: bool,
    /// PID to associate (defaults to own pid).
    #[usage(long, value_name = "PID")]
    pub(crate) pid: Option<u32>,
    /// Act as UID (root only).
    #[usage(long = "as", value_name = "UID")]
    pub(crate) as_uid: Option<u32>,
}

#[derive(Args)]
pub(crate) struct RemoveSession {
    /// Session id (or app id with --app, service name with --service).
    pub(crate) sid: String,
    /// Operate on an app instead of a session.
    #[usage(long = "app")]
    pub(crate) app: bool,
    /// Operate on a per-user service (`user@<uid>/services/<name>`) instead.
    #[usage(long = "service")]
    pub(crate) service: bool,
    /// Act as UID (root only).
    #[usage(long = "as", value_name = "UID")]
    pub(crate) as_uid: Option<u32>,
}

#[derive(Args)]
pub(crate) struct ListSessions {
    /// Act as UID (root only).
    #[usage(long = "as", value_name = "UID")]
    pub(crate) as_uid: Option<u32>,
}

#[derive(Args)]
pub(crate) struct ListApps {
    /// Act as UID (root only).
    #[usage(long = "as", value_name = "UID")]
    pub(crate) as_uid: Option<u32>,
}

#[derive(Args)]
pub(crate) struct Spawn {
    /// App id.
    pub(crate) aid: String,
    /// Environment entry KEY=VAL (repeatable).
    #[usage(long, value_name = "K=V")]
    pub(crate) env: Vec<String>,
    /// Stdout spec: null|inherit|file:/p|append:/p|truncate:/p.
    #[usage(long, value_name = "SPEC")]
    pub(crate) stdout: Option<String>,
    /// Stderr spec: null|inherit|file:/p|append:/p|truncate:/p.
    #[usage(long, value_name = "SPEC")]
    pub(crate) stderr: Option<String>,
    /// Working directory (absolute path).
    #[usage(long, value_name = "DIR")]
    pub(crate) cwd: Option<String>,
    /// Umask in octal (e.g. 022).
    #[usage(long, value_name = "OCT")]
    pub(crate) umask: Option<String>,
    /// Limit FILE=VALUE (repeatable).
    #[usage(long, value_name = "FILE=VALUE")]
    pub(crate) limit: Vec<String>,
    /// Pre command, whitespace-split (repeatable).
    #[usage(long, value_name = "CMD")]
    pub(crate) pre: Vec<String>,
    /// Post command, whitespace-split (repeatable).
    #[usage(long, value_name = "CMD")]
    pub(crate) post: Vec<String>,
    /// Login session to run in (default: the session this command runs in).
    #[usage(long, value_name = "SID")]
    pub(crate) session: Option<String>,
    /// Act as UID (root only).
    #[usage(long = "as", value_name = "UID")]
    pub(crate) as_uid: Option<u32>,
    /// Program and args after `--`.
    #[usage(arg, required, double_dash = "required", value_name = "PROGRAM")]
    pub(crate) program: Vec<String>,
}

#[derive(Args)]
pub(crate) struct Stop {
    /// Session id (or app id with --app, service name with --service).
    pub(crate) sid: String,
    /// Operate on an app instead of a session.
    #[usage(long = "app")]
    pub(crate) app: bool,
    /// Grace period in seconds.
    #[usage(long, default = "2", value_name = "SECS")]
    pub(crate) grace: u64,
    /// Act as UID (root only).
    #[usage(long = "as", value_name = "UID")]
    pub(crate) as_uid: Option<u32>,
}

#[derive(Args)]
pub(crate) struct State {
    /// Session id (or app id with --app, service name with --service).
    pub(crate) sid: String,
    /// Operate on an app instead of a session.
    #[usage(long = "app")]
    pub(crate) app: bool,
    /// Act as UID (root only).
    #[usage(long = "as", value_name = "UID")]
    pub(crate) as_uid: Option<u32>,
}

#[derive(Args)]
pub(crate) struct Wait {
    /// Session id (or app id with --app, service name with --service).
    pub(crate) sid: String,
    /// Operate on an app instead of a session.
    #[usage(long = "app")]
    pub(crate) app: bool,
    /// Timeout in seconds.
    #[usage(long, default = "30", value_name = "SECS")]
    pub(crate) timeout: u64,
    /// Act as UID (root only).
    #[usage(long = "as", value_name = "UID")]
    pub(crate) as_uid: Option<u32>,
}

#[derive(Args)]
pub(crate) struct Ps {
    /// Session id (or app id with --app, service name with --service).
    pub(crate) sid: String,
    /// Operate on an app instead of a session.
    #[usage(long = "app")]
    pub(crate) app: bool,
    /// Act as UID (root only).
    #[usage(long = "as", value_name = "UID")]
    pub(crate) as_uid: Option<u32>,
}

#[derive(Args)]
pub(crate) struct Enter {
    /// Cgroup path to join.
    pub(crate) path: String,
}

#[derive(Args)]
pub(crate) struct ScopeRun {
    /// App id (default: app-<prog>-<pid>).
    #[usage(long, value_name = "AID")]
    pub(crate) aid: Option<String>,
    /// Act as UID (root only).
    #[usage(long = "as", value_name = "UID")]
    pub(crate) as_uid: Option<u32>,
    /// Program and args after `--`.
    #[usage(arg, required, double_dash = "required", value_name = "PROGRAM")]
    pub(crate) program: Vec<String>,
}
