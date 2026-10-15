import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { applyTheme, getStoredTheme } from "./theme";
import "./theme.css";
import "./styles.css";

// Apply the stored palette before React paints. The inline bootstrap in
// index.html already set the attribute to avoid a flash of the default theme;
// this keeps the meta theme-color in step and covers in-app reloads.
applyTheme(getStoredTheme());

const container = document.getElementById("root");
if (!container) throw new Error("missing #root element");

createRoot(container).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
