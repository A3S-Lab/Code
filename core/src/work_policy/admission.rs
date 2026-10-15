//! Policy facts in the original thread; the caller retains original owner/writer admission.

use super::{WorkPolicyError, WorkPolicyV1, WORK_POLICY_SCHEMA};
use a3s_effect::{Fact, FileLog, FileLogWriterFence, LogStore, NewFact};
use serde::{Deserialize, Serialize};

pub(crate) const POLICY_KIND: &str = "a3s.work-policy.admitted";
const POLICY_KEY: &str = "work-policy";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PolicyFact {
    schema: String,
    thread_id: String,
    policy: WorkPolicyV1,
    digest: String,
}

pub(crate) struct PreparedPolicy {
    pub policy: Option<WorkPolicyV1>,
    thread_id: String,
    pending: Option<NewFact>,
}

impl PreparedPolicy {
    pub(crate) fn is_pending(&self) -> bool {
        self.pending.is_some()
    }
}

pub(crate) fn prepare_policy(
    facts: &[Fact],
    thread: &str,
    requested: Option<&WorkPolicyV1>,
) -> Result<PreparedPolicy, WorkPolicyError> {
    let mut admitted = None;
    for (index, fact) in facts.iter().enumerate() {
        if fact.kind.starts_with("a3s.work-policy.") && fact.kind != POLICY_KIND {
            return Err(WorkPolicyError::Corrupt("unsupported policy fact kind"));
        }
        if fact.kind != POLICY_KIND {
            continue;
        }
        if admitted.is_some()
            || fact.key != POLICY_KEY
            || index == 0
            || facts[..index]
                .iter()
                .any(|fact| fact.kind != crate::standing_context::STANDING_CONTEXT_FACT_KIND)
        {
            return Err(WorkPolicyError::Corrupt(
                "invalid policy admission order or key",
            ));
        }
        let stored = PolicyFact::deserialize(&fact.payload)
            .map_err(|_| WorkPolicyError::Corrupt("invalid policy fact payload"))?;
        if stored.schema != WORK_POLICY_SCHEMA
            || stored.thread_id != thread
            || stored.digest != stored.policy.digest()?
        {
            return Err(WorkPolicyError::Corrupt(
                "policy schema, thread or digest mismatch",
            ));
        }
        admitted = Some(stored.policy);
    }
    if let Some(policy) = admitted {
        if requested.is_some_and(|requested| requested != &policy) {
            return Err(WorkPolicyError::Conflict);
        }
        return Ok(PreparedPolicy {
            policy: Some(policy),
            thread_id: thread.into(),
            pending: None,
        });
    }
    let Some(policy) = requested else {
        return Ok(PreparedPolicy {
            policy: None,
            thread_id: thread.into(),
            pending: None,
        });
    };
    if facts
        .iter()
        .any(|fact| fact.kind != crate::standing_context::STANDING_CONTEXT_FACT_KIND)
    {
        return Err(WorkPolicyError::LegacyRebind);
    }
    let payload = PolicyFact {
        schema: WORK_POLICY_SCHEMA.into(),
        thread_id: thread.into(),
        policy: policy.clone(),
        digest: policy.digest()?,
    };
    Ok(PreparedPolicy {
        policy: Some(policy.clone()),
        thread_id: thread.into(),
        pending: Some(NewFact {
            kind: POLICY_KIND.into(),
            key: POLICY_KEY.into(),
            payload: serde_json::to_value(payload).map_err(WorkPolicyError::Encoding)?,
        }),
    })
}

pub(crate) fn commit_policy(
    log: &FileLog,
    writer: &FileLogWriterFence,
    thread: &str,
    prepared: &PreparedPolicy,
) -> Result<(), WorkPolicyError> {
    if thread != prepared.thread_id {
        return Err(WorkPolicyError::Invalid {
            field: "thread",
            reason: "must match the prepared original thread",
        });
    }
    if !writer.is_for(log).map_err(WorkPolicyError::Log)? {
        return Err(WorkPolicyError::Invalid {
            field: "writer",
            reason: "must be the original admitted log",
        });
    }
    if prepared.pending.is_some() {
        let facts = log.read(thread).map_err(WorkPolicyError::Log)?;
        if facts.is_empty() {
            return Err(WorkPolicyError::Corrupt(
                "standing admission must precede policy",
            ));
        }
        let verified = prepare_policy(&facts, thread, prepared.policy.as_ref())?;
        if let Some(fact) = verified.pending {
            log.append(thread, &[fact], None)
                .map_err(WorkPolicyError::Log)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
