"""Hold-to-talk and in-place language switching, end to end on the real app.

usage: python ptt_probe.py <path-to-hebrew-dictation.exe> <label>

Mechanics only: the microphone hears a quiet room, so nothing gets typed. What
is checked is what the user would see: the recording bar appears on a Ctrl+Win
hold and leaves on release, Start does not open, a Ctrl+Win+<key> shortcut
never starts a recording, and switching language mid-dictation keeps the bar up
and the main window closed while the bar's language label changes.

Uses the machine's real settings and Deepgram key (a few seconds of audio). It
turns hold-to-talk on for the test and restores the previous value at the end.
Same rules as the other probes: close the user's copy of the app first, and
only with the user's agreement not to type meanwhile.
"""
import ctypes, json, os, subprocess, sys, threading, time
import alt_probe as ap
import paste_probe as pp

user32 = ap.user32
VK_LCONTROL, VK_LWIN, VK_F24, VK_X, VK_W = 0xA2, 0x5B, 0x87, 0x58, 0x57


def call(ws, cmd, args=None):
    return pp.cdp_eval(ws, "window.__TAURI_INTERNALS__.invoke(%s, %s).then(r => JSON.stringify(r ?? 'ok'), e => 'ERR: ' + e)"
                       % (json.dumps(cmd), json.dumps(args or {})))


def target_ws(label):
    targets = json.loads(__import__("urllib.request").request.urlopen(f"http://127.0.0.1:{pp.CDP}/json", timeout=3).read())
    for t in targets:
        if t.get("type") == "page" and "webSocketDebuggerUrl" in t:
            if pp.cdp_eval(t["webSocketDebuggerUrl"], "window.__TAURI_INTERNALS__?.metadata?.currentWindow?.label || ''", 10) == label:
                return t["webSocketDebuggerUrl"]
    return None


class Watch(threading.Thread):
    """Samples toolbar / main visibility and the foreground every 20ms."""
    def __init__(self, toolbar, main):
        super().__init__(daemon=True)
        self.toolbar, self.main, self.samples, self.stop, self.t0 = toolbar, main, [], False, time.time()

    def run(self):
        while not self.stop:
            fg = user32.GetForegroundWindow()
            self.samples.append((int((time.time() - self.t0) * 1000), bool(user32.IsWindowVisible(self.toolbar)),
                                 bool(user32.IsWindowVisible(self.main)), pp.title(fg)))
            time.sleep(0.02)

    def first(self, pred):
        return next((t for t, *s in self.samples if pred(*s)), None)


def hold(keys, ms, extra=None):
    for k in keys:
        ap.send(ap.key(k)); time.sleep(0.03)
    if extra:
        time.sleep(0.05); ap.send(ap.key(extra))
    time.sleep(ms / 1000)
    if extra:
        ap.send(ap.key(extra, True))
    for k in reversed(keys):
        ap.send(ap.key(k, True)); time.sleep(0.03)


