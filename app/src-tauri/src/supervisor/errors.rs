//! S1-04 review finding 4: every failure reason this crate can expose to
//! the frontend (via `SupervisorState::Failed`) or write to a log is one
//! of a small, fixed, compile-time-enumerated set of `&'static str`
//! values -- never a `format!()`/`.to_string()` of a received message
//! type, field value, file path, HTTP error, or parser exception.
//!
//! `SupervisorState::Failed` only accepts `&'static str` (not `String`),
//! which makes this a *type-level* guarantee, not just a convention: a
//! `format!()` call produces an owned `String`, which does not coerce to
//! `&'static str`, so accidentally interpolating request/child-controlled
//! content into a `Failed` reason is a compile error, not a runtime leak
//! someone has to notice in review.
//!
//! Each module's own internal error types (`process::ProcessError`,
//! `protocol::ProtocolError`, ...) may still carry rich `Display` output
//! (a file path, an I/O error, ...) for *local* debug-build diagnostics
//! (`supervisor::mod::drain_stderr`-style `eprintln!`, `#[cfg(debug_assertions)]`
//! only) -- but every call site that turns one of those into a
//! `SupervisorState`/log-visible reason must go through
//! [`FailureReason`] first, via an explicit `match`, never `.to_string()`.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureReason {
    ExecutableNotFound,
    ResourceDirUnavailable,
    SpawnFailed,
    JobObjectCreateFailed,
    JobObjectAssignFailed,
    ChildStdinUnavailable,
    ChildStdoutUnavailable,
    PrivateChannelWriteFailed,
    PrivateChannelReadFailed,
    ChildExitedDuringStartup,
    ChildExitedUnexpectedly,
    EndpointNotLoopback,
    HttpClientBuildFailed,
    ChallengeRequestFailed,
    ChallengeResponseRejected,
    ChallengeResponseMalformed,
    ChallengeResponseTooLarge,
    ChallengeVerificationFailed,
    StartupTimeout,
}

impl FailureReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ExecutableNotFound => "executable_not_found",
            Self::ResourceDirUnavailable => "resource_dir_unavailable",
            Self::SpawnFailed => "spawn_failed",
            Self::JobObjectCreateFailed => "job_object_create_failed",
            Self::JobObjectAssignFailed => "job_object_assign_failed",
            Self::ChildStdinUnavailable => "child_stdin_unavailable",
            Self::ChildStdoutUnavailable => "child_stdout_unavailable",
            Self::PrivateChannelWriteFailed => "private_channel_write_failed",
            Self::PrivateChannelReadFailed => "private_channel_read_failed",
            Self::ChildExitedDuringStartup => "child_exited_during_startup",
            Self::ChildExitedUnexpectedly => "child_exited_unexpectedly",
            Self::EndpointNotLoopback => "endpoint_not_loopback",
            Self::HttpClientBuildFailed => "http_client_build_failed",
            Self::ChallengeRequestFailed => "challenge_request_failed",
            Self::ChallengeResponseRejected => "challenge_response_rejected",
            Self::ChallengeResponseMalformed => "challenge_response_malformed",
            Self::ChallengeResponseTooLarge => "challenge_response_too_large",
            Self::ChallengeVerificationFailed => "challenge_verification_failed",
            Self::StartupTimeout => "startup_timeout",
        }
    }
}

impl std::fmt::Display for FailureReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The equally-bounded set of reasons [`crate::check_service_health`] (and
/// its Rust-side counterpart in this module) can fail with -- kept
/// separate from [`FailureReason`] because these describe a *request*
/// outcome, not a lifecycle/startup outcome, and are returned directly as
/// a `#[tauri::command]` `Err(String)` rather than stored in
/// `SupervisorState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthCheckError {
    ServiceNotReady,
    ServiceNoLongerAlive,
    RequestFailed,
    UnexpectedStatus,
    ResponseTooLarge,
    ResponseMalformed,
}

impl HealthCheckError {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ServiceNotReady => "service_not_ready",
            Self::ServiceNoLongerAlive => "service_no_longer_alive",
            Self::RequestFailed => "health_request_failed",
            Self::UnexpectedStatus => "health_request_unexpected_status",
            Self::ResponseTooLarge => "health_response_too_large",
            Self::ResponseMalformed => "health_response_malformed",
        }
    }
}

impl std::fmt::Display for HealthCheckError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
