//! Private observations of the original retention path, never write grants.
//!
//! These values do not parse proposals, validate items, call a model/store or
//! own extraction work. The original validator and writer retain those roles.

use super::MAX_RELATED_MEMORY_ITEMS;
use crate::memory::{MemoryExtractionTicket, MemoryWriteProgress};
use tokio_util::sync::CancellationToken;

const MAX_PRIMARY_ITEMS: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MemoryRetentionOriginV1 {
    ValidatedModel,
    #[cfg(test)]
    HostTyped,
}

impl MemoryRetentionOriginV1 {
    #[cfg(test)]
    pub(super) fn parse(value: &str) -> anyhow::Result<Self> {
        match value {
            "validated_model" => Ok(Self::ValidatedModel),
            "host_typed" => Ok(Self::HostTyped),
            _ => anyhow::bail!("MemoryRetentionOriginV1 refuses {value:?}"),
        }
    }

    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::ValidatedModel => "validated_model",
            #[cfg(test)]
            Self::HostTyped => "host_typed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MemoryRetentionDispositionV1 {
    NotAdmitted,
    NoItems,
    Stored,
    Refused,
    Partial,
    Unproved,
}

impl MemoryRetentionDispositionV1 {
    #[cfg(test)]
    pub(super) fn parse(value: &str) -> anyhow::Result<Self> {
        match value {
            "not_admitted" => Ok(Self::NotAdmitted),
            "no_items" => Ok(Self::NoItems),
            "stored" => Ok(Self::Stored),
            "refused" => Ok(Self::Refused),
            "partial" => Ok(Self::Partial),
            "unproved" => Ok(Self::Unproved),
            _ => anyhow::bail!("MemoryRetentionDispositionV1 refuses {value:?}"),
        }
    }

    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::NotAdmitted => "not_admitted",
            Self::NoItems => "no_items",
            Self::Stored => "stored",
            Self::Refused => "refused",
            Self::Partial => "partial",
            Self::Unproved => "unproved",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MemoryRetentionRefusalV1 {
    Disabled,
    EmptyTurn,
    #[cfg(test)]
    TurnNotCompleted,
    BudgetDenied,
    Cancelled,
    InvalidOutput,
    InvalidItem,
    ProviderFailed,
    AdmissionUnproved,
    StoreFailed,
    RelationUpdateFailed,
}

impl MemoryRetentionRefusalV1 {
    #[cfg(test)]
    pub(super) fn parse(value: &str) -> anyhow::Result<Self> {
        match value {
            "disabled" => Ok(Self::Disabled),
            "empty_turn" => Ok(Self::EmptyTurn),
            "turn_not_completed" => Ok(Self::TurnNotCompleted),
            "budget_denied" => Ok(Self::BudgetDenied),
            "cancelled" => Ok(Self::Cancelled),
            "invalid_output" => Ok(Self::InvalidOutput),
            "invalid_item" => Ok(Self::InvalidItem),
            "provider_failed" => Ok(Self::ProviderFailed),
            "admission_unproved" => Ok(Self::AdmissionUnproved),
            "store_failed" => Ok(Self::StoreFailed),
            "relation_update_failed" => Ok(Self::RelationUpdateFailed),
            _ => anyhow::bail!("MemoryRetentionRefusalV1 refuses {value:?}"),
        }
    }

    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::EmptyTurn => "empty_turn",
            #[cfg(test)]
            Self::TurnNotCompleted => "turn_not_completed",
            Self::BudgetDenied => "budget_denied",
            Self::Cancelled => "cancelled",
            Self::InvalidOutput => "invalid_output",
            Self::InvalidItem => "invalid_item",
            Self::ProviderFailed => "provider_failed",
            Self::AdmissionUnproved => "admission_unproved",
            Self::StoreFailed => "store_failed",
            Self::RelationUpdateFailed => "relation_update_failed",
        }
    }
}

/// A primary acknowledgement keeps the canonical ID returned by the store.
#[derive(Debug, Default)]
pub(super) struct PrimaryWriteProgress {
    pub(super) entered: bool,
    pub(super) canonical_id: Option<String>,
}

