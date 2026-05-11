// OpenCode plugin: publish OpenCode activity to z.
//
// Emits structured `z notify --event` updates when OpenCode works, waits for
// user intervention, or errors.
// Resolves the target session from Z_SESSION_NAME (set by Zellij layout env block)
// with fallback to ZELLIJ_SESSION_NAME. If neither is set, the plugin is a no-op.
//
// Requires the `$` shell helper from OpenCode's plugin API (Bun template literals).

const DEDUPE_WINDOW_MS = 2000;
const TOOL = "opencode";
const lastNotificationByKey = new Map();

function sessionName() {
  return process.env.Z_SESSION_NAME || process.env.ZELLIJ_SESSION_NAME || "";
}

function eventSessionId(eventOrInput) {
  return eventOrInput?.properties?.sessionID || eventOrInput?.sessionID || "";
}

function shouldSend(kind, eventOrInput) {
  const key = `${kind}:${eventSessionId(eventOrInput)}`;
  const now = Date.now();
  const last = lastNotificationByKey.get(key) || 0;
  if (now - last < DEDUPE_WINDOW_MS) return false;
  lastNotificationByKey.set(key, now);
  return true;
}

function statusType(event) {
  return event?.properties?.status?.type || event?.status?.type || "";
}

function permissionStatus(eventOrInput) {
  return (
    eventOrInput?.properties?.status ||
    eventOrInput?.status ||
    eventOrInput?.output?.status ||
    ""
  );
}

function isPermissionAskLike(eventOrInput) {
  const type = eventOrInput?.type || "";
  if (type === "permission.asked" || type === "permission.ask") return true;
  if (type !== "permission.updated" && type !== "permission.replied") return false;
  return permissionStatus(eventOrInput) === "ask";
}

async function notifyActivity($, kind, eventOrInput, eventName) {
  const session = sessionName();
  if (!session) return; // no-op outside a z-managed session
  if (!shouldSend(kind, eventOrInput)) return;

  try {
    await $`z notify --event ${eventName} --tool ${TOOL} --session ${session}`;
  } catch {
    // Silently ignore failures — activity reporting is best-effort.
  }
}

async function notifyWaiting($, kind, eventOrInput, reason, text, level) {
  const session = sessionName();
  if (!session) return; // no-op outside a z-managed session
  if (!shouldSend(kind, eventOrInput)) return;

  try {
    await $`z notify --event llm.waiting --tool ${TOOL} --session ${session} --reason ${reason} --message ${text} --level ${level}`;
  } catch {
    // Silently ignore failures — activity reporting is best-effort.
  }
}

async function ZNotifyPlugin({ $ }) {
  return {
    event: async ({ event }) => {
      if (event.type === "session.status" && statusType(event) === "busy") {
        await notifyActivity($, "working", event, "llm.working");
        return;
      }

      if (event.type === "session.idle") {
        await notifyWaiting($, "input", event, "input", "OpenCode is waiting for input", "info");
        return;
      }

      if (event.type === "session.status" && statusType(event) === "idle") {
        // `session.status idle` is a technical engine state and can be emitted
        // around the same turn as `session.idle`. Do not map it to `llm.idle`,
        // because that would erase the user-visible “waiting for input” state.
        return;
      }

      if (event.type === "question.asked") {
        await notifyWaiting($, "question", event, "question", "OpenCode is waiting for an answer", "info");
        return;
      }

      if (isPermissionAskLike(event)) {
        await notifyWaiting($, "permission", event, "permission", "OpenCode needs permission", "warning");
        return;
      }

      if (event.type === "session.error") {
        await notifyWaiting($, "error", event, "error", "OpenCode encountered an error", "error");
      }
    },

    // OpenCode versions differ between `permission.asked` events and a direct
    // `permission.ask` hook. Supporting both keeps the notification best-effort
    // without changing the permission decision itself.
    "permission.ask": async (input) => {
      await notifyWaiting($, "permission", input, "permission", "OpenCode needs permission", "warning");
    },
    "permission.asked": async (input) => {
      await notifyWaiting($, "permission", input, "permission", "OpenCode needs permission", "warning");
    },
  };
}

export { ZNotifyPlugin };
export default ZNotifyPlugin;
