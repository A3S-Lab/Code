//! Immutable references to the original policy/Goal source, never another content store.

use crate::execution_identity::ExecutionIdentityV1;
use a3s_effect::Fact;
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

const SOURCE_DOMAIN: &str = "a3s.code.session-fact-source.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionFactSourceV1 {
    pub sequence: u64,
    pub identity: ExecutionIdentityV1,
}

impl SessionFactSourceV1 {
    pub fn validate(&self) -> Result<()> {
        self.identity.validate()?;
        if self.sequence == 0 || self.identity.domain != SOURCE_DOMAIN {
            bail!("invalid durable session fact source");
        }
        Ok(())
    }

    pub(crate) fn from_facts(session_id: &str, facts: &[Fact]) -> Result<Option<Self>> {
        // A slice closeout is not definition authority; old checkpoints stay current.
        let Some(index) = facts.iter().rposition(|fact| {
            fact.kind.starts_with("a3s.work-policy.")
                || (fact.kind.starts_with("a3s.goal.")
                    && fact.kind != crate::goal_control::GoalFactKindV1::SliceSettled.as_str())
        }) else {
            return Ok(None);
        };
        let source = Self {
            sequence: facts[index].seq,
            identity: ExecutionIdentityV1::derive(SOURCE_DOMAIN, &(session_id, &facts[..=index]))?,
        };
        source.validate()?;
        Ok(Some(source))
    }

    /// Later facts are allowed; the saved original prefix must remain byte-identical.
    pub(crate) fn verify(&self, session_id: &str, facts: &[Fact]) -> Result<()> {
        self.validate()?;
        let Some(index) = facts.iter().position(|fact| fact.seq == self.sequence) else {
            bail!("durable session fact source is missing");
        };
        if Self::from_facts(session_id, &facts[..=index])?.as_ref() != Some(self) {
            bail!("durable session fact source differs from the saved binding");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