/// Archive `.store` returns unit, so its acknowledgement addresses the old ID
/// supplied by the host. It does not invent a canonical item from that result.
#[derive(Debug)]
pub(super) struct ArchiveWriteProgress {
    pub(super) primary_index: usize,
    pub(super) addressed_id: String,
    pub(super) entered: bool,
    pub(super) acknowledged: bool,
    pub(super) applied: Option<bool>,
}

/// Local bounded progress for one original extraction attempt. No ticket,
/// MemoryItem, store reference or callback is held here. Its origin is only a
/// diagnostic; constructing or parsing it cannot authorize an item.
#[derive(Debug)]
pub(super) struct MemoryRetentionProgress {
    origin: MemoryRetentionOriginV1,
    accepted_items: usize,
    rejected_items: usize,
    primary: Vec<PrimaryWriteProgress>,
    archives: Vec<ArchiveWriteProgress>,
    refusal: Option<MemoryRetentionRefusalV1>,
}

impl MemoryRetentionProgress {
    pub(super) fn new(
        origin: MemoryRetentionOriginV1,
        accepted_items: usize,
        rejected_items: usize,
    ) -> Option<Self> {
        // Only accepted items allocate write slots. Preserve the independently
        // counted rejected proposals exactly, without a cap or extra storage.
        if accepted_items > MAX_PRIMARY_ITEMS {
            return None;
        }
        Some(Self {
            origin,
            accepted_items,
            rejected_items,
            primary: (0..accepted_items)
                .map(|_| PrimaryWriteProgress::default())
                .collect(),
            archives: Vec::new(),
            refusal: None,
        })
    }

    pub(super) fn origin(&self) -> MemoryRetentionOriginV1 {
        self.origin
    }

    pub(super) fn item_counts(&self) -> (usize, usize) {
        (self.accepted_items, self.rejected_items)
    }

    pub(super) fn primary(&self) -> &[PrimaryWriteProgress] {
        &self.primary
    }

    pub(super) fn archives(&self) -> &[ArchiveWriteProgress] {
        &self.archives
    }

    /// Register an already validated exact relation before any await that
    /// could leave it unfinished. This is bookkeeping, not ID authorization.
    pub(super) fn require_archive(
        &mut self,
        primary_index: usize,
        addressed_id: String,
    ) -> Option<usize> {
        self.primary.get(primary_index)?;
        let mut count = 0;
        for archive in &self.archives {
            if archive.primary_index == primary_index {
                if archive.addressed_id == addressed_id {
                    return None;
                }
                count += 1;
            }
        }
        if count == MAX_RELATED_MEMORY_ITEMS {
            return None;
        }
        let index = self.archives.len();
        self.archives.push(ArchiveWriteProgress {
            primary_index,
            addressed_id,
            entered: false,
            acknowledged: false,
            applied: None,
        });
        Some(index)
    }

    pub(super) fn enter_primary(&mut self, index: usize) -> bool {
        let Some(primary) = self.primary.get_mut(index) else {
            return false;
        };
        if primary.entered {
            return false;
        }
        primary.entered = true;
        true
    }

    pub(super) fn acknowledge_primary(&mut self, index: usize, canonical_id: String) -> bool {
        let Some(primary) = self.primary.get_mut(index) else {
            return false;
        };
        if !primary.entered || primary.canonical_id.is_some() {
            return false;
        }
        primary.canonical_id = Some(canonical_id);
        true
    }

    pub(super) fn enter_archive(&mut self, index: usize) -> bool {
        let Some(archive) = self.archives.get_mut(index) else {
            return false;
        };
        if archive.entered || archive.applied.is_some() {
            return false;
        }
        archive.entered = true;
        true
    }

    pub(super) fn acknowledge_archive(&mut self, index: usize) -> bool {
        let Some(archive) = self.archives.get_mut(index) else {
            return false;
        };
        if !archive.entered || archive.acknowledged {
            return false;
        }
        archive.acknowledged = true;
        true
    }

