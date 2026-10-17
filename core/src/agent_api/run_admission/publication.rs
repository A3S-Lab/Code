use super::*;

impl OriginalExecutionContext {
    pub(super) fn worker_state(&self) -> Result<std::sync::MutexGuard<'_, OriginalOwnerState>> {
        let state = self
            .state
            .lock()
            .map_err(|_| owner_error("poisoned original owner"))?;
        if state.phase != EffectPhase::MayHaveEffects || state.worker_id != tokio::task::try_id() {
            return Err(owner_error(
                "operation requires same entered original worker",
            ));
        }
        Ok(state)
    }

    pub(in crate::agent_api) fn validate_original_lineage(
        self: &Arc<Self>,
        session_id: &str,
        admission: &Arc<RunAdmission>,
        active: &Arc<AsyncMutex<Option<Arc<crate::run_control::RunControlInbox>>>>,
        current: &Arc<AsyncMutex<Option<String>>>,
        cancel: &Arc<AsyncMutex<Option<CancellationToken>>>,
    ) -> Result<()> {
        if self.session_id != session_id
            || !Arc::ptr_eq(active, &self.active_slot)
            || !Arc::ptr_eq(current, &self.current_slot)
            || !Arc::ptr_eq(cancel, &self.cancel_slot)
            || !admission
                .current_original()
                .as_ref()
                .is_some_and(|actual| Arc::ptr_eq(actual, self))
        {
            return Err(owner_error("foreign original registered lineage"));
        }
        let state = self
            .state
            .lock()
            .map_err(|_| owner_error("poisoned original lineage"))?;
        if state.release.retired {
            return Err(owner_error("original authority retired"));
        }
        Ok(())
    }

    pub(in crate::agent_api) fn validate_original_worker(&self) -> Result<()> {
        let _state = self.worker_state()?;
        Ok(())
    }
    pub(in crate::agent_api) fn created_handle(&self) -> Result<crate::run::RunHandle> {
        let state = self.worker_state()?;
        match &state.run {
            OriginalRunBinding::Created { handle, .. } => Ok(handle.clone()),
            _ => Err(owner_error("no actual Created handle")),
        }
    }
    pub(in crate::agent_api) async fn correlated_current_control(
        self: &Arc<Self>,
        session_id: &str,
        admission: &Arc<RunAdmission>,
    ) -> Option<Arc<crate::run_control::RunControlInbox>> {
        if self.session_id != session_id {
            return None;
        }
        let active = self.active_slot.lock().await;
        let current = self.current_slot.lock().await;
        let cancel = self.cancel_slot.lock().await;
        if !admission
            .current_original()
            .as_ref()
            .is_some_and(|actual| Arc::ptr_eq(actual, self))
        {
            return None;
        }
        let state = self.state.lock().ok()?;
        let OriginalRunBinding::Created { snapshot, handle } = &state.run else {
            return None;
        };
        let inbox = state.release.inbox.as_ref()?;
        if snapshot.session_id != self.session_id
            || snapshot.id != handle.id()
            || snapshot.id != inbox.snapshot_run_id()
        {
            return None;
        }
        if !state.release.retired && self.slots_match(&state, &active, &current, &cancel) {
            return Some(Arc::clone(inbox));
        }
        // The original admission still owns this once-bound, sealed inbox
        // after exact slot removal. It grants retries/refusals only, never a
        // current Run handle, cancellation pin or fresh execution fallback.
        (state.release.installed
            && state.release.slots_retired
            && state.release.sealed
            && state
                .release
                .controls
                .is_some_and(|actual| !actual.has_accepted())
            && active.is_none()
            && current.is_none()
            && cancel.is_none())
        .then(|| Arc::clone(inbox))
    }

    pub(in crate::agent_api) fn validate_original_token(
        &self,
        actual: &CancellationToken,
    ) -> Result<()> {
        let _state = self.worker_state()?;
        if *actual != self.original_cancel {
            return Err(owner_error("foreign original token"));
        }
        Ok(())
    }

    pub(in crate::agent_api) fn record_created_run(
        self: &Arc<Self>,
        snapshot: crate::run::RunSnapshot,
        handle: crate::run::RunHandle,
    ) -> Result<()> {
        let mut state = self.worker_state()?;
        if state.release.sealed
            || !matches!(state.run, OriginalRunBinding::Unobserved)
            || snapshot.session_id != self.session_id
            || snapshot.status != crate::run::RunStatus::Created
            || snapshot.id != handle.id()
            || snapshot.session_id != handle.session_id()
            || !handle.has_cancel_pin(&RunCancelPin::from_original(self))
        {
            return Err(owner_error(
                "actual Created original Run binding required once",
            ));
        }
        state.run = OriginalRunBinding::Created {
            snapshot: Box::new(snapshot),
            handle,
        };
        Ok(())
    }

    pub(in crate::agent_api) fn record_no_run_branch(&self) -> Result<()> {
        let mut state = self.worker_state()?;
        if state.release.sealed || !matches!(state.run, OriginalRunBinding::Unobserved) {
            return Err(owner_error("positive NoRun requires unconstructed branch"));
        }
        state.run = OriginalRunBinding::NoRun;
        Ok(())
    }

    pub(in crate::agent_api) fn bind_inbox_once(
        &self,
        actual: Arc<crate::run_control::RunControlInbox>,
    ) -> Result<()> {
        let mut state = self.worker_state()?;
        let OriginalRunBinding::Created { snapshot, .. } = &state.run else {
            return Err(owner_error("inbox requires actual Created Run"));
        };
        if state.release.sealed
            || state.release.no_inbox
            || state.release.inbox.is_some()
            || actual.snapshot_run_id() != snapshot.id
        {
            return Err(owner_error("foreign or duplicate original inbox"));
        }
        state.release.inbox = Some(actual);
        Ok(())
    }

    pub(in crate::agent_api) fn record_no_inbox(&self) -> Result<()> {
        let mut state = self.worker_state()?;
        if state.release.sealed
            || state.release.inbox.is_some()
            || state.release.installed
            || state.release.no_inbox
        {
            return Err(owner_error(
                "positive NoInbox requires actual unconstructed branch",
            ));
        }
        state.release.no_inbox = true;
        Ok(())
    }

    pub(in crate::agent_api) async fn attach_original_slots(
        self: &Arc<Self>,
        session_id: &str,
        admission: &Arc<RunAdmission>,
        run_id: &str,
        actual: Arc<crate::run_control::RunControlInbox>,
        closed: &AtomicBool,
    ) -> Result<()> {
        self.validate_original_lineage(
            session_id,
            admission,
            &self.active_slot,
            &self.current_slot,
            &self.cancel_slot,
        )?;
        let mut active = self.active_slot.lock().await;
        let mut current = self.current_slot.lock().await;
        let mut cancel = self.cancel_slot.lock().await;
        self.validate_original_lineage(
            session_id,
            admission,
            &self.active_slot,
            &self.current_slot,
            &self.cancel_slot,
        )?;
        let mut state = self.worker_state()?;
        let OriginalRunBinding::Created { snapshot, .. } = &state.run else {
            return Err(owner_error("owned attach without Created Run"));
        };
        if closed.load(Ordering::Acquire) {
            return Err(CodeError::SessionClosed {
                session_id: self.session_id.clone(),
            });
        }

        if snapshot.id != run_id
            || !state
                .release
                .inbox
                .as_ref()
                .is_some_and(|inbox| Arc::ptr_eq(inbox, &actual))
            || state.release.sealed
            || state.release.installed
            || active.is_some()
            || current.is_some()
            || cancel.is_some()
        {
            return Err(owner_error("owned attach requires original empty slots"));
        }
        *active = Some(actual);
        *current = Some(run_id.to_owned());
        *cancel = Some(self.original_cancel.clone());
        state.release.installed = true;
        Ok(())
    }

    pub(in crate::agent_api) async fn correlated_current_handle(
        self: &Arc<Self>,
        session_id: &str,
        admission: &Arc<RunAdmission>,
    ) -> Option<crate::run::RunHandle> {
        self.validate_original_lineage(
            session_id,
            admission,
            &self.active_slot,
            &self.current_slot,
            &self.cancel_slot,
        )
        .ok()?;
        let handle = {
            let state = self.state.lock().ok()?;
            let OriginalRunBinding::Created { handle, .. } = &state.run else {
                return None;
            };
            handle.clone()
        };
        let snapshot = handle.snapshot().await?;
        let active = self.active_slot.lock().await;
        let current = self.current_slot.lock().await;
        let cancel = self.cancel_slot.lock().await;
        self.validate_original_lineage(
            session_id,
            admission,
            &self.active_slot,
            &self.current_slot,
            &self.cancel_slot,
        )
        .ok()?;
        let state = self.state.lock().ok()?;
        if snapshot.id != handle.id()
            || snapshot.session_id != self.session_id
            || state.release.retired
            || !self.slots_match(&state, &active, &current, &cancel)
        {
            return None;
        }
        Some(handle)
    }

    pub(super) fn slots_match(
        &self,
        state: &OriginalOwnerState,
        active: &Option<Arc<crate::run_control::RunControlInbox>>,
        current: &Option<String>,
        cancel: &Option<CancellationToken>,
    ) -> bool {
        let OriginalRunBinding::Created { snapshot, .. } = &state.run else {
            return false;
        };
        state.release.installed
            && !state.release.slots_retired
            && state
                .release
                .inbox
                .as_ref()
                .zip(active.as_ref())
                .is_some_and(|(expected, actual)| Arc::ptr_eq(expected, actual))
            && current.as_deref() == Some(snapshot.id.as_str())
            && cancel.as_ref() == Some(&self.original_cancel)
    }

    pub(super) fn can_request_cancel(
        &self,
        state: &OriginalOwnerState,
        active: &Option<Arc<crate::run_control::RunControlInbox>>,
        current: &Option<String>,
        cancel: &Option<CancellationToken>,
        run_id: Option<&str>,
        origin: OriginalCancelOrigin,
    ) -> bool {
        if state.release.sealed
            || state.release.retired
            || state.phase == EffectPhase::WorkerFinished
        {
            return false;
        }
        let bound = match &state.run {
            OriginalRunBinding::Created { snapshot, .. } => Some(snapshot.id.as_str()),
            _ => None,
        };
        if run_id.is_some() && run_id != bound {
            return false;
        }
        if state.release.installed {
            self.slots_match(state, active, current, cancel)
        } else {
            origin == OriginalCancelOrigin::Close
                && active.is_none()
                && current.is_none()
                && cancel.is_none()
        }
    }

    pub(super) async fn request(
        self: &Arc<Self>,
        session_id: &str,
        run_id: Option<&str>,
        origin: OriginalCancelOrigin,
    ) -> bool {
        if self.session_id != session_id {
            return false;
        }
        let inbox = {
            let active = self.active_slot.lock().await;
            let current = self.current_slot.lock().await;
            let cancel = self.cancel_slot.lock().await;
            let Ok(state) = self.state.lock() else {
                return false;
            };
            if !self.can_request_cancel(&state, &active, &current, &cancel, run_id, origin) {
                return false;
            }
            state.release.inbox.clone()
        };
        // No Session slot or owner-state guard survives this inbox await.
        // Sealing preserves Accepted debt and exact retry receipts. The worker
        // retains the only input writer and later records its actual counts.
        if let Some(actual) = &inbox {
            actual.seal_new_requests().await;
        }
        let active = self.active_slot.lock().await;
        let current = self.current_slot.lock().await;
        let cancel = self.cancel_slot.lock().await;
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        let same_inbox = match (&inbox, &state.release.inbox) {
            (Some(expected), Some(actual)) => Arc::ptr_eq(expected, actual),
            (None, None) => true,
            _ => false,
        };
        if !same_inbox
            || !self.can_request_cancel(&state, &active, &current, &cancel, run_id, origin)
        {
            return false;
        }
        state.release.cancel_origin.get_or_insert(origin);
        self.original_cancel.cancel();
        true // No await after this synchronized acceptance.
    }

    pub(in crate::agent_api) async fn request_session_close(self: &Arc<Self>) -> bool {
        self.request(&self.session_id, None, OriginalCancelOrigin::Close)
            .await
    }

    pub(in crate::agent_api) fn reserve_committer_install(
        &self,
    ) -> Result<OriginalCommitterInstall<'_>> {
        let _state = self.worker_state()?;
        self.committer.reserve_install()
    }
    pub(in crate::agent_api) fn record_no_committer(&self) -> Result<()> {
        let _state = self.worker_state()?;
        self.committer.positively_disabled()
    }
    pub(in crate::agent_api) async fn wait_committer(&self) -> Result<ActualCommitterJoin> {
        {
            let _state = self.worker_state()?;
        }
        self.committer.wait().await
    }

    pub(in crate::agent_api) fn mark_scope_attempted(&self, kind: OriginalScopeKind) -> Result<()> {
        let mut state = self.worker_state()?;
        let slot = &mut state.release.scopes[kind.index()];
        if !matches!(slot, OriginalScopeObservation::Unobserved) {
            return Err(owner_error("scope attempt already observed"));
        }
        *slot = OriginalScopeObservation::Attempted;
        Ok(())
    }
    pub(in crate::agent_api) fn record_scope_returned(
        &self,
        kind: OriginalScopeKind,
        actual: &crate::capability::SessionCapabilityRun,
    ) -> Result<()> {
        let instance =
            actual.run_scope().handle().instance_id().map_err(|error| {
                owner_error(&format!("actual scope identity unavailable: {error}"))
            })?;
        let mut state = self.worker_state()?;
        let index = kind.index();
        if !matches!(state.release.scopes[index], OriginalScopeObservation::Attempted)
            || state.release.scopes.iter().any(|slot| matches!(slot, OriginalScopeObservation::Returned { instance: previous, .. } if *previous == instance))
        { return Err(owner_error("scope return requires distinct actual attempted guard")); }
        state.release.scopes[index] = OriginalScopeObservation::Returned {
            instance,
            close: None,
        };
        Ok(())
    }
    pub(in crate::agent_api) fn record_scope_close(
        &self,
        kind: OriginalScopeKind,
        actual: &std::result::Result<
            crate::capability::ScopeCloseReport,
            crate::capability::CapabilityRuntimeError,
        >,
    ) -> Result<()> {
        let mut state = self.worker_state()?;
        let OriginalScopeObservation::Returned { close, .. } =
            &mut state.release.scopes[kind.index()]
        else {
            return Err(owner_error("close without original returned scope"));
        };
        if close.is_some() {
            return Err(owner_error("scope close already observed"));
        }
        *close = Some(actual.clone());
        Ok(())
    }
    pub(in crate::agent_api) fn record_scope_not_attempted(
        &self,
        kind: OriginalScopeKind,
    ) -> Result<()> {
        let mut state = self.worker_state()?;
        let slot = &mut state.release.scopes[kind.index()];
        if !matches!(slot, OriginalScopeObservation::Unobserved) {
            return Err(owner_error("NotAttempted cannot replace scope attempt"));
        }
        *slot = OriginalScopeObservation::NotAttempted;
        Ok(())
    }
    pub(in crate::agent_api) async fn seal_original_controls(&self) -> Result<()> {
        let actual = {
            let state = self.worker_state()?;
            if state.release.controls.is_some() {
                return Err(owner_error("controls already observed"));
            }
            state.release.inbox.clone()
        };
        if let Some(actual) = actual {
            let observed = actual.seal_and_observe_pending().await;
            let mut state = self.worker_state()?;
            if !state
                .release
                .inbox
                .as_ref()
                .is_some_and(|inbox| Arc::ptr_eq(inbox, &actual))
            {
                return Err(owner_error("foreign sealed inbox"));
            }
            state.release.controls = Some(observed);
        } else {
            let state = self.worker_state()?;
            if !state.release.no_inbox {
                return Err(owner_error("missing inbox is unproved"));
            }
        }
        Ok(())
    }

    pub(in crate::agent_api) fn seal_publication(self: &Arc<Self>) -> Result<SealedPublication> {
        let mut state = self.worker_state()?;
        if state.release.sealed
            || state.release.retired
            || matches!(state.run, OriginalRunBinding::Unobserved)
        {
            return Err(owner_error(
                "publication requires once-bound original target",
            ));
        }
        state.release.cancelled_at_seal = self.original_cancel.is_cancelled();
        state.release.sealed = true;
        Ok(SealedPublication {
            original: Arc::downgrade(self),
        })
    }
    pub(super) fn check_ticket(&self, sealed: &SealedPublication) -> Result<()> {
        if !std::ptr::eq(sealed.original.as_ptr(), self) {
            return Err(owner_error("foreign publication ticket"));
        }
        Ok(())
    }
    pub(in crate::agent_api) fn publication_target(
        &self,
        sealed: &SealedPublication,
    ) -> Result<OriginalPublicationTarget> {
        self.check_ticket(sealed)?;
        let state = self.worker_state()?;
        if !state.release.sealed || state.release.publication_complete {
            return Err(owner_error("publication ticket no longer open"));
        }
        let run_id = match &state.run {
            OriginalRunBinding::Created { snapshot, .. } => Some(snapshot.id.clone()),
            OriginalRunBinding::NoRun => None,
            OriginalRunBinding::Unobserved => {
                return Err(owner_error("unobserved publication target"))
            }
        };
        Ok(OriginalPublicationTarget {
            session_id: self.session_id.clone(),
            run_id,
            cancelled: state.release.cancelled_at_seal,
            cancel_origin: state.release.cancel_origin,
        })
    }
    pub(in crate::agent_api) fn record_publication_progress(
        &self,
        sealed: &SealedPublication,
        actual: &OriginalPublicationObservation,
    ) -> Result<()> {
        self.check_ticket(sealed)?;
        let mut state = self.worker_state()?;
        retain_publication_progress(&mut state, actual)
    }
    pub(in crate::agent_api) fn record_publication(
        &self,
        sealed: SealedPublication,
        actual: OriginalPublicationObservation,
    ) -> Result<()> {
        self.record_publication_progress(&sealed, &actual)?;
        let mut state = self.worker_state()?;
        state.release.publication_complete = true;
        Ok(())
    }
    pub(in crate::agent_api) fn record_admission_cancel(&self) -> Result<()> {
        let mut state = self.worker_state()?;
        if state.release.sealed {
            return Err(owner_error("admission intent after publication seal"));
        }
        state
            .release
            .cancel_origin
            .get_or_insert(OriginalCancelOrigin::Admission);
        self.original_cancel.cancel();
        Ok(())
    }
    pub(in crate::agent_api) async fn retire_original_slots(
        self: &Arc<Self>,
        session_id: &str,
        admission: &Arc<RunAdmission>,
        run_id: &str,
    ) -> Result<()> {
        self.validate_original_lineage(
            session_id,
            admission,
            &self.active_slot,
            &self.current_slot,
            &self.cancel_slot,
        )?;
        drop(self.worker_state()?);
        {
            // Never wait while holding the three slot guards. A live
            // reservation/borrow refuses; it is not a terminal observation.
            let mut model = self.model_call.state.try_lock().map_err(|_| {
                owner_error("original cleanup proof incomplete; zero slot mutation")
            })?;
            if OriginalModelCell::observe_join(&mut model).await == ModelCallJoin::Unproved {
                return Err(owner_error(
                    "original cleanup proof incomplete; zero slot mutation",
                ));
            }
        }
        let mut active = self.active_slot.lock().await;
        let mut current = self.current_slot.lock().await;
        let mut cancel = self.cancel_slot.lock().await;
        self.validate_original_lineage(
            session_id,
            admission,
            &self.active_slot,
            &self.current_slot,
            &self.cancel_slot,
        )?;
        let mut state = self.worker_state()?;
        if !state.release.publication_complete || state.release.slots_retired {
            return Err(owner_error(
                "retirement before final original publication or repeated",
            ));
        }
        if !self.model_call.has_terminal_observation()
            || !self.committer.has_completed_observation()
            || !state
                .release
                .scopes
                .iter()
                .all(OriginalScopeObservation::clean)
            || !(state.release.no_inbox
                || state
                    .release
                    .controls
                    .is_some_and(|actual| !actual.has_accepted()))
            || !publication_proved(&state, &self.session_id)
        {
            return Err(owner_error(
                "original cleanup proof incomplete; zero slot mutation",
            ));
        }
        if let OriginalRunBinding::Created { snapshot, .. } = &state.run {
            if snapshot.id != run_id {
                return Err(owner_error("foreign cleanup Run"));
            }
        }
        if state.release.installed {
            if !self.slots_match(&state, &active, &current, &cancel) {
                return Err(owner_error("foreign cleanup slot; zero mutation"));
            }
            *active = None;
            *current = None;
            *cancel = None;
        } else if active.is_some()
            || current.is_some()
            || cancel.is_some()
            || !state.release.no_inbox
        {
            return Err(owner_error(
                "NotInstalled requires original unconstructed branch and empty slots",
            ));
        }
        state.release.slots_retired = true;
        Ok(())
    }
    pub(in crate::agent_api) fn record_tools_cleaned(&self) -> Result<()> {
        let mut state = self.worker_state()?;
        if !state.release.slots_retired || state.release.tools_cleaned {
            return Err(owner_error(
                "tools cleanup requires exact once-only retirement",
            ));
        }
        state.release.tools_cleaned = true;
        Ok(())
    }
    pub(super) fn qualify_release(
        &self,
        actual_id: tokio::task::Id,
        model: ModelCallJoin,
        committer: Result<ActualCommitterJoin>,
    ) -> OwnerDisposition {
        let Ok(mut state) = self.state.lock() else {
            return OwnerDisposition::RetainedUnproved;
        };
        let positive = state.worker_id == Some(actual_id)
            && matches!(state.worker_join, Some(Ok(())))
            && matches!(state.start, StartObservation::Received)
            && model != ModelCallJoin::Unproved
            && matches!(
                committer,
                Ok(ActualCommitterJoin::PositivelyDisabled
                    | ActualCommitterJoin::Joined(ActualJoinObservation::Completed))
            )
            && state
                .release
                .scopes
                .iter()
                .all(OriginalScopeObservation::clean)
            && (state.release.no_inbox
                || state
                    .release
                    .controls
                    .as_ref()
                    .is_some_and(|actual| !actual.has_accepted()))
            && publication_proved(&state, &self.session_id)
            && state.release.slots_retired
            && state.release.tools_cleaned;
        let disposition = if positive {
            state.release.retired = true;
            OwnerDisposition::Released
        } else {
            OwnerDisposition::RetainedUnproved
        };
        state.disposition = Some(disposition);
        disposition
    }
}

