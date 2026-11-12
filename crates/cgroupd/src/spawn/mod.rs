//! Fork+setuid+exec into delegated app cgroups. The root daemon is the
//! parent; the child is moved into the app cgroup before it may exec.

mod control;
mod exec;
mod hooks;
mod launch;
mod limits;
mod proc;
mod state;

pub(crate) use control::{get_state, stop_target, wait_session};
pub(crate) use launch::{SpawnParams, spawn_app};
pub(crate) use state::drop_state;
