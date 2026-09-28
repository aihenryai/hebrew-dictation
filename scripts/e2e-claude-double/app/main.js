// Test double for Claude Desktop: the lone-Alt menu tracker below is copied
// (de-minified) from Claude 2.9939.2.0's app.asar (functions v0r/C0r), and the
// composer mimics its Enter-sends / Shift+Enter-newline behaviour. Logs every
// key event it receives; "OPEN_MENU" is where Claude would pop its menu.
// Control channel on 127.0.0.1:47931: /state, /reset, /focus.
const { app, BrowserWindow, Menu, ipcMain } = require('electron');
const fs = require('fs');
const path = require('path');

const LOG = process.env.HD_TEST_LOG;
const STATE = process.env.HD_TEST_STATE;
const log = (line) => fs.appendFileSync(LOG, `${Date.now()} ${line}\n`);

function makeTracker() {
  let armed = false;
  return {
    onKey(t) {
      const isAlt = t.key === 'Alt';
      const otherMod = t.control || t.shift || t.meta;
      if (t.type === 'keyDown') {
        if (isAlt) { if (!t.isAutoRepeat) armed = !otherMod; } else { armed = false; }
        return 'pass';
      }
      const open = t.type === 'keyUp' && isAlt && armed && !otherMod;
      armed = false;
      return open ? 'open' : 'pass';
    },
    reset() { armed = false; },
  };
}

const state = { value: '', sent: [] };
const save = () => fs.writeFileSync(STATE, JSON.stringify(state));

app.whenReady().then(() => {
  Menu.setApplicationMenu(Menu.buildFromTemplate([{ label: 'File', submenu: [{ role: 'quit' }] }]));
  const win = new BrowserWindow({
    x: 40, y: 40, width: 620, height: 380, title: 'HD-ALT-TEST', alwaysOnTop: true,
    webPreferences: { preload: path.join(__dirname, 'preload.js') },
  });
  win.setAutoHideMenuBar(false);
  win.setMenuBarVisibility(false);

  const tracker = makeTracker();
  const nonKey = new Set(['rawKeyDown', 'keyDown', 'keyUp', 'char', 'mouseMove', 'pointerMove', 'pointerRawUpdate']);
  win.webContents.on('before-input-event', (_e, input) => {
    log(`INPUT ${input.type} key=${JSON.stringify(input.key)} code=${JSON.stringify(input.code)} alt=${input.alt} ctrl=${input.control} shift=${input.shift}`);
    if (tracker.onKey(input) === 'open') log('OPEN_MENU');
  });
  win.webContents.on('input-event', (_e, ev) => { if (!nonKey.has(ev.type)) tracker.reset(); });
  win.on('blur', () => { tracker.reset(); log('BLUR'); });
  win.on('focus', () => log('FOCUS'));

  ipcMain.on('value', (_e, v) => { state.value = v; save(); });
  ipcMain.on('sent', (_e, v) => { state.sent.push(v); state.value = ''; save(); log('SENT ' + JSON.stringify(v)); });
  ipcMain.on('reset', () => { state.value = ''; state.sent = []; save(); });

  win.loadFile(path.join(__dirname, 'index.html'));
  win.webContents.on('did-finish-load', () => { save(); win.focus(); log('READY'); });

  // Control channel for the harness: read state, clear the composer.
  require('http').createServer(async (req, res) => {
    if (req.url === '/reset') {
      await win.webContents.executeJavaScript("document.getElementById('composer').value='';document.getElementById('composer').focus();1");
      state.value = ''; state.sent = []; save();
    }
    if (req.url === '/focus') {
      win.show(); win.focus(); win.webContents.focus();
      await win.webContents.executeJavaScript("document.getElementById('composer').focus();1");
    }
    res.setHeader('content-type', 'application/json; charset=utf-8');
    res.end(JSON.stringify(state));
  }).listen(47931, '127.0.0.1');
});
