//! Sidecar process lifecycle for the narration engine: spawn, health-check,
//! Windows Job Object crash-protection, and guaranteed teardown. Talks to
//! `narration.rs` for the HTTP protocol itself (build_server_args, health_check,
//! synthesize) — this module only owns "is there a running process and is it
//! healthy," never the HTTP details. Windows-only (macOS out of scope for
//! Phase 1); a later task provides the non-Windows Tauri-command stub.

use crate::narration::{build_server_args, health_check, synthesize, NarrationError, NarrationParams};
use crate::narration_provision::get_venv_dir;
use std::path::Path;
use std::time::Duration;

#[cfg(target_os = "windows")]
use win32job::{ExtendedLimitInfo, Job};

/// A running (or adopted) narration sidecar. `Owned` means this process spawned
/// it and holds the handles needed to kill it; `Unmanaged` means it was already
/// healthily running when we checked (our own earlier orphan, or a startup
/// race) — usable, but this instance has no way to kill it. This asymmetry is
/// deliberate: see `synthesize_with_restart`'s doc comment for why an
/// `Unmanaged` failure is never retried by respawning on the same port.
#[allow(clippy::large_enum_variant)] // singleton held in Mutex<Option<…>> in AppState — boxing Owned would add indirection for zero benefit
pub enum NarrationServer {
    Owned {
        child: tokio::process::Child,
        #[cfg(target_os = "windows")]
        _job: Job,
        port: u16,
        /// Remembered so a restart respawns the SAME voice the user picked,
        /// rather than silently reverting to the default mid-session.
        voice_id: String,
        client: reqwest::Client,
    },
    Unmanaged {
        port: u16,
        client: reqwest::Client,
    },
}

impl NarrationServer {
    fn port(&self) -> u16 {
        match self {
            NarrationServer::Owned { port, .. } => *port,
            NarrationServer::Unmanaged { port, .. } => *port,
        }
    }

    fn client(&self) -> &reqwest::Client {
        match self {
            NarrationServer::Owned { client, .. } => client,
            NarrationServer::Unmanaged { client, .. } => client,
        }
    }

    /// Get a running sidecar on `port`: adopt an already-healthy one if
    /// present, otherwise spawn a fresh one and wait for it to become ready.
    /// This is both the normal "start the engine" path AND the stale-sweep
    /// concept from the design — there is deliberately no separate sweep
    /// function; see the module-level design note above.
    pub async fn spawn_or_adopt(port: u16, voice_id: &str) -> Result<Self, String> {
        let client = reqwest::Client::new();
        if health_check(&client, port).await {
            return Ok(NarrationServer::Unmanaged { port, client });
        }
        Self::spawn_owned(port, voice_id, client).await
    }

    async fn spawn_owned(
        port: u16,
        voice_id: &str,
        client: reqwest::Client,
    ) -> Result<Self, String> {
        let venv_dir = get_venv_dir();
        let python_exe = venv_dir.join("Scripts").join("python.exe");
        if !python_exe.exists() {
            return Err(format!(
                "מנוע הקריינות לא מותקן כראוי — python.exe לא נמצא בנתיב הצפוי ({}).",
                python_exe.display()
            ));
        }

        let script = crate::narration_provision::get_server_script_path();
        let voice = crate::narration_provision::get_voice_path(voice_id);
        let config = crate::narration_provision::get_voice_config_path();
        let phonikud = crate::narration_provision::get_phonikud_path();
        let tokenizer = crate::narration_provision::get_tokenizer_path();
        let paths = crate::narration::ServerPaths {
            script: &script.to_string_lossy(),
            voice: &voice.to_string_lossy(),
            config: &config.to_string_lossy(),
            phonikud: &phonikud.to_string_lossy(),
            tokenizer: &tokenizer.to_string_lossy(),
        };
        let args = build_server_args(&paths, "127.0.0.1", port);

        // Pin the child's CWD to the engine directory. Paths are absolute so
        // this is not strictly required today, but the previous engine
        // resolved its voice relative to CWD and failed in any real
        // deployment because of it — keeping this makes that class of bug
        // impossible to reintroduce.
        let mut child = tokio::process::Command::new(&python_exe)
            .args(&args)
            .current_dir(crate::narration_provision::get_narration_dir())
            .kill_on_drop(true) // layer (a): clean-exit teardown
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| format!("הפעלת מנוע הקריינות נכשלה. (פרטים טכניים: {e})"))?;

