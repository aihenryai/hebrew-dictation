const { contextBridge, ipcRenderer } = require('electron');
contextBridge.exposeInMainWorld('probe', {
  value: (v) => ipcRenderer.send('value', v),
  sent: (v) => ipcRenderer.send('sent', v),
});