    /// Preserve the original helper's returned true/false separately from
    /// the actual unit store Ack. None/missing/read failures enter no store.
    pub(super) fn finish_archive(&mut self, index: usize, applied: bool) -> bool {
        let Some(archive) = self.archives.get_mut(index) else {
            return false;
        };
        if archive.applied.is_some()
            || (applied && !archive.acknowledged)
            || (!applied && archive.entered)
        {
            return false;
        }
        archive.applied = Some(applied);
        true
    }

    pub(super) fn refuse(&mut self, reason: MemoryRetentionRefusalV1) {
        // Keep the first typed refusal; later acknowledgements remain intact.
        self.refusal.get_or_insert(reason);
    }

    pub(super) fn refusal(&self) -> Option<MemoryRetentionRefusalV1> {
        self.refusal
    }

    pub(super) fn entered_store_calls(&self) -> usize {
        self.primary.iter().filter(|item| item.entered).count()
            + self.archives.iter().filter(|item| item.entered).count()
    }

    pub(super) fn has_unproved_write(&self) -> bool {
        self.primary
            .iter()
            .any(|item| item.entered && item.canonical_id.is_none())
            || self
                .archives
                .iter()
                .any(|item| item.entered && !item.acknowledged)
    }

    pub(super) fn disposition(&self, valid_output: bool) -> MemoryRetentionDispositionV1 {
        let acknowledged = self.primary.iter().any(|item| item.canonical_id.is_some())
            || self.archives.iter().any(|item| item.acknowledged);
        if acknowledged {
            let complete = self.primary.iter().all(|item| item.canonical_id.is_some())
                && self
                    .archives
                    .iter()
                    .all(|item| item.acknowledged && item.applied == Some(true));
            if complete && self.rejected_items == 0 && self.refusal.is_none() {
                MemoryRetentionDispositionV1::Stored
            } else {
                MemoryRetentionDispositionV1::Partial
            }
        } else if self.has_unproved_write() {
            MemoryRetentionDispositionV1::Unproved
        } else if valid_output
            && self.accepted_items == 0
            && self.rejected_items == 0
            && self.refusal.is_none()
        {
            MemoryRetentionDispositionV1::NoItems
        } else {
            MemoryRetentionDispositionV1::Refused
        }
    }

    fn trace(&self, disposition: MemoryRetentionDispositionV1, session_id: &str, abandoned: bool) {
        let canonical_ids = self
            .primary
            .iter()
            .filter_map(|item| item.canonical_id.as_deref())
            .collect::<Vec<_>>();
        let (accepted_items, rejected_items) = self.item_counts();
        tracing::info!(
            target: "memory_retention",
            session_id,
            origin = self.origin().as_str(),
            disposition = disposition.as_str(),
            accepted_items, rejected_items,
            entered_store_calls = self.entered_store_calls(),
            canonical_ids = ?canonical_ids,
            archives = ?self.archives(),
            archive_entered = self.archives.iter().filter(|item| item.entered).count(),
            archive_acknowledged = self.archives.iter().filter(|item| item.acknowledged).count(),
            archive_applied = self.archives.iter().filter(|item| item.applied == Some(true)).count(),
            unproved_write = self.has_unproved_write(),
            refusal = self.refusal().map(MemoryRetentionRefusalV1::as_str),
            abandoned,
            "memory retention attempt"
        );
    }
}

/// The original ticket and local observations have exactly one owner. Drop
/// reports only what was synchronously recorded; it performs no async work.
pub(super) struct MemoryRetentionAttempt<'a> {
    ticket: MemoryExtractionTicket,
    pub(super) progress: MemoryRetentionProgress,
    cancel: &'a CancellationToken,
    session_id: &'a str,
    valid_output: bool,
    finished: bool,
}

impl<'a> MemoryRetentionAttempt<'a> {
    pub(super) fn new(
        ticket: MemoryExtractionTicket,
        cancel: &'a CancellationToken,
        session_id: &'a str,
    ) -> Self {
        Self {
            ticket,
            progress: MemoryRetentionProgress {
                origin: MemoryRetentionOriginV1::ValidatedModel,
                accepted_items: 0,
                rejected_items: 0,
                primary: Vec::new(),
                archives: Vec::new(),
                refusal: None,
            },
            cancel,
            session_id,
            valid_output: false,
            finished: false,
        }
    }