        // Layer (b): Windows Job Object with KILL_ON_JOB_CLOSE — the sidecar
        // dies with this app even on a hard crash/taskkill, not just clean exit.
        #[cfg(target_os = "windows")]
        let job = {
            let mut info = ExtendedLimitInfo::new();
            info.limit_kill_on_job_close();
            let job = Job::create_with_limit_info(&info)
                .map_err(|e| format!("יצירת הגנת תהליך למנוע הקריינות נכשלה. (פרטים טכניים: {e})"))?;
            let handle = child
                .raw_handle()
                .ok_or_else(|| "מנוע הקריינות יצא מיד לאחר ההפעלה.".to_string())?
                as isize;
            job.assign_process(handle)
                .map_err(|e| format!("שיוך מנוע הקריינות להגנת התהליך נכשל. (פרטים טכניים: {e})"))?;
            job
        };

        // Poll /info until healthy or a 30s ceiling — cold start (loading the
        // diacritizer and the ONNX voice) is real wall-clock time, not instant.
        // Also check whether the child has already exited on each iteration:
        // a crash (e.g. a missing/misconfigured voice) fails fast, and without
        // this check the loop would burn the full 30s ceiling polling a dead
        // port before reporting a misleading "didn't respond in time" error
        // instead of the real cause.
        let mut attempts = 0;
        loop {
            if health_check(&client, port).await {
                break;
            }
            if let Ok(Some(status)) = child.try_wait() {
                return Err(format!(
                    "מנוע הקריינות קרס מיד לאחר ההפעלה (קוד יציאה: {status}). ודא שהמנוע הותקן כראוי."
                ));
            }
            attempts += 1;
            if attempts > 60 {
                let _ = child.kill().await;
                return Err("מנוע הקריינות לא הגיב בזמן סביר. נסה שוב.".to_string());
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }

        Ok(NarrationServer::Owned {
            child,
            #[cfg(target_os = "windows")]
            _job: job,
            port,
            voice_id: voice_id.to_string(),
            client,
        })
    }

    /// Generate audio for `text`. On failure, restart once — but **only when
    /// `self` is `Owned`**. An `Unmanaged` failure is NOT retried this way:
    /// we don't know whether the adopted process is actually dead or just
    /// returned one flaky response, `shutdown()` is a no-op for `Unmanaged`
    /// (nothing to kill), and `spawn_owned`'s readiness poll can't tell "my
    /// new process answered" from "the still-alive adopted process answered"
    /// on the same port. If the adopted process is actually still alive, a
    /// blind respawn attempt would silently create a second process bound to
    /// the same port, orphan the tracked (new, likely dead-on-bind) one, and
    /// leave the real orphan permanently untracked — exactly what
    /// `spawn_or_adopt`'s adopt-instead-of-kill design was trying to avoid.
    /// So an `Unmanaged` failure just surfaces the error; the caller can
    /// re-run `spawn_or_adopt` from scratch, which correctly re-probes health.
    ///
    /// Restarting is also gated on the error KIND, not merely on failure: only
    /// `Unreachable` implies the process may be dead.
    /// On `Err` from a failed respawn attempt, `self` retains the `Owned`
    /// variant with a dead child; the next call will retry the respawn
    /// attempt (shutdown on a dead child is a safe no-op).
    pub async fn synthesize_with_restart(
        &mut self,
        text: &str,
        params: NarrationParams,
    ) -> Result<Vec<u8>, NarrationError> {
        let first_error = match synthesize(self.client(), self.port(), text, params).await {
            Ok(bytes) => return Ok(bytes),
            Err(e) => e,
        };

        // Restart ONLY when the error suggests the process is gone. A 4xx/5xx
        // proves the opposite — narration_server.py catches synthesis errors
        // and deliberately keeps serving — and a timeout usually just means
        // the text was long. Restarting on those killed a healthy engine, paid
        // a full cold start (re-loading the 308MB diacritizer), retried the
        // same input and failed identically, turning one error into two.
        if !matches!(first_error, NarrationError::Unreachable(_)) {
            return Err(first_error);
        }

        let (port, voice_id) = match self {
            NarrationServer::Owned { port, voice_id, .. } => (*port, voice_id.clone()),
            // Surface the real error rather than a hardcoded string: with
            // stdout/stderr piped to null, this is the only diagnostic the
            // user or a bug report will ever see.
            NarrationServer::Unmanaged { .. } => return Err(first_error),
        };

        self.shutdown().await;

        match Self::spawn_owned(port, &voice_id, reqwest::Client::new()).await {
            Ok(restarted) => {
                let result =
                    synthesize(restarted.client(), restarted.port(), text, params).await;
                *self = restarted;
                result
            }
            Err(e) => Err(NarrationError::Unreachable(e)),
        }
    }

