// OpenCode plugin: notify z sessions on key events.
//
// Fires when OpenCode finishes work, asks for user intervention, or errors.
// Resolves the target session from Z_SESSION_NAME (set by Zellij layout env block)
// with fallback to ZELLIJ_SESSION_NAME. If neither is set, the plugin is a no-op.
//
// Requires the `$` shell helper from OpenCode's plugin API (Bun template literals).

const DEDUPE_WINDOW_MS = 2000;
const lastNotificationByKey = new Map();

function sessionName() {
  return process.env.Z_SESSION_NAME || process.env.ZELLIJ_SESSION_NAME || "";
}

function eventSessionId(eventOrInput) {
  return eventOrInput?.properties?.sessionID || eventOrInput?.sessionID || "";
}

function shouldNotify(kind, eventOrInput) {
  const key = `${kind}:${eventSessionId(eventOrInput)}`;
  const now = Date.now();
  const last = lastNotificationByKey.get(key) || 0;
  if (now - last < DEDUPE_WINDOW_MS) return false;
  lastNotificationByKey.set(key, now);
  return true;
}

function level(eventType) {
  switch (eventType) {
    case "session.error":
      return "error";
    case "permission.asked":
      return "warning";
    default:
      return "info";
  }
}

function message(eventType) {
  switch (eventType) {
    case "session.idle":
      return "OpenCode finished working";
    case "permission.asked":
      return "OpenCode needs your permission";
    case "session.error":
      return "OpenCode encountered an error";
    default:
      return `OpenCode event: ${eventType}`;
  }
}

function isIdleEvent(event) {
  return (
    event.type === "session.idle" ||
    (event.type === "session.status" && event.properties?.status?.type === "idle")
  );
}

async function notify($, kind, eventOrInput, text, lvl) {
  const session = sessionName();
  if (!session) return; // no-op outside a z-managed session
  if (!shouldNotify(kind, eventOrInput)) return;

  try {
    await $`z notify ${session} ${text} --level ${lvl}`;
  } catch {
    // Silently ignore failures — the notification is best-effort.
  }
}

async function ZNotifyPlugin({ $ }) {
  return {
    event: async ({ event }) => {
      if (isIdleEvent(event)) {
        await notify($, "idle", event, message("session.idle"), "info");
        return;
      }

      if (event.type === "permission.asked" || event.type === "permission.ask") {
        await notify($, "permission", event, message("permission.asked"), "warning");
        return;
      }

      if (event.type === "session.error") {
        await notify($, "error", event, message(event.type), level(event.type));
      }
    },

    // OpenCode versions differ between `permission.asked` events and a direct
    // `permission.ask` hook. Supporting both keeps the notification best-effort
    // without changing the permission decision itself.
    "permission.ask": async (input) => {
      await notify($, "permission", input, message("permission.asked"), "warning");
    },
    "permission.asked": async (input) => {
      await notify($, "permission", input, message("permission.asked"), "warning");
    },
  };
}

export { ZNotifyPlugin };
export default ZNotifyPlugin;
