//! Immediate placement made a3s-4.7 reason more in step replay; one call later it reasoned
//! about a third less, so `delay` defaults to 1.

use std::collections::VecDeque;

use crate::util::config::LongReasoningReminderSettings;
use a3s_code_config_types::BoolFlag;

pub(crate) const ENV: &str = "GROK_LONG_REASONING_REMINDER";
pub(crate) const DEFAULT_TOKENS: u32 = 1000;
pub(crate) const DEFAULT_DELAY: u32 = 1;
/// Below this a routine step counts as long; above it the reminder never fires in practice.
pub(crate) const TOKENS_RANGE: std::ops::RangeInclusive<u32> = 100..=200_000;
/// A large delay would land the reminder long after the step it is about, so it is capped.
pub(crate) const DELAY_RANGE: std::ops::RangeInclusive<u32> = 0..=10;

pub(crate) const REMINDER: &str = "Your previous step used an excessively long hidden reasoning trace. \
     Stop doing that. From this point on you must not reason at length: read the result, decide \
     the single next action, and emit it immediately, in a few sentences of reasoning at most. \
     Long reasoning is treated as a task failure regardless of the outcome. Verification is still \
     required, but verify by running a check or reading a result, never by thinking at length.";

/// Tuning resolves even when disabled so a control session tallies `long_calls` against the same threshold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LongReasoningReminder {
    pub(crate) enabled: bool,
    pub(crate) tokens: u32,
    pub(crate) delay: u32,
}

impl LongReasoningReminder {
    pub(crate) fn resolve(
        local: &LongReasoningReminderSettings,
        remote: Option<&LongReasoningReminderSettings>,
    ) -> Self {
        Self::resolve_layers(
            env_settings(std::env::var(ENV).ok().as_deref()).as_ref(),
            local,
            remote,
        )
    }

    pub(crate) fn resolve_layers(
        env: Option<&LongReasoningReminderSettings>,
        local: &LongReasoningReminderSettings,
        remote: Option<&LongReasoningReminderSettings>,
    ) -> Self {
        let field = |get: fn(&LongReasoningReminderSettings) -> Option<u32>| {
            env.and_then(get)
                .or_else(|| get(local))
                .or_else(|| remote.and_then(get))
        };
        Self {
            enabled: BoolFlag::env_value(env.and_then(|e| e.enabled))
                .config(local.enabled)
                .feature_flag(remote.and_then(|r| r.enabled))
                .default(false)
                .resolve()
                .value,
            tokens: field(|s| s.tokens).map_or(DEFAULT_TOKENS, |t| {
                t.clamp(*TOKENS_RANGE.start(), *TOKENS_RANGE.end())
            }),
            delay: field(|s| s.delay).map_or(DEFAULT_DELAY, |d| {
                d.clamp(*DELAY_RANGE.start(), *DELAY_RANGE.end())
            }),
        }
    }

    fn history_len(self) -> usize {
        self.delay as usize + 1
    }
}

/// Env tier: a JSON object in the shared shape, or a bool word for `enabled` alone; anything else is unset.
pub(crate) fn env_settings(raw: Option<&str>) -> Option<LongReasoningReminderSettings> {
    let raw = raw?.trim();
    if raw.is_empty() {
        return None;
    }
    if raw.starts_with('{') {
        return match serde_json::from_str::<LongReasoningReminderSettings>(raw) {
            Ok(settings) => Some(settings),
            Err(error) => {
                tracing::warn!(%error, "ignoring malformed {ENV}: expected a JSON object");
                None
            }
        };
    }
    let enabled = match raw.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" | "enabled" => true,
        "0" | "false" | "no" | "off" | "disabled" => false,
        _ => return None,
    };
    Some(LongReasoningReminderSettings {
        enabled: Some(enabled),
        ..Default::default()
    })
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct LongReasoningTurnState {
    recent_calls: VecDeque<u32>,
    /// A retry re-enters the loop without a new call; the same call must not fire twice.
    reminded_at_call: u32,
    /// Due while a Length continuation was pending; history may evict the call before it can inject.
    pending: Option<u32>,
    pub(crate) model_calls: u32,
    pub(crate) reasoning_tokens: u64,
    pub(crate) completion_tokens: u64,
    pub(crate) max_call_reasoning_tokens: u32,
    pub(crate) long_calls: u32,
    pub(crate) reminders_fired: u32,
    /// Captured at turn start so a cancel can emit the torn-down turn's coordinates without async reads.
    pub(crate) turn_number: u64,
    pub(crate) model: String,
}

