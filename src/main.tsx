import React from "react";
import ReactDOM from "react-dom/client";
import App, { ToolbarApp } from "./App";

const isToolbar =
  new URLSearchParams(window.location.search).get("window") === "toolbar";

const Root = isToolbar ? ToolbarApp : App;

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <Root />
  </React.StrictMode>,
);