impl OriginalScopeKind {
    pub(super) fn index(self) -> usize {
        match self {
            Self::Command => 0,
            Self::Fact => 1,
        }
    }
}

pub(super) fn validate_publication_progress(
    previous: &OriginalPublicationObservation,
    actual: &OriginalPublicationObservation,
) -> Result<()> {
    fn store(old: &OriginalStoreObservation, new: &OriginalStoreObservation) -> bool {
        match old {
            OriginalStoreObservation::NotAttempted => matches!(
                new,
                OriginalStoreObservation::NotAttempted | OriginalStoreObservation::Entered
            ),
            OriginalStoreObservation::Entered => matches!(
                new,
                OriginalStoreObservation::Entered | OriginalStoreObservation::Returned(_)
            ),
            OriginalStoreObservation::NotApplicable => {
                matches!(new, OriginalStoreObservation::NotApplicable)
            }
            OriginalStoreObservation::Returned(old) => {
                matches!(new, OriginalStoreObservation::Returned(new) if match (old.as_ref(),new.as_ref()) { (None,None) => true, (Some(old),Some(new)) => old.id == new.id && old.session_id == new.session_id && old.status == new.status && old.event_count == new.event_count && old.updated_at_ms == new.updated_at_ms, _ => false })
            }
        }
    }
    fn hook(old: OriginalHookObservation, new: OriginalHookObservation) -> bool {
        match old {
            OriginalHookObservation::NotAttempted => matches!(
                new,
                OriginalHookObservation::NotAttempted | OriginalHookObservation::Entered
            ),
            OriginalHookObservation::Entered => matches!(
                new,
                OriginalHookObservation::Entered | OriginalHookObservation::Returned
            ),
            other => other == new,
        }
    }
    if previous.kind != actual.kind
        || !store(&previous.terminal, &actual.terminal)
        || !store(&previous.diagnostic, &actual.diagnostic)
        || !hook(previous.cancellation_hook, actual.cancellation_hook)
        || !hook(previous.event_hook, actual.event_hook)
    {
        return Err(owner_error(
            "publication progress cannot erase actual earlier return",
        ));
    }
    Ok(())
}