impl LongReasoningTurnState {
    pub(crate) fn begin_turn(&mut self, turn_number: u64, model: String) {
        *self = Self {
            turn_number,
            model,
            ..Default::default()
        };
    }

    pub(crate) fn finish_turn(&mut self) -> Self {
        std::mem::take(self)
    }

    pub(crate) fn record_call(
        &mut self,
        policy: LongReasoningReminder,
        reasoning_tokens: u32,
        completion_tokens: u32,
    ) {
        self.model_calls += 1;
        self.reasoning_tokens += u64::from(reasoning_tokens);
        self.completion_tokens += u64::from(completion_tokens);
        self.max_call_reasoning_tokens = self.max_call_reasoning_tokens.max(reasoning_tokens);
        if reasoning_tokens > policy.tokens {
            self.long_calls += 1;
        }
        self.recent_calls.push_back(reasoning_tokens);
        while self.recent_calls.len() > policy.history_len() {
            self.recent_calls.pop_front();
        }
    }

    pub(crate) fn take_due_reminder(&mut self, policy: LongReasoningReminder) -> Option<u32> {
        let long_call = self.pending.take().or_else(|| self.due(policy))?;
        self.reminders_fired += 1;
        Some(long_call)
    }

    pub(crate) fn defer_due_reminder(&mut self, policy: LongReasoningReminder) {
        if let Some(long_call) = self.due(policy) {
            self.pending = Some(long_call);
        }
    }

    fn due(&mut self, policy: LongReasoningReminder) -> Option<u32> {
        if !policy.enabled
            || self.model_calls == self.reminded_at_call
            || self.recent_calls.len() < policy.history_len()
        {
            return None;
        }
        let idx = self.recent_calls.len() - policy.history_len();
        let long_call = *self.recent_calls.get(idx)?;
        if long_call <= policy.tokens {
            return None;
        }
        self.reminded_at_call = self.model_calls;
        Some(long_call)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reminder_timing_refire_guard_and_disabled_tally() {
        let on = LongReasoningReminder {
            enabled: true,
            tokens: 2000,
            delay: 1,
        };
        let mut s = LongReasoningTurnState::default();
        s.record_call(on, 5000, 10);
        assert_eq!(None, s.take_due_reminder(on), "delay=1 waits one more call");
        s.record_call(on, 10, 10);
        assert_eq!(Some(5000), s.take_due_reminder(on));
        assert_eq!(
            None,
            s.take_due_reminder(on),
            "a retry without a new call does not re-fire"
        );
        s.record_call(on, 10, 10);
        assert_eq!(None, s.take_due_reminder(on));
        assert_eq!((3, 1), (s.model_calls, s.reminders_fired));

        // Length salvage: due while awaiting continuation, evicted by the continuation's sample,
        // still injected once normal sampling resumes.
        let mut s = LongReasoningTurnState::default();
        s.record_call(on, 5000, 10);
        s.record_call(on, 10, 10);
        s.defer_due_reminder(on);
        s.record_call(on, 10, 10);
        assert_eq!(Some(5000), s.take_due_reminder(on));
        assert_eq!(None, s.take_due_reminder(on));

        let off = LongReasoningReminder {
            enabled: false,
            ..on
        };
        let mut s = LongReasoningTurnState::default();
        s.record_call(off, 5000, 100);
        s.record_call(off, 10, 20);
        assert_eq!(None, s.take_due_reminder(off));
        assert_eq!(
            (2, 5010, 120, 5000, 1, 0),
            (
                s.model_calls,
                s.reasoning_tokens,
                s.completion_tokens,
                s.max_call_reasoning_tokens,
                s.long_calls,
                s.reminders_fired
            )
        );
    }
}
