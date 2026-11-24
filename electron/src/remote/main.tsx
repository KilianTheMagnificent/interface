import { createRoot } from "react-dom/client";
import { RemoteApp } from "./RemoteApp";
import "../renderer/theme.css";
import "../renderer/codicons.css";
import "./remote.css";

document.documentElement.dataset.theme = "dark";
createRoot(document.getElementById("root")!).render(<RemoteApp />);
