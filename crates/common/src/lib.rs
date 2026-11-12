#![forbid(unsafe_code)]

pub mod client;
pub mod delegate;
pub mod framing;
pub mod paths;
pub mod proto;
pub mod validate;
pub mod varlink;

pub use client::{ClientError, send};
pub use delegate::{delegate_files, is_fatal_delegate};
pub use framing::{FrameError, read_frame};
pub use paths::{
    Roots, app_unit_path, flat_containers, flat_sessions, locate_app, new_app_path, owns_session,
    pid_session, resolve_roots, service_path, session_app_dir, session_dir, session_path,
    unit_path, user_path,
};
pub use proto::{Limit, Request, Response, Scope, StdioSpec};
pub use validate::{
    limit_controller, parse_stdio_spec, resolve_uid, validate_args, validate_cwd, validate_env,
    validate_limit, validate_program, validate_sid, validate_stdio, validate_uid, validate_umask,
};

pub const MAX_MESSAGE_BYTES: usize = 64 * 1024;
pub const DEFAULT_SOCKET_PATH: &str = "/run/cgroupd.sock";
