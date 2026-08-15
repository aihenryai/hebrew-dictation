//! Sidecar process lifecycle for the narration engine: spawn, health-check,
//! Windows Job Object crash-protection, and guaranteed teardown. Talks to
//! `narration.rs` for the HTTP protocol itself (build_server_args, health_check,
//! synthesize) — this module only owns "is there a running process and is it
//! healthy," never the HTTP details. Windows-only (macOS out of scope for
//! Phase 1); a later task provides the non-Windows Tauri-command stub.

use crate::narration::{build_server_args, health_check, synthesize, NarrationError, NarrationParams};
use crate::narration_provision::get_venv_dir;
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
    /// On `Err` from a failed respawn attempt, `self` retains the `Owned`
    /// variant with a dead child; the next call will retry the respawn
    /// attempt (shutdown on a dead child is a safe no-op).
    pub async fn synthesize_with_restart(
        &mut self,
        text: &str,
        params: NarrationParams,
    ) -> Result<Vec<u8>, NarrationError> {
        if let Ok(bytes) = synthesize(self.client(), self.port(), text, params).await {
            return Ok(bytes);
        }

        let (port, voice_id) = match self {
            NarrationServer::Owned { port, voice_id, .. } => (*port, voice_id.clone()),
            NarrationServer::Unmanaged { .. } => {
                return Err(NarrationError::Unreachable(
                    "מנוע הקריינות (שאומץ מריצה קודמת) לא הגיב. נסה שוב.".to_string(),
                ));
            }
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
        if let NarrationServer::Owned { child, .. } = self {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::narration::looks_like_valid_wav;

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

        assert!(matches!(result, Err(NarrationError::Unreachable(_))));
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
