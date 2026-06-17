/**
 * Top-of-app header notification bar. Renders the global notification store
 * ([`./notifications`]) as thin, full-width, colour-coded rows above the app
 * interface. Severity → colour: info (accent), warn (amber), issue (red).
 */
import { For, Show } from "solid-js";
import Fa from "solid-fa";
import {
  faCircleExclamation,
  faCircleInfo,
  faTriangleExclamation,
  faXmark,
} from "@fortawesome/free-solid-svg-icons";

import { dismiss, notifications, type NotifySeverity } from "./notifications";

const ICON: Record<NotifySeverity, typeof faCircleInfo> = {
  info: faCircleInfo,
  warn: faTriangleExclamation,
  issue: faCircleExclamation,
};

export default function NotificationBar() {
  return (
    <Show when={notifications().length > 0}>
      <div class="notif-stack">
        <For each={notifications()}>
          {(n) => (
            <div class={`notif-bar notif-${n.severity}`} role="status">
              <span class="notif-icon">
                <Fa icon={ICON[n.severity]} />
              </span>
              <span class="notif-msg">{n.message}</span>
              <Show when={n.actionLabel}>
                <button class="notif-link" onClick={() => n.onAction?.()}>
                  {n.actionLabel}
                </button>
              </Show>
              <Show when={n.dismissible !== false}>
                <button class="notif-close" title="Dismiss" onClick={() => dismiss(n.id)}>
                  <Fa icon={faXmark} />
                </button>
              </Show>
            </div>
          )}
        </For>
      </div>
    </Show>
  );
}
