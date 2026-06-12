/**
 * SolidJS entry point. Mounts the <App/> root into index.html's #root.
 */
import { render } from "solid-js/web";
import App from "./App";
import "./styles.css";
import "./ui.css";

const root = document.getElementById("root");
if (root) {
  render(() => <App />, root);
}
