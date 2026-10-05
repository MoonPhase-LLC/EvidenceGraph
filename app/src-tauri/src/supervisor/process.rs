//! Child process resolution and spawning (S1-04 requirement 1). Never
//! resolves through PATH, never invokes a shell, never uses `shell:open`/
//! `cmd /C`/PowerShell/`uv run`.

use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::process::{Child, Command};

#[derive(Debug)]
pub enum ProcessError {
    ExecutableNotFound(PathBuf),
    // Only constructed by `resolve_production`, which is itself
    // `#[cfg(not(debug_assertions))]` -- genuinely dead code in a dev/
    // debug build (including `cargo clippy`'s default profile), not a
    // real "unused" mistake; real in a release build.
    #[cfg_attr(debug_assertions, allow(dead_code))]
    ResourceDirUnavailable,
    SpawnFailed(std::io::Error),
}

impl fmt::Display for ProcessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ExecutableNotFound(path) => {
                write!(
                    f,
                    "expected service executable not found: {}",
                    path.display()
                )
            }
            Self::ResourceDirUnavailable => write!(f, "could not resolve app resource directory"),
            Self::SpawnFailed(e) => write!(f, "failed to spawn service process: {e}"),
        }
    }
}

impl std::error::Error for ProcessError {}

/// Resolves the absolute path to the service executable/interpreter this
/// process should launch, and the argv to pass it. Exactly two resolution
/// strategies exist -- see [`resolve_dev`] and [`resolve_production`] --
/// selected at *compile time* by `debug_assertions`, never at runtime by
/// an environment variable or config value an attacker (or a packaging
/// mistake) could influence.
pub fn resolve(app: &tauri::AppHandle) -> Result<(PathBuf, Vec<String>), ProcessError> {
    #[cfg(debug_assertions)]
    {
        let _ = app; // unused in dev resolution; kept for a uniform signature
        resolve_dev()
    }
    #[cfg(not(debug_assertions))]
    {
        resolve_production(app)
    }
}

/// **Development only.** This function does not exist in a release binary
/// at all (`#[cfg(debug_assertions)]`), so it can never become a
/// production PATH fallback -- the requirement that a dev-only resolver
/// "cannot silently become a production PATH fallback" is enforced by the
/// compiler, not by convention.
///
/// Resolves to *this workspace's own* `service/.venv` interpreter via
/// `CARGO_MANIFEST_DIR`, a compile-time constant baked into the dev
/// binary (the absolute path to `app/src-tauri` at build time) -- not
/// PATH, not a runtime environment variable, not a shell. Requires `uv
/// sync` to have already been run in `service/` (documented in
/// `service/README.md` and this crate's own README), same as every other
/// S1-03 manual-testing prerequisite.
/// `pub(super)`, not private: the integration tests in `supervisor`'s own
/// test suite call this directly to get a real, working child target
/// without needing a mock `AppHandle` (dev resolution ignores it anyway).
#[cfg(debug_assertions)]
pub(super) fn resolve_dev() -> Result<(PathBuf, Vec<String>), ProcessError> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let venv_bin = if cfg!(windows) { "Scripts" } else { "bin" };
    let python_name = if cfg!(windows) {
        "python.exe"
    } else {
        "python"
    };
    let python = manifest_dir
        .join("..")
        .join("..")
        .join("service")
        .join(".venv")
        .join(venv_bin)
        .join(python_name);

    if !python.is_file() {
        return Err(ProcessError::ExecutableNotFound(python));
    }

    Ok((
        python,
        vec![
            "-m".to_string(),
            "evidencegraph_service".to_string(),
            "--supervised".to_string(),
        ],
    ))
}

/// **Production.** The bundled sidecar executable S1-09's packaging step
/// must place at `<resource_dir>/service/evidencegraph-service[.exe]` --
/// via `tauri.conf.json` `bundle.resources` (plain file copying into the
/// app's resource directory), deliberately *not* Tauri's `externalBin`/
/// sidecar-plugin mechanism, so no `tauri-plugin-shell` dependency or
/// ACL/capability entry is needed anywhere in this app for the frontend
/// to be exposed to (it never is -- this spawn happens entirely in
/// backend `setup()` code, invoked as an ordinary absolute-path child
/// process). S1-09 owns actually producing that executable; S1-04 (this
/// function) only implements resolving it and failing closed if it's
/// missing -- until S1-09 lands, every release build correctly, expectedly
/// fails closed here.
#[cfg(not(debug_assertions))]
fn resolve_production(app: &tauri::AppHandle) -> Result<(PathBuf, Vec<String>), ProcessError> {
    use tauri::Manager;

    let resource_dir = app
        .path()
        .resource_dir()
        .map_err(|_| ProcessError::ResourceDirUnavailable)?;
    let exe_name = if cfg!(windows) {
        "evidencegraph-service.exe"
    } else {
        "evidencegraph-service"
    };
    let exe_path = resource_dir.join("service").join(exe_name);

    if !exe_path.is_file() {
        return Err(ProcessError::ExecutableNotFound(exe_path));
    }

    Ok((exe_path, vec!["--supervised".to_string()]))
}

/// Spawns `executable` directly (never through a shell, never PATH-
/// resolved -- `executable` must already be an absolute path from
/// [`resolve`]) with piped stdio for the private channel, and
/// `kill_on_drop` as a first, cheap process-guard layer: if this `Child`
/// handle is ever dropped without an explicit graceful/forced shutdown
/// (e.g. an early return, a panic unwinding past it), tokio sends a kill
/// rather than silently leaking the child. The Windows Job Object guard
/// (`job_object.rs`) is the second, stronger layer, covering this whole
/// process terminating abnormally, not just this handle being dropped.
pub fn spawn(executable: &Path, args: &[String]) -> Result<Child, ProcessError> {
    let mut command = Command::new(executable);
    command
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    #[cfg(windows)]
    {
        // `tokio::process::Command::creation_flags` (Windows-only) wraps
        // the same std `CommandExt` method internally -- no extra trait
        // import needed here. Prevents Windows from allocating a console
        // window for this
        // console-subsystem child, in *both* dev and release builds --
        // "production release mode must not open an additional console
        // window" (S1-04 requirement 7). There's no reason to want one in
        // dev either: all three standard streams are already piped, and
        // stderr is drained and relayed by this process instead
        // (`supervisor::drain_stderr`).
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    command.spawn().map_err(ProcessError::SpawnFailed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(debug_assertions)]
    #[test]
    fn dev_resolution_finds_the_real_workspace_venv_python() {
        // This is a real filesystem check, not a mock -- it only passes
        // if `uv sync` has actually been run in `service/`, same
        // prerequisite as every other S1-03/S1-04 manual test.
        let (python, args) = resolve_dev().expect(
            "service/.venv python not found -- run `uv sync` in service/ first \
             (see service/README.md)",
        );
        assert!(python.is_absolute());
        assert!(python.is_file());
        assert_eq!(args, vec!["-m", "evidencegraph_service", "--supervised"]);
    }
}
