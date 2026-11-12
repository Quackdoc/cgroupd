//! `SpawnApp`: turn the D-Bus JSON spec into a `Request`.

use cgroupd_common::Request;

#[derive(serde::Deserialize)]
struct SpawnSpec {
    /// Login session to run in (flat layout); default: the user's only session.
    #[serde(default)]
    sid: Option<String>,
    #[serde(default)]
    program: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: Vec<String>,
    #[serde(default)]
    stdout: Option<String>,
    #[serde(default)]
    stderr: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    umask: Option<u32>,
    #[serde(default)]
    limits: Vec<cgroupd_common::Limit>,
    #[serde(default)]
    pre: Vec<Vec<String>>,
    #[serde(default)]
    post: Vec<Vec<String>>,
}

/// Build the `SpawnApp` request for `aid` from the caller's JSON `spec`.
pub(super) fn spawn_request(
    uid: Option<u32>,
    aid: String,
    spec_json: &str,
) -> Result<Request, String> {
    let spec: SpawnSpec =
        serde_json::from_str(spec_json).map_err(|e| format!("bad spec JSON: {e}"))?;
    let stdio =
        |s: &Option<String>| cgroupd_common::parse_stdio_spec(s.as_deref().unwrap_or("null"));
    Ok(Request::SpawnApp {
        uid,
        sid: spec.sid,
        aid,
        program: spec.program,
        args: spec.args,
        env: spec.env,
        stdout: stdio(&spec.stdout)?,
        stderr: stdio(&spec.stderr)?,
        cwd: spec.cwd,
        umask: spec.umask,
        limits: spec.limits,
        pre: spec.pre,
        post: spec.post,
    })
}