    /// Kill the process if we own it. A no-op for `Unmanaged` — there is
    /// nothing to kill (see the module design note and `synthesize_with_restart`).
    /// Waits for the OS to actually reap the process after `kill()` — `kill()`
    /// alone only sends `TerminateProcess` and returns once that syscall
    /// completes, not once the process (and the port it held) is released.
    /// Without the wait, a caller that immediately tries to bind the same
    /// port (as `synthesize_with_restart`'s respawn does) races the OS
    /// teardown.
    pub async fn shutdown(&mut self) {
        let NarrationServer::Owned { child, .. } = self else {
            return;
        };

        // Capture the process tree BEFORE the kill. The venv's `python.exe` is
        // a shim that re-execs into the real interpreter under `python\`, and
        // THAT grandchild is the process which actually binds the port and
        // holds the model files open — `child.kill()` alone never touched it.
        // It only died later, when dropping `Owned` closed the Job Object; in
        // `synthesize_with_restart` that drop happens AFTER the respawn, so
        // the "restarted" sidecar could health-check the surviving old process,
        // conclude it had come up, and keep a dead child handle instead.
        let tree = child
            .id()
            .map(|pid| descendants_of(&refreshed_system(), pid))
            .unwrap_or_default();

        let _ = child.kill().await;
        let _ = child.wait().await;

        if tree.is_empty() {
            return;
        }
        let root = crate::narration_provision::get_narration_dir();
        for _ in 0..25 {
            let sys = refreshed_system();
            // Intersect the captured tree with "still running out of our own
            // directory". Windows recycles PIDs, and a captured PID could by
            // then belong to something unrelated — this makes killing a
            // stranger impossible rather than merely unlikely.
            let alive: Vec<_> = processes_under(&sys, &root)
                .into_iter()
                .filter(|p| tree.contains(&p.pid))
                .collect();
            if alive.is_empty() {
                return;
            }
            for stale in alive {
                if let Some(p) = sys.process(sysinfo::Pid::from_u32(stale.pid)) {
                    p.kill();
                }
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
}

/// A live process running out of the narration directory. Carries the exe
/// path, not just the PID: an error message naming a bare number tells the
/// user nothing they can act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaleProcess {
    pub pid: u32,
    pub exe: String,
}

impl std::fmt::Display for StaleProcess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (PID {})", self.exe, self.pid)
    }
}

/// True when `candidate` sits inside `root`.
///
/// Compares whole path COMPONENTS, case-insensitively. A plain string
/// `starts_with` would be wrong twice over on Windows: it ignores that paths
/// are case-insensitive, and it would match a sibling whose name merely begins
/// with the root's (`…\narration-backup` next to `…\narration`) — which would
/// then be swept up by a kill this module performs on the user's behalf.
fn path_is_under(candidate: &Path, root: &Path) -> bool {
    let mut parts = candidate.components();
    for expected in root.components() {
        match parts.next() {
            Some(part) => {
                if !part
                    .as_os_str()
                    .to_string_lossy()
                    .eq_ignore_ascii_case(&expected.as_os_str().to_string_lossy())
                {
                    return false;
                }
            }
            None => return false,
        }
    }
    true
}

