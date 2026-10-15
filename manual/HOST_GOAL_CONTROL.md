# Host-selected continuation bounds

Platform decision: `goal_control` belongs to the existing
`HOST_AUTHORITATIVE_BLOCKS` set (spec 018, T002/T017). Explicit host ACL
(`--config` / `A3S_CONFIG`) and the trusted user ACL may supply it.
Automatically discovered workspace ACL cannot enable, increase, disable,
or replace the host's policy. The existing host-layer filter enforces this
for ACP and the local TUI scaffold.

The declaration is optional. When absent, automatic continuation is disabled.
When present, all four fields are required, including when disabled:

```acl
goal_control {
  automatic_enabled = false
  max_slices = "8"
  max_no_progress_slices = "2"
  deadline_unix_ms = "1861920000000"
}
```

The three integers are canonical positive decimal strings; `max_slices` is at
most 1000 (checked at parse). A slice is admitted only while the deadline is
within 30 days of the host clock, so set it close to now before enabling work. The deadline is an absolute UTC Unix
timestamp in milliseconds; the host must choose a current finite deadline
before enabling work. Strings preserve integer precision in the ACL numeric
model. A closed-field check uses a3s-acl tokens before accepting the
AST, which otherwise erases duplicate fields. Unknown or duplicate fields,
missing values, labels, nested blocks, expressions, numeric values, signs,
leading zeroes and overflow are rejected. A file may declare this block once; separate configuration layers retain their original ordering
before workspace authority is withheld.

These are the existing `GoalContinuationPolicyV1` bounds. They add no token
or cost ledger, Run-internal continuation option, worker or scheduler.
Configuration is not execution permission: every automatic admission must
also pass the original current-source, ownership, permission, deadline,
historical-count and unsettled-work gates. Ordinary Runs do not consume an
automatic slice merely because this policy is configured.

Configuration is available on `CodeConfig`; `tick_goal` / `stream_goal_tick`
admit at most one Run per call under this policy. The separately pinned CLI and
SDK/product delivery qualification remain under the existing delivery plan.
No current user or repository configuration is changed by this declaration.
