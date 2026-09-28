"""End-to-end check of the dictation app's paste path and Alt handling against
the Claude-like Electron test double.

usage: python paste_probe.py <path-to-hebrew-dictation.exe> <label> [--lines]

The app is launched with WebView2 remote debugging so the harness can call the
exact command the "paste" button calls (invoke('inject_text')) while the app's
main window holds the foreground, as it does when the user clicks the button.
"""
import ctypes, json, os, subprocess, sys, time, urllib.request
import websocket
import alt_probe as ap

APP, LABEL = sys.argv[1], sys.argv[2]
WITH_LINES = "--lines" in sys.argv
CDP = 9333
user32 = ap.user32


def ctl(path):
    raw = urllib.request.urlopen(f"http://127.0.0.1:47931{path}", timeout=5).read()
    return json.loads(raw.decode("utf-8"))


def cdp_eval(ws_url, expr, timeout=90):
    ws = websocket.create_connection(ws_url, timeout=timeout, suppress_origin=True)
    try:
        ws.send(json.dumps({"id": 1, "method": "Runtime.evaluate", "params": {
            "expression": expr, "awaitPromise": True, "returnByValue": True}}))
        while True:
            msg = json.loads(ws.recv())
            if msg.get("id") == 1:
                return msg.get("result", {}).get("result", {}).get("value", msg)
    finally:
        ws.close()


def main_target():
    t0 = time.time()
    while time.time() - t0 < 40:
        try:
            targets = json.loads(urllib.request.urlopen(f"http://127.0.0.1:{CDP}/json", timeout=3).read())
            for t in targets:
                if t.get("type") != "page" or "webSocketDebuggerUrl" not in t:
                    continue
                label = cdp_eval(t["webSocketDebuggerUrl"],
                                 "window.__TAURI_INTERNALS__?.metadata?.currentWindow?.label || ''", 10)
                if label == "main":
                    return t["webSocketDebuggerUrl"]
        except Exception:
            pass
        time.sleep(0.5)
    raise SystemExit("main webview never appeared on the CDP port")


def windows_of(pid):
    out = []
    proc = ctypes.WINFUNCTYPE(ctypes.c_bool, ctypes.c_void_p, ctypes.c_void_p)

    def cb(hwnd, _):
        p = ctypes.c_ulong()
        user32.GetWindowThreadProcessId(hwnd, ctypes.byref(p))
        if p.value == pid:
            buf = ctypes.create_unicode_buffer(256)
            user32.GetWindowTextW(hwnd, buf, 256)
            out.append((hwnd, buf.value, bool(user32.IsWindowVisible(hwnd))))
        return True

    user32.EnumWindows(proc(cb), 0)
    return out


def title(hwnd):
    buf = ctypes.create_unicode_buffer(256)
    user32.GetWindowTextW(hwnd, buf, 256)
    return buf.value


def alt_p_scenario(dbl):
    if not ap.ensure_foreground(dbl):
        return "SKIPPED: test window not foreground"
    ap.mark(f"{LABEL} Alt+P")
    ap.send(ap.key(ap.VK_LMENU)); time.sleep(0.04)
    ap.send(ap.key(0x50)); time.sleep(0.09)
    ap.send(ap.key(ap.VK_LMENU, True)); time.sleep(0.03); ap.send(ap.key(0x50, True))
    time.sleep(0.6)
    ap.mark("end")
    block = open(ap.LOG, encoding="utf-8").read().split(f"SCENARIO {LABEL} Alt+P")[-1].split("SCENARIO end")[0]
    if 'code="KeyP"' in block and "keyDown" in block.split('code="KeyP"')[0].splitlines()[-1]:
        return "INVALID: P reached the window, the app did not own Alt+P"
    return ("MENU OPENED" if "OPEN_MENU" in block else "no menu") + ("  (mask seen)" if "Unidentified" in block else "")


def paste_scenario(dbl, main_hwnd, ws_url, text):
    ctl("/reset")
    if not ap.ensure_foreground(dbl):
        return {"skipped": "test window not foreground"}
    time.sleep(0.2)
    if not ap.ensure_foreground(main_hwnd):
        return {"skipped": "app main window would not take the foreground"}
    t0 = time.time()
    res = cdp_eval(ws_url, "window.__TAURI_INTERNALS__.invoke('inject_text', {text: %s})"
                   ".then(() => 'ok', e => 'ERR: ' + e)" % json.dumps(text, ensure_ascii=False))
    took = time.time() - t0
    fg = user32.GetForegroundWindow()
    time.sleep(2.0)
    st = ctl("/state")
    got = st["value"]
    return {"invoke": res, "took_s": round(took, 2), "foreground_after": title(fg),
            "sent_messages": st["sent"], "typed_chars": len(got), "expected_chars": len(text),
            "exact": got == text and not st["sent"],
            "missing_tail": text[len(got):][:60] if text.startswith(got) and got != text else ""}


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
        env = dict(os.environ, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=f"--remote-debugging-port={CDP}")
        app_proc = subprocess.Popen([APP], env=env)
        ws_url = main_target()
        time.sleep(2.0)
        mains = [w for w in windows_of(app_proc.pid) if w[2] and w[1] != "Dictation Toolbar"]
        if not mains:
            raise SystemExit(f"no visible main window: {windows_of(app_proc.pid)}")
        main_hwnd = mains[0][0]
        print(f"[{LABEL}] app pid={app_proc.pid} main='{mains[0][1]}'")

        print(f"[{LABEL}] Alt+P while the test window is focused: {alt_p_scenario(dbl)}")

        long_text = " ".join(f"משפט מספר {i} בהכתבה ארוכה שנבדקת עד הסוף." for i in range(1, 21)) + " סוף."
        for n in range(3):
            print(f"[{LABEL}] long paste #{n + 1}:", json.dumps(paste_scenario(dbl, main_hwnd, ws_url, long_text), ensure_ascii=False))
        if WITH_LINES:
            lines = "דובר 1: שלום לכולם, מתחילים\n\nדובר 2: תודה רבה. שורה שנייה\nשורה שלישית בלי רווח"
            print(f"[{LABEL}] multi-line paste:", json.dumps(paste_scenario(dbl, main_hwnd, ws_url, lines), ensure_ascii=False))
    finally:
        if app_proc:
            subprocess.run(["taskkill", "/PID", str(app_proc.pid), "/T", "/F"], capture_output=True)
        dbl_proc.terminate()


if __name__ == "__main__":
    main()