fn refreshed_system() -> sysinfo::System {
    sysinfo::System::new_with_specifics(
        sysinfo::RefreshKind::nothing().with_processes(
            sysinfo::ProcessRefreshKind::nothing().with_exe(sysinfo::UpdateKind::Always),
        ),
    )
}

/// Every live process whose EXECUTABLE sits inside `root`.
///
/// Matching is on the executable path, never on the process name: the engine
/// is a Python sidecar, and a name-based sweep for `python.exe` would kill
/// every unrelated Python on the machine (MCP servers, tooling, the user's own
/// scripts). The engine directory is ours by construction, so anything running
/// out of it is ours to stop.
///
/// The working directory is deliberately NOT matched, even though the sidecar
/// is spawned with its CWD pinned there: a cwd match would equally catch a
/// shell the user had `cd`-ed into that folder, and killing someone's terminal
/// to delete a TTS engine is not a trade worth making. Every real sidecar is
/// caught by its exe path regardless.
fn processes_under(sys: &sysinfo::System, root: &Path) -> Vec<StaleProcess> {
    let mut found: Vec<StaleProcess> = sys
        .processes()
        .values()
        .filter_map(|p| {
            let exe = p.exe()?;
            path_is_under(exe, root).then(|| StaleProcess {
                pid: p.pid().as_u32(),
                exe: exe.display().to_string(),
            })
        })
        .collect();
    // Stable order so the Hebrew error message reads the same on every retry.
    found.sort_by_key(|p| p.pid);
    found
}

/// Snapshot of everything currently running out of the narration directory —
/// whichever app instance spawned it, whichever port it bound, and whether or
/// not we hold a handle to it. This is the check `delete_narration_engine`
/// used to lack: it trusted its own in-memory handle as the sole authority on
/// "is anything using these files", so a sidecar it never tracked (a leftover
/// from a crashed dev run, a second instance, one started by hand) sailed
/// straight past it into a half-completed `remove_dir_all`.
pub fn processes_under_narration_dir() -> Vec<StaleProcess> {
    let root = crate::narration_provision::get_narration_dir();
    processes_under(&refreshed_system(), &root)
}

