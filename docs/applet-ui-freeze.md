# Applet UI Freeze Maintenance (Code)

Status: delivered freeze (`HOST-UI1`); continuous maintenance (P2)

Canonical multi-project plan:
[`docs/desktop-applet-plugin-path.md`](../../../docs/desktop-applet-plugin-path.md).

## Ownership

Code freezes path-free `UiBinding` bytes and dependency edges into
`SessionCapabilityBatch`. It does not render, navigate, install packages, or
own CSP/origin policy.

## Mechanisms (P2)

1. Preserve `HOST-UI1` contracts (host handle, N/N+1 isolation, digest-bound
   entry/style/script).
2. Keep dependency edges limited to Tool / Skill / MCP / Flow; fail closed
   otherwise.
3. Session close cancels active UI host use and releases Use leases.
4. N handle retains N’s exact document across N+1 publish; missing lookup
   acquires no lease.

## Exit / regression gates

- `HOST-UI1` remains Delivered in [`ROADMAP.md`](../ROADMAP.md).
- Verification notes in `manual/CAPABILITY_VERIFICATION.md` and
  `manual/SCOPED_CAPABILITY_ARCHITECTURE.md` stay accurate for UI.
- Monorepo non-Desktop gate (includes Code `ui_binding` / `ui_projection`):
  `just test::applet-non-desktop` from the a3s root.

## Non-goals

- WebView, payment, camera, or install APIs
- Desktop shell catalog or Plugins UI
- Becoming a second package mutation path
