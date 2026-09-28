"""Reproduce the Claude Desktop lone-Alt menu with a real RegisterHotKey(Alt+D),
then check whether a mask keystroke sent from the WM_HOTKEY handler prevents it.

Target is the Claude-like Electron test double in ./app, never a user window:
every scenario refuses to send a single key unless the test window holds the
foreground.

These probes take the keyboard focus for a few seconds. Run them only when the
machine's user has agreed not to type or dictate meanwhile, and close any
running copy of the dictation app first (the probes that launch the app need
its hotkeys and its WebView2 profile to themselves).

ELECTRON_EXE: path to an electron.exe (any recent Electron; the test double
has no dependencies). Unzip one from %LOCALAPPDATA%\\electron\\Cache.
"""
import ctypes, ctypes.wintypes as wt, json, os, subprocess, sys, tempfile, time

HERE = os.path.dirname(os.path.abspath(__file__))
ELECTRON = os.environ.get("ELECTRON_EXE", os.path.join(HERE, "electron", "electron.exe"))
APPDIR = os.path.join(HERE, "app")
LOG = os.path.join(tempfile.gettempdir(), "hd_claude_double.log")
STATE = os.path.join(tempfile.gettempdir(), "hd_claude_double_state.json")

user32 = ctypes.WinDLL("user32", use_last_error=True)
kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
ULONG_PTR = ctypes.c_size_t


class KEYBDINPUT(ctypes.Structure):
    _fields_ = [("wVk", wt.WORD), ("wScan", wt.WORD), ("dwFlags", wt.DWORD),
                ("time", wt.DWORD), ("dwExtraInfo", ULONG_PTR)]


class MOUSEINPUT(ctypes.Structure):
    _fields_ = [("dx", wt.LONG), ("dy", wt.LONG), ("mouseData", wt.DWORD),
                ("dwFlags", wt.DWORD), ("time", wt.DWORD), ("dwExtraInfo", ULONG_PTR)]


class _U(ctypes.Union):
    _fields_ = [("ki", KEYBDINPUT), ("mi", MOUSEINPUT)]


class INPUT(ctypes.Structure):
    _anonymous_ = ("u",)
    _fields_ = [("type", wt.DWORD), ("u", _U)]


class MSG(ctypes.Structure):
    _fields_ = [("hwnd", wt.HWND), ("message", wt.UINT), ("wParam", wt.WPARAM),
                ("lParam", wt.LPARAM), ("time", wt.DWORD), ("pt", wt.POINT), ("lPrivate", wt.DWORD)]


user32.GetForegroundWindow.restype = wt.HWND
user32.SetForegroundWindow.argtypes = [wt.HWND]
user32.BringWindowToTop.argtypes = [wt.HWND]
user32.ShowWindow.argtypes = [wt.HWND, ctypes.c_int]
user32.GetWindowThreadProcessId.argtypes = [wt.HWND, ctypes.POINTER(wt.DWORD)]
user32.GetWindowThreadProcessId.restype = wt.DWORD
user32.AttachThreadInput.argtypes = [wt.DWORD, wt.DWORD, wt.BOOL]
user32.GetWindowTextW.argtypes = [wt.HWND, wt.LPWSTR, ctypes.c_int]
user32.IsWindowVisible.argtypes = [wt.HWND]
user32.PeekMessageW.argtypes = [ctypes.POINTER(MSG), wt.HWND, wt.UINT, wt.UINT, wt.UINT]
user32.RegisterHotKey.argtypes = [wt.HWND, ctypes.c_int, wt.UINT, wt.UINT]
user32.UnregisterHotKey.argtypes = [wt.HWND, ctypes.c_int]
user32.SendInput.argtypes = [wt.UINT, ctypes.POINTER(INPUT), ctypes.c_int]
user32.MapVirtualKeyW.argtypes = [wt.UINT, wt.UINT]
user32.GetAsyncKeyState.argtypes = [ctypes.c_int]
user32.GetAsyncKeyState.restype = ctypes.c_short

KEYUP = 0x2
VK_LMENU, VK_D, VK_E8, VK_F24 = 0xA4, 0x44, 0xE8, 0x87
MOD_ALT, MOD_NOREPEAT, WM_HOTKEY = 0x1, 0x4000, 0x0312


def key(vk, up=False):
    i = INPUT(type=1)
    i.ki = KEYBDINPUT(vk, user32.MapVirtualKeyW(vk, 0), KEYUP if up else 0, 0, 0)
    return i


def send(*inputs):
    arr = (INPUT * len(inputs))(*inputs)
    n = user32.SendInput(len(inputs), arr, ctypes.sizeof(INPUT))
    if n != len(inputs):
        raise OSError(f"SendInput sent {n}/{len(inputs)} err={ctypes.get_last_error()}")


