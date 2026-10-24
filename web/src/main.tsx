import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { App } from "./App";

const root = document.getElementById("root");
if (!root) {
  throw new Error("cista-web: 缺少 #root 挂载点");
}

createRoot(root).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