    pub(super) async fn wait_for_turn(&mut self) {
        self.ticket.wait_for_turn().await;
    }

    pub(super) fn validated(&mut self, accepted: usize, rejected: usize) -> bool {
        let Some(progress) = MemoryRetentionProgress::new(
            MemoryRetentionOriginV1::ValidatedModel,
            accepted,
            rejected,
        ) else {
            self.progress
                .refuse(MemoryRetentionRefusalV1::AdmissionUnproved);
            return false;
        };
        self.progress = progress;
        self.valid_output = true;
        if rejected != 0 {
            self.progress.refuse(MemoryRetentionRefusalV1::InvalidItem);
        }
        true
    }

    pub(super) fn finish(&mut self) {
        self.finished = true;
        self.progress.trace(
            self.progress.disposition(self.valid_output),
            self.session_id,
            false,
        );
    }
}

impl Drop for MemoryRetentionAttempt<'_> {
    fn drop(&mut self) {
        if !self.finished {
            self.progress.refuse(if self.cancel.is_cancelled() {
                MemoryRetentionRefusalV1::Cancelled
            } else {
                MemoryRetentionRefusalV1::AdmissionUnproved
            });
            self.progress.trace(
                self.progress.disposition(self.valid_output),
                self.session_id,
                true,
            );
        }
    }
}

pub(super) struct PrimaryProgress<'a>(&'a mut MemoryRetentionProgress, usize);

impl<'a> PrimaryProgress<'a> {
    pub(super) fn new(progress: &'a mut MemoryRetentionProgress, index: usize) -> Option<Self> {
        let primary = progress.primary.get(index)?;
        if primary.entered || primary.canonical_id.is_some() {
            return None;
        }
        Some(Self(progress, index))
    }
}

impl MemoryWriteProgress for PrimaryProgress<'_> {
    fn entered(&mut self) -> bool {
        let entered = self.0.enter_primary(self.1);
        if !entered {
            self.0.refuse(MemoryRetentionRefusalV1::AdmissionUnproved);
        }
        entered
    }
    fn acknowledged(&mut self, canonical_id: Option<&str>) {
        if !canonical_id.is_some_and(|id| self.0.acknowledge_primary(self.1, id.to_owned())) {
            self.0.refuse(MemoryRetentionRefusalV1::AdmissionUnproved);
        }
    }
}

pub(super) struct ArchiveProgress<'a>(&'a mut MemoryRetentionProgress, usize);

impl<'a> ArchiveProgress<'a> {
    pub(super) fn new(progress: &'a mut MemoryRetentionProgress, index: usize) -> Option<Self> {
        let archive = progress.archives.get(index)?;
        if archive.entered || archive.acknowledged || archive.applied.is_some() {
            return None;
        }
        Some(Self(progress, index))
    }
}

impl MemoryWriteProgress for ArchiveProgress<'_> {
    fn entered(&mut self) -> bool {
        let entered = self.0.enter_archive(self.1);
        if !entered {
            self.0.refuse(MemoryRetentionRefusalV1::AdmissionUnproved);
        }
        entered
    }
    fn acknowledged(&mut self, canonical_id: Option<&str>) {
        if canonical_id.is_some() || !self.0.acknowledge_archive(self.1) {
            self.0.refuse(MemoryRetentionRefusalV1::AdmissionUnproved);
        }
    }
}

pub(super) fn trace_not_admitted(reason: MemoryRetentionRefusalV1) {
    let progress = MemoryRetentionProgress {
        origin: MemoryRetentionOriginV1::ValidatedModel,
        accepted_items: 0,
        rejected_items: 0,
        primary: Vec::new(),
        archives: Vec::new(),
        refusal: Some(reason),
    };
    progress.trace(MemoryRetentionDispositionV1::NotAdmitted, "", false);
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod progress_tests;