def main():
    for p in (ap.LOG, ap.STATE):
        if os.path.exists(p):
            os.remove(p)
    dbl_proc = subprocess.Popen([ap.ELECTRON, ap.APPDIR], env=dict(os.environ, HD_TEST_LOG=ap.LOG, HD_TEST_STATE=ap.STATE))
    app_proc, ws, ptt_before = None, None, None
    try:
        t0 = time.time()
        while not (os.path.exists(ap.LOG) and "READY" in open(ap.LOG, encoding="utf-8").read()):
            if time.time() - t0 > 30:
                raise SystemExit("test double never became ready")
            time.sleep(0.2)
        dbl = ap.find_window("HD-ALT-TEST")
        app_proc = subprocess.Popen([pp.APP], env=dict(os.environ, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=f"--remote-debugging-port={pp.CDP}"))
        ws = pp.main_target()
        time.sleep(2.5)
        wins = pp.windows_of(app_proc.pid)
        toolbar = next(h for h, t, v in wins if t == "Dictation Toolbar")
        main_hwnd = next(h for h, t, v in wins if v and t != "Dictation Toolbar")
        settings = json.loads(call(ws, "get_settings"))
        ptt_before, toggle = settings.get("push_to_talk_enabled", False), settings.get("hotkey")
        print(f"[{pp.LABEL}] toggle hotkey={toggle} language_hotkey={settings.get('language_hotkey')} ptt_before={ptt_before}")
        call(ws, "set_push_to_talk_enabled", {"enabled": True})
        # Main in the tray, like normal use.
        user32.PostMessageW.argtypes = [ctypes.c_void_p, ctypes.c_uint, ctypes.c_size_t, ctypes.c_ssize_t]
        user32.PostMessageW(main_hwnd, 0x0010, 0, 0)
        time.sleep(0.8)

        def scenario(name, action):
            pp.ctl("/focus"); time.sleep(0.2)
            if not ap.ensure_foreground(dbl):
                print(f"[{pp.LABEL}] {name}: SKIPPED (test window not foreground)"); return
            pp.ctl("/focus"); time.sleep(0.2)
            w = Watch(toolbar, main_hwnd); w.start()
            action()
            time.sleep(1.8)
            w.stop = True; w.join()
            bar_on = w.first(lambda bar, main, fg: bar)
            bar_off_after = next((t for t, bar, main, fg in w.samples if bar_on is not None and t > bar_on and not bar), None)
            start_menu = any(fg in ("Start", "Search", "חיפוש", "התחל") for _, _, _, fg in w.samples)
            print(f"[{pp.LABEL}] {name}: " + json.dumps({
                "bar_shown_at_ms": bar_on, "bar_hidden_at_ms": bar_off_after,
                "main_ever_shown": any(m for _, _, m, _ in w.samples),
                "start_menu_opened": start_menu, "foreground_at_end": w.samples[-1][3]}, ensure_ascii=False))
            return w

        scenario("hold Ctrl+Win 2s", lambda: hold([VK_LCONTROL, VK_LWIN], 2000))
        scenario("Ctrl+Win+F24 (a shortcut)", lambda: hold([VK_LCONTROL, VK_LWIN], 700, extra=VK_F24))

        # Language switch in the middle of a toggle-started dictation.
        tb_ws = target_ws("toolbar")
        label = lambda: pp.cdp_eval(tb_ws, "document.querySelector('.toolbar-lang')?.innerText || ''", 5) if tb_ws else "?"
        def switch_mid_dictation():
            mods = [VK_LCONTROL] if toggle and toggle.startswith("ctrl") else [0xA4]
            letter = ord(toggle.split("+")[-1].upper()) if toggle else ord("D")
            hold(mods + [letter], 60)                      # start (toggle hotkey)
            time.sleep(1.5)
            before = label()
            hold([0xA4, VK_X], 60)                         # Alt+X: switch language
            time.sleep(2.0)
            after = label()
            hold(mods + [letter], 60)                      # stop
            print(f"[{pp.LABEL}]   bar language: {before} -> {after}")
        w = scenario("Alt+X mid-dictation", switch_mid_dictation)
        if w:
            gaps = [t for t, bar, main, fg in w.samples if w.first(lambda b, m, f: b) and t > w.first(lambda b, m, f: b) and not bar]
            print(f"[{pp.LABEL}]   first moment the bar was down after starting: {gaps[0] if gaps else None} ms")
        err = pp.cdp_eval(ws, "document.querySelector('.error')?.innerText || ''", 5)
        print(f"[{pp.LABEL}] app error text: {err!r}")
    finally:
        if ws is not None and ptt_before is not None:
            try:
                call(ws, "set_push_to_talk_enabled", {"enabled": bool(ptt_before)})
            except Exception as e:
                print("could not restore push_to_talk_enabled:", e)
        if app_proc:
            subprocess.run(["taskkill", "/PID", str(app_proc.pid), "/T", "/F"], capture_output=True)
        dbl_proc.terminate()


if __name__ == "__main__":
    main()
