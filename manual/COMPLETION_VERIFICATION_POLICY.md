# Host completion verification policy

The default is `CompletionVerificationPolicy::RequireVerification`.
An embedding host can set `SessionOptions::with_completion_verification_policy(CompletionVerificationPolicy::WaiveMissingVerification)` when it intentionally delegates verification outside the agent session.

The result remains `Waived`, not `Verified`. The host must inspect the completion terminal, not infer a verification pass from a successful API return. This setting grants no tool permissions and does not acknowledge unfinished child tasks, unresolved tool effects or incomplete observations. A report for the current effect must be resolved; only missing evidence is waived. Explicit digest-bound waivers retain their existing independent semantics.

The model must provide its final response. This setting never turns a tool result alone into a successful final answer. It is a session-level host API, not a workspace-controlled setting or process-wide environment variable.