def find_window(title):
    found = []
    proc = ctypes.WINFUNCTYPE(wt.BOOL, wt.HWND, wt.LPARAM)

    def cb(hwnd, _):
        buf = ctypes.create_unicode_buffer(256)
        user32.GetWindowTextW(hwnd, buf, 256)
        if buf.value == title and user32.IsWindowVisible(hwnd):
            found.append(hwnd)
        return True

    user32.EnumWindows(proc(cb), 0)
    return found[0] if found else None


def ensure_foreground(hwnd):
    if user32.GetForegroundWindow() == hwnd:
        return True
    fg = user32.GetForegroundWindow()
    fg_thread = user32.GetWindowThreadProcessId(fg, None) if fg else 0
    me = kernel32.GetCurrentThreadId()
    if fg_thread:
        user32.AttachThreadInput(me, fg_thread, True)
    user32.ShowWindow(hwnd, 5)
    user32.BringWindowToTop(hwnd)
    user32.SetForegroundWindow(hwnd)
    if fg_thread:
        user32.AttachThreadInput(me, fg_thread, False)
    time.sleep(0.3)
    return user32.GetForegroundWindow() == hwnd


def wait_hotkey(timeout=1.0):
    msg = MSG()
    t0 = time.time()
    while time.time() - t0 < timeout:
        while user32.PeekMessageW(ctypes.byref(msg), None, 0, 0, 1):
            if msg.message == WM_HOTKEY:
                return True
        time.sleep(0.002)
    return False


def mark(text):
    with open(LOG, "a", encoding="utf-8") as f:
        f.write(f"{int(time.time() * 1000)} SCENARIO {text}\n")


def scenario_lone_alt():
    send(key(VK_LMENU)); time.sleep(0.05); send(key(VK_LMENU, True))
    return True


def scenario_hotkey(mask=None, alt_released_first=True):
    send(key(VK_LMENU)); time.sleep(0.04)
    send(key(VK_D))
    got = wait_hotkey(1.0)
    if got and mask:
        send(key(mask), key(mask, True))
    time.sleep(0.05)
    if alt_released_first:
        send(key(VK_LMENU, True)); time.sleep(0.03); send(key(VK_D, True))
    else:
        send(key(VK_D, True)); time.sleep(0.03); send(key(VK_LMENU, True))
    return got


def main():
    for p in (LOG, STATE):
        if os.path.exists(p):
            os.remove(p)
    env = dict(os.environ, HD_TEST_LOG=LOG, HD_TEST_STATE=STATE)
    proc = subprocess.Popen([ELECTRON, APPDIR], env=env)
    try:
        t0 = time.time()
        while time.time() - t0 < 30:
            if os.path.exists(LOG) and "READY" in open(LOG, encoding="utf-8").read():
                break
            time.sleep(0.2)
        else:
            print("electron never became ready"); return 2
        hwnd = find_window("HD-ALT-TEST")
        if not hwnd:
            print("test window not found"); return 2
        if not user32.RegisterHotKey(None, 1, MOD_ALT | MOD_NOREPEAT, VK_D):
            print("RegisterHotKey(Alt+D) failed:", ctypes.get_last_error()); return 2
        scenarios = [
            ("lone Alt (control)", scenario_lone_alt),
            ("Alt+D hotkey, Alt released first, no mask", lambda: scenario_hotkey(None, True)),
            ("Alt+D hotkey, D released first, no mask", lambda: scenario_hotkey(None, False)),
            ("Alt+D hotkey, Alt released first, mask vkE8", lambda: scenario_hotkey(VK_E8, True)),
            ("Alt+D hotkey, D released first, mask vkE8", lambda: scenario_hotkey(VK_E8, False)),
            ("Alt+D hotkey, Alt released first, mask F24", lambda: scenario_hotkey(VK_F24, True)),
        ]
        results = []
        for name, fn in scenarios:
            if not ensure_foreground(hwnd):
                print(f"SKIP {name}: test window is not foreground, refusing to send keys")
                results.append((name, None, None)); continue
            mark(name)
            got = fn()
            time.sleep(0.5)
            results.append((name, got, None))
        mark("END")
        user32.UnregisterHotKey(None, 1)
        # Split the shared log by scenario markers.
        lines = open(LOG, encoding="utf-8").read().splitlines()
        blocks, cur = {}, None
        for ln in lines:
            parts = ln.split(" ", 1)
            body = parts[1] if len(parts) > 1 else ""
            if body.startswith("SCENARIO "):
                cur = body[len("SCENARIO "):]
                blocks[cur] = []
            elif cur:
                blocks[cur].append(body)
        for name, got, _ in results:
            b = blocks.get(name, [])
            opened = any(x == "OPEN_MENU" for x in b)
            print(f"\n== {name}  hotkey_fired={got}  MENU_OPENED={opened}")
            for x in b:
                print("   ", x)
        return 0
    finally:
        proc.terminate()


if __name__ == "__main__":
    sys.exit(main())