/// Terminate everything running out of the narration directory and wait for
/// the OS to actually reap it. Returns whatever SURVIVED — an empty vec means
/// the directory is genuinely free.
///
/// Re-scans on each pass rather than killing a captured list once: a process
/// can take a moment to die, and the sidecar's shim spawns the real
/// interpreter, so a single pass can see a tree mid-teardown.
pub async fn kill_processes_under_narration_dir() -> Vec<StaleProcess> {
    let root = crate::narration_provision::get_narration_dir();
    for _ in 0..25 {
        let sys = refreshed_system();
        let alive = processes_under(&sys, &root);
        if alive.is_empty() {
            return Vec::new();
        }
        for stale in &alive {
            if let Some(p) = sys.process(sysinfo::Pid::from_u32(stale.pid)) {
                p.kill();
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    processes_under_narration_dir()
}

/// Every still-live descendant of `pid`, captured BEFORE the parent is killed:
/// once the parent dies its children are re-parented and the link that
/// identifies them is gone.
fn descendants_of(sys: &sysinfo::System, pid: u32) -> Vec<u32> {
    let mut tree = vec![pid];
    let mut cursor = 0;
    while cursor < tree.len() {
        let parent = tree[cursor];
        for p in sys.processes().values() {
            let child = p.pid().as_u32();
            if p.parent().map(|pp| pp.as_u32()) == Some(parent) && !tree.contains(&child) {
                tree.push(child);
            }
        }
        cursor += 1;
    }
    tree.remove(0); // the root is killed through its own handle by the caller
    tree
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::narration::looks_like_valid_wav;

    /// The whole safety of the sweep rests on this predicate: everything it
    /// says `true` about gets KILLED. A sibling directory sharing a name
    /// prefix is the case a naive `starts_with` on strings gets wrong.
    #[test]
    fn path_is_under_matches_components_not_string_prefixes() {
        let root = Path::new(r"C:\Users\x\AppData\Roaming\hebrew-dictation\narration");

        assert!(path_is_under(
            Path::new(r"C:\Users\x\AppData\Roaming\hebrew-dictation\narration\venv\Scripts\python.exe"),
            root
        ));
        // Windows paths are case-insensitive; the same file spelled either way
        // must resolve the same, or a sweep would silently miss the sidecar.
        assert!(path_is_under(
            Path::new(r"c:\users\x\appdata\roaming\HEBREW-DICTATION\Narration\python\python.exe"),
            root
        ));

        // A SIBLING that merely starts with the same text — the exact false
        // positive that would make this module delete someone else's files.
        assert!(!path_is_under(
            Path::new(r"C:\Users\x\AppData\Roaming\hebrew-dictation\narration-backup\python.exe"),
            root
        ));
        // A parent, and an unrelated Python (the machine runs many).
        assert!(!path_is_under(
            Path::new(r"C:\Users\x\AppData\Roaming\hebrew-dictation\settings.json"),
            root
        ));
        assert!(!path_is_under(
            Path::new(r"C:\Users\x\AppData\Local\Programs\Python\Python312\python.exe"),
            root
        ));
    }

    /// The sweep must find the CURRENT process (this test binary runs from a
    /// real path) when handed its own directory, and must find nothing when
    /// handed a directory nothing runs from. Cheap, but it proves the sysinfo
    /// wiring — refresh kind included — actually populates `exe()`, which is
    /// the one thing the unit test above cannot check.
    #[test]
    fn processes_under_finds_real_running_processes_by_exe_path() {
        let sys = refreshed_system();
        let me = std::env::current_exe().expect("the test binary has a path");
        let my_dir = me.parent().expect("…and a parent directory");

        let found = processes_under(&sys, my_dir);
        assert!(
            found.iter().any(|p| p.pid == std::process::id()),
            "the sweep should find this very test process under its own directory; \
             if it doesn't, exe() is coming back empty and every real sweep is a no-op"
        );

        let nowhere = Path::new(r"C:\definitely-not-a-real-directory-9f3a\narration");
        assert!(processes_under(&sys, nowhere).is_empty());
    }

    /// `shutdown()` kills the sidecar's re-exec'd grandchild by walking the
    /// process tree, which only works if `parent()` is actually populated
    /// under the narrow refresh kind this module asks for. If it came back
    /// `None`, the walk would return an empty tree and the tree-kill would be
    /// a silent no-op — the exact failure shape being fixed here. So prove it
    /// against a real child process rather than trusting the sysinfo docs.
    #[test]
    fn descendants_of_finds_a_real_child_process() {
        let mut child = std::process::Command::new("cmd.exe")
            .args(["/c", "ping", "-n", "6", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .expect("cmd.exe is always present on Windows");

        let tree = descendants_of(&refreshed_system(), std::process::id());
        let found = tree.contains(&child.id());

        let _ = child.kill();
        let _ = child.wait();

        assert!(
            found,
            "the child should appear as a descendant of this test process; \
             if it doesn't, parent() is empty and shutdown()'s tree-kill is dead code"
        );
    }

    fn spawn_fake_healthy_sidecar() -> u16 {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        std::thread::spawn(move || {
            for request in server.incoming_requests() {
                let response = tiny_http::Response::from_data(b"{}".to_vec());
                let _ = request.respond(response);
            }
        });
        port
    }

    #[tokio::test]
    async fn spawn_or_adopt_returns_unmanaged_when_port_already_healthy() {
        let port = spawn_fake_healthy_sidecar();

        let result = NarrationServer::spawn_or_adopt(port, crate::narration_provision::DEFAULT_VOICE).await;

        assert!(
            matches!(result, Ok(NarrationServer::Unmanaged { .. })),
            "expected Unmanaged (adopted) when something already answers /info, got {:?}",
            result.is_ok()
        );
    }

    /// `Unmanaged` failures must never trigger a respawn attempt (see the
    /// module design note and `synthesize_with_restart`'s doc comment) — this
    /// constructs an `Unmanaged` pointing at an unreachable port directly
    /// (no need to go through `spawn_or_adopt`) and confirms the error comes
    /// back immediately as `Unreachable`, with `self` still `Unmanaged`
    /// afterwards (proof no respawn/state-swap happened).
    #[tokio::test]
    async fn synthesize_with_restart_on_unmanaged_failure_returns_unreachable_without_respawn() {
        // Port 1 is reserved/unassignable — nothing will ever listen there,
        // so this reliably reproduces "connection refused" (same convention
        // as narration.rs's own unreachable-port test).
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(3))
            .build()
            .unwrap();
        let mut server = NarrationServer::Unmanaged { port: 1, client };

        let result = server.synthesize_with_restart("שלום עולם", NarrationParams::default()).await;

        assert!(matches!(result, Err(NarrationError::Unreachable(_))));
        assert!(
            matches!(server, NarrationServer::Unmanaged { .. }),
            "an Unmanaged failure must never respawn/swap self into Owned"
        );
    }

    /// A fake sidecar that answers `/info` healthily (so `spawn_or_adopt`
    /// adopts it as `Unmanaged`) but fails `/synthesize` specifically —
    /// exercises the "adopted process is alive but flaky" case.
    fn spawn_fake_healthy_info_failing_synthesize() -> u16 {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        std::thread::spawn(move || {
            for request in server.incoming_requests() {
                if request.url() == "/synthesize" {
                    let response = tiny_http::Response::from_data(b"error".to_vec())
                        .with_status_code(tiny_http::StatusCode(500));
                    let _ = request.respond(response);
                } else {
                    let response = tiny_http::Response::from_data(b"{}".to_vec());
                    let _ = request.respond(response);
                }
            }
        });
        port
    }

    #[tokio::test]
    async fn synthesize_with_restart_on_adopted_process_surfaces_synthesize_failure() {
        let port = spawn_fake_healthy_info_failing_synthesize();

        // Adopts (Unmanaged) because /info answers healthily.
        let mut server = NarrationServer::spawn_or_adopt(port, crate::narration_provision::DEFAULT_VOICE).await.unwrap();
        assert!(matches!(server, NarrationServer::Unmanaged { .. }));

        // /synthesize fails on the adopted process — must surface as an
        // error, never attempt a respawn on the same port.
        let result = server.synthesize_with_restart("שלום עולם", NarrationParams::default()).await;

        // The REAL error must survive. This previously asserted `Unreachable`,
        // because the code replaced every adopted-sidecar failure with one
        // hardcoded message — discarding the only diagnostic available, given
        // the child's stdout/stderr go to null. The fake sidecar answers /info
        // but returns 500 from /synthesize, so BadResponse is the truth here.
        assert!(
            matches!(result, Err(NarrationError::BadResponse(_))),
            "the sidecar's actual error must reach the caller, got {result:?}"
        );
        // Still Unmanaged: no respawn was attempted on the adopted port.
        assert!(matches!(server, NarrationServer::Unmanaged { .. }));
    }

    /// The regression test for the delete-engine failure of 2026-08-15.
    ///
    /// Reproduces the ACTUAL cause, which was not the one first suspected: a
    /// sidecar this app never tracked. In the real incident it had been
    /// started by hand from a shell on a different port while fuzzing the
    /// server script, so `AppState.narration_server` was `None`, the delete
    /// command's shutdown step did nothing at all, and `remove_dir_all` walked
    /// into a venv whose `python.exe` was running — deleting the marker and
    /// the models before failing with "Access is denied (os error 5)" and
    /// leaving ~600MB of unreachable leftovers.
    ///
    /// So this test spawns the sidecar the way that incident did — directly,
    /// with no `NarrationServer`, no Job Object, no handle kept — and proves
    /// the sweep finds and stops it anyway. Non-destructive: it never deletes
    /// the engine, only proves the lock exists and then clears it.
    #[tokio::test]
    #[ignore = "needs a fully provisioned narration engine — run explicitly with `-- --ignored`"]
    async fn sweep_stops_an_untracked_sidecar_that_the_app_never_spawned() {
        let venv_python = get_venv_dir().join("Scripts").join("python.exe");
        assert!(
            venv_python.exists(),
            "provision the engine before running this test"
        );

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        // Deliberately raw: this is what a stray shell, a crashed dev run or a
        // second app instance leaves behind, and none of them hand us a handle.
        let mut stray = std::process::Command::new(&venv_python)
            .args(["narration_server.py", "--port", &port.to_string()])
            .current_dir(crate::narration_provision::get_narration_dir())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("the stray sidecar should start");

        // Wait for the shim to re-exec into the real interpreter, so the sweep
        // is tested against the two-process tree the incident actually had.
        tokio::time::sleep(Duration::from_secs(3)).await;

        let found = processes_under_narration_dir();
        assert!(
            found.iter().any(|p| p.pid == stray.id()),
            "the sweep must see a sidecar it never spawned; found {found:?}"
        );

        // Proof the lock is real, not theoretical: Windows refuses to unlink
        // the image file of a running process. This is the exact error the
        // delete command hit, reproduced without deleting anything.
        assert!(
            std::fs::remove_file(&venv_python).is_err(),
            "a running process's exe must not be deletable — if this passes, \
             the premise of the whole fix is wrong"
        );

        let survivors = kill_processes_under_narration_dir().await;
        assert!(
            survivors.is_empty(),
            "the sweep must leave nothing running; survivors: {survivors:?}"
        );
        assert!(
            processes_under_narration_dir().is_empty(),
            "including the re-exec'd grandchild, which is the process that \
             actually holds the port and the model files"
        );

        let _ = stray.kill();
        let _ = stray.wait();
    }

    #[tokio::test]
    #[ignore = "needs a fully provisioned narration engine (real uv+venv+piper-tts+voice) — run explicitly after provisioning has completed for real"]
    async fn spawn_owned_produces_a_working_sidecar_and_shutdown_kills_it() {
        // OS-assigned free port, not a hardcoded number — a hardcoded port
        // could collide with a still-bound leftover from a previous crashed
        // test run, in which case spawn_owned would "succeed" by talking to
        // that stale process instead of the one it just spawned.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener); // free the port so piper's own bind() can claim it

        let mut server = NarrationServer::spawn_owned(port, crate::narration_provision::DEFAULT_VOICE, reqwest::Client::new())
            .await
            .expect("spawn_owned should succeed against a real provisioned engine");

        // Real proof #1: it actually generates audio, not just "the process started."
        let audio = synthesize(server.client(), server.port(), "בדיקה", NarrationParams::default())
            .await
            .expect("a live, healthy sidecar should synthesize real audio");
        assert!(looks_like_valid_wav(&audio));

        // Real proof #2: shutdown() (the clean-exit path) actually kills the
        // process — the orphan-prevention claim, verified behaviorally rather
        // than assumed from reading the win32job/tokio API docs. Re-probing
        // the port after shutdown should find nothing there.
        server.shutdown().await;

        // Give the OS a moment to actually tear down the listening socket —
        // kill() returning doesn't guarantee the port is instantly free.
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        let client = reqwest::Client::new();
        assert!(
            !health_check(&client, port).await,
            "the sidecar should be unreachable after shutdown() — if this fails, \
             either kill_on_drop or the explicit kill() call isn't actually \
             terminating the process, which is the exact orphan bug this whole \
             chunk exists to catch"
        );
    }
}
