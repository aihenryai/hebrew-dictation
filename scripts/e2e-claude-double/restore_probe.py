"""End of dictation: does bringing the main window back cost the target app
the last keystrokes? Plus regression checks for summoning main mid-dictation
and for the paste button.

usage: python restore_probe.py <path-to-hebrew-dictation.exe> <label>

Safety: the app only ever types while the Claude-like test window holds the
foreground. Every injection is skipped (and reported) otherwise.
"""
import ctypes, json, os, subprocess, sys, threading, time
import alt_probe as ap
import paste_probe as pp

user32 = ap.user32
WM_CLOSE = 0x0010
user32.PostMessageW.argtypes = [ctypes.c_void_p, ctypes.c_uint, ctypes.c_size_t, ctypes.c_ssize_t]
LONG = " ".join(f"משפט מספר {i} בהכתבה ארוכה שנבדקת עד הסוף." for i in range(1, 21)) + " סוף."


def call(ws, cmd, args=None):
    return pp.cdp_eval(ws, "window.__TAURI_INTERNALS__.invoke(%s, %s).then(() => 'ok', e => 'ERR: ' + e)"
                       % (json.dumps(cmd), json.dumps(args or {}, ensure_ascii=False)))


class Timeline(threading.Thread):
    """Foreground window changes, in ms since start."""
    def __init__(self):
        super().__init__(daemon=True)
        self.t0, self.events, self.stop = time.time(), [], False

    def now(self):
        return int((time.time() - self.t0) * 1000)

    def run(self):
        last = None
        while not self.stop:
            fg = user32.GetForegroundWindow()
            if fg != last:
                self.events.append((self.now(), pp.title(fg) or "?"))
                last = fg
            time.sleep(0.01)


def focus_double(dbl):
    ok = ap.ensure_foreground(dbl)
    pp.ctl("/focus")
    time.sleep(0.2)
    return ok and user32.GetForegroundWindow() == dbl


def guarded_inject(ws, dbl, text):
    fg = user32.GetForegroundWindow()
    if fg != dbl:
        return f"SKIPPED (foreground is '{pp.title(fg)}')"
    return call(ws, "inject_text", {"text": text})


def wait_fg_leaves(hwnd, secs=1.5):
    t0 = time.time()
    while time.time() - t0 < secs:
        fg = user32.GetForegroundWindow()
        if fg and fg != hwnd:
            return pp.title(fg)
        time.sleep(0.02)
    return None


def end_of_dictation(name, ws, dbl, main_hwnd, force, start_in_main):
    pp.ctl("/reset")
    if not focus_double(dbl):
        print(f"[{pp.LABEL}] {name}: SKIPPED, test window would not take focus"); return
    if start_in_main and not ap.ensure_foreground(main_hwnd):
        print(f"[{pp.LABEL}] {name}: SKIPPED, main would not take focus"); return
    tl = Timeline(); tl.start()
    call(ws, "show_toolbar_window", {"streaming": True})
    activated = wait_fg_leaves(main_hwnd) if start_in_main else pp.title(user32.GetForegroundWindow())
    if start_in_main:
        pp.ctl("/focus")  # Chromium's own focus, as a real reactivation would restore it
        time.sleep(0.15)
    res = guarded_inject(ws, dbl, LONG)
    t_inject_done = tl.now()
    call(ws, "hide_toolbar_window", {"forceShowMain": force, "deferRestore": True})
    call(ws, "restore_after_dictation", {"forceShowMain": force})
    t_restore = tl.now()
    time.sleep(3.5)
    tl.stop = True
    st = pp.ctl("/state")
    main_active_at = next((t for t, title in tl.events if t >= t_restore and title == "הכתבה בעברית"), None)
    print(f"[{pp.LABEL}] {name}: " + json.dumps({
        "window_after_hiding_main": activated, "inject": res,
        "typed": f"{len(st['value'])}/{len(LONG)}", "exact": st["value"] == LONG and not st["sent"],
        "main_visible_at_end": bool(user32.IsWindowVisible(main_hwnd)),
        "main_foreground_ms_after_restore": None if main_active_at is None else main_active_at - t_restore,
        "inject_to_restore_ms": t_restore - t_inject_done,
        "foreground_timeline": tl.events[-6:]}, ensure_ascii=False))


def main():
    for p in (ap.LOG, ap.STATE):
        if os.path.exists(p):
            os.remove(p)
    dbl_proc = subprocess.Popen([ap.ELECTRON, ap.APPDIR],
                                env=dict(os.environ, HD_TEST_LOG=ap.LOG, HD_TEST_STATE=ap.STATE))
    app_proc = None
    try:
        t0 = time.time()
        while not (os.path.exists(ap.LOG) and "READY" in open(ap.LOG, encoding="utf-8").read()):
            if time.time() - t0 > 30:
                raise SystemExit("test double never became ready")
            time.sleep(0.2)
        dbl = ap.find_window("HD-ALT-TEST")
        env = dict(os.environ, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=f"--remote-debugging-port={pp.CDP}")
        app_proc = subprocess.Popen([pp.APP], env=env)
        ws = pp.main_target()
        time.sleep(2.0)
        main_hwnd = [w for w in pp.windows_of(app_proc.pid) if w[2] and w[1] != "Dictation Toolbar"][0][0]

        # A: main open, the user started from it, stopped with the hotkey.
        end_of_dictation("A main open, hotkey stop", ws, dbl, main_hwnd, force=False, start_in_main=True)

        # B: main hidden by a dictation, the user summons it from the tray / idle circle.
        if not user32.IsWindowVisible(main_hwnd):
            call(ws, "open_main_window"); time.sleep(0.5)
        ap.ensure_foreground(main_hwnd)
        call(ws, "show_toolbar_window", {"streaming": True})
        wait_fg_leaves(main_hwnd)
        hidden = not user32.IsWindowVisible(main_hwnd)
        call(ws, "open_main_window")
        time.sleep(0.6)
        print(f"[{pp.LABEL}] B summon mid-dictation: " + json.dumps({
            "hidden_by_dictation": hidden, "visible_after_summon": bool(user32.IsWindowVisible(main_hwnd))},
            ensure_ascii=False))
        call(ws, "hide_toolbar_window", {"forceShowMain": False, "deferRestore": False})
        time.sleep(0.5)

        # C: main in the tray, stopped from the floating bar (asks to see main).
        user32.PostMessageW(main_hwnd, WM_CLOSE, 0, 0)
        time.sleep(0.8)
        end_of_dictation("C tray, bar stop", ws, dbl, main_hwnd, force=True, start_in_main=False)

        # Paste button regression check (main must be visible for it).
        if not user32.IsWindowVisible(main_hwnd):
            call(ws, "open_main_window"); time.sleep(0.5)
        print(f"[{pp.LABEL}] paste: " + json.dumps(pp.paste_scenario(dbl, main_hwnd, ws, LONG), ensure_ascii=False))
    finally:
        if app_proc:
            subprocess.run(["taskkill", "/PID", str(app_proc.pid), "/T", "/F"], capture_output=True)
        dbl_proc.terminate()


if __name__ == "__main__":
    main()
