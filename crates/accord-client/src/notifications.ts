/**
 * Global app-notification store for the top header bar.
 *
 * A module-level Solid signal so any part of the app (mesh status, the voice
 * layer, etc.) can raise a banner without prop-drilling. The `<NotificationBar/>`
 * at the app root renders whatever is here. Three severities drive the colour:
 * `info` (neutral/accent), `warn` (amber), `issue` (red).
 *
 * Use `key` for conditions that should not stack (e.g. "yggdrasil not set up"):
 * pushing the same key replaces the existing entry, and `dismissKey` clears it
 * once the condition resolves. Use `notifyTransient` for short-lived messages
 * (e.g. "you're muted") that auto-dismiss.
 */
import { createSignal } from "solid-js";

export type NotifySeverity = "info" | "warn" | "issue";

export interface AppNotification {
  id: number;
  /** De-dupe key: pushing with an existing key replaces it (no stacking). */
  key?: string;
  severity: NotifySeverity;
  message: string;
  /** Optional inline action button. */
  actionLabel?: string;
  onAction?: () => void;
  /** Whether the × dismiss button shows (default true). */
  dismissible?: boolean;
}

const [items, setItems] = createSignal<AppNotification[]>([]);

/** Reactive list of active notifications (oldest first). */
export const notifications = items;

let nextId = 1;

/** Show a notification; returns its id. With `key` set, replaces any existing
 * notification carrying that key (so a repeated condition doesn't pile up). */
export function notify(n: Omit<AppNotification, "id">): number {
  const id = nextId++;
  setItems((prev) => {
    const base = n.key ? prev.filter((p) => p.key !== n.key) : prev;
    return [...base, { dismissible: true, ...n, id }];
  });
  return id;
}

/** Like {@link notify} but auto-dismisses after `ms` (a transient bar). */
export function notifyTransient(n: Omit<AppNotification, "id">, ms = 5000): number {
  const id = notify(n);
  setTimeout(() => dismiss(id), ms);
  return id;
}

/** Remove a notification by id. */
export function dismiss(id: number): void {
  setItems((prev) => prev.filter((p) => p.id !== id));
}

/** Remove the notification with this key, if present (condition resolved). */
export function dismissKey(key: string): void {
  setItems((prev) => prev.filter((p) => p.key !== key));
}