pub(super) fn publication_proved(state: &OriginalOwnerState, session_id: &str) -> bool {
    if !state.release.publication_complete {
        return false;
    }
    let Some(actual) = &state.release.publication else {
        return false;
    };
    if matches!(state.run, OriginalRunBinding::NoRun) {
        return actual.kind == OriginalPublicationKind::NoRun
            && matches!(actual.terminal, OriginalStoreObservation::NotApplicable)
            && matches!(actual.diagnostic, OriginalStoreObservation::NotApplicable)
            && actual.cancellation_hook == OriginalHookObservation::NotApplicable
            && actual.event_hook == OriginalHookObservation::NotApplicable;
    }
    let OriginalRunBinding::Created { snapshot, .. } = &state.run else {
        return false;
    };
    let ack = |slot: &OriginalStoreObservation, status: crate::run::RunStatus, event: bool| matches!(slot, OriginalStoreObservation::Returned(actual) if actual.as_ref().as_ref().is_some_and(|returned| returned.id == snapshot.id && returned.session_id == session_id && returned.status == status && (!event || returned.event_count > snapshot.event_count)));
    let hook = |slot| match state.release.hooks_configured {
        Some(true) => slot == OriginalHookObservation::Returned,
        Some(false) => slot == OriginalHookObservation::NotApplicable,
        None => false,
    };
    let cancellation_hook = if state.release.cancel_origin.is_some() {
        hook(actual.cancellation_hook)
    } else {
        actual.cancellation_hook == OriginalHookObservation::NotApplicable
    };
    if !cancellation_hook {
        return false;
    }
    let no_diagnostic = matches!(actual.diagnostic, OriginalStoreObservation::NotApplicable);
    match actual.kind {
        OriginalPublicationKind::End => {
            !state.release.cancelled_at_seal
                && ack(&actual.terminal, crate::run::RunStatus::Completed, true)
                && no_diagnostic
                && hook(actual.event_hook)
        }
        OriginalPublicationKind::Error => {
            if state.release.cancelled_at_seal {
                ack(&actual.terminal, crate::run::RunStatus::Cancelled, false)
                    && ack(&actual.diagnostic, crate::run::RunStatus::Cancelled, true)
                    && hook(actual.event_hook)
            } else {
                ack(&actual.terminal, crate::run::RunStatus::Failed, true)
                    && no_diagnostic
                    && hook(actual.event_hook)
            }
        }
        OriginalPublicationKind::Cancelled => {
            state.release.cancelled_at_seal
                && ack(&actual.terminal, crate::run::RunStatus::Cancelled, false)
                && no_diagnostic
                && actual.event_hook == OriginalHookObservation::NotApplicable
        }
        OriginalPublicationKind::AdmissionFailed => {
            !state.release.cancelled_at_seal
                && ack(&actual.terminal, crate::run::RunStatus::Failed, false)
                && no_diagnostic
                && actual.event_hook == OriginalHookObservation::NotApplicable
        }
        OriginalPublicationKind::NoRun => false,
    }
}
