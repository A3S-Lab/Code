use super::*;

// The [process start, first phase] gap, recorded once per launch even across a fallback begin().
pub(crate) static PROCESS_INIT_RECORDED: AtomicBool = AtomicBool::new(false);

/// The untimed [last completed phase, first confirmed frame] gap. The states make the ordering explicit: the
/// settle span can open only after a phase has ended, and the gap records once per launch.
pub(crate) enum FrameGap {
    /// No phase has ended yet.
    Idle,
    /// A phase ended at this instant; the settle span has not opened.
    PhaseEnded(Instant),
    /// The settle span is open, timing the wait since the last phase end.
    Settling { since: Instant, span: tracing::Span },
    /// The gap has been recorded; later signals are ignored.
    Recorded,
}

static FRAME_GAP: Mutex<FrameGap> = Mutex::new(FrameGap::Idle);

pub(crate) fn frame_gap() -> std::sync::MutexGuard<'static, FrameGap> {
    FRAME_GAP.lock().unwrap_or_else(|e| e.into_inner())
}

pub(crate) fn note_phase_end() {
    let now = Instant::now();
    match &mut *frame_gap() {
        g @ FrameGap::Idle => *g = FrameGap::PhaseEnded(now),
        FrameGap::PhaseEnded(since) | FrameGap::Settling { since, .. } => *since = now,
        FrameGap::Recorded => {}
    }
}

/// Closes the settle span, keeping the phase-end instant for the gap record. The span drops after the lock, so
/// closing it never runs subscriber hooks while the mutex is held.
pub(crate) fn close_first_frame_span() {
    let mut g = frame_gap();
    let span = match std::mem::replace(&mut *g, FrameGap::Idle) {
        FrameGap::Settling { since, span } => {
            *g = FrameGap::PhaseEnded(since);
            Some(span)
        }
        other => {
            *g = other;
            None
        }
    };
    drop(g);
    drop(span);
}

/// Resets the first-frame gap for a new or abandoned attempt, dropping any open settle span off-lock, so a
/// superseded or cancelled attempt's phase end cannot leak into the next attempt or a later paint.
pub(crate) fn reset_frame_gap() {
    let stale = std::mem::replace(&mut *frame_gap(), FrameGap::Idle);
    drop(stale);
}

/// Routes a launch gap no live timer can wrap to the `subtimer_duration` histogram and the local waterfall.
/// Process init runs before the subscriber, and the first-frame wait runs after the phases close.
pub(crate) fn record_launch_gap(name: &'static str, elapsed: Duration) {
    crate::instrumentation::emit_startup_timing(name, elapsed);
    let key = name.strip_prefix("startup.").unwrap_or(name);
    crate::session_ctx::log_event(crate::events::StartupSubTimers {
        timings: vec![(key.to_owned(), duration_ms(elapsed))],
        outcome: StartupOutcome::Ok,
        auth_mode: *STARTUP_AUTH_MODE.lock().unwrap_or_else(|e| e.into_inner()),
    });
}

/// The [last completed phase, ready] segment, recorded once from whichever ready signal fires first.
pub(crate) fn record_first_frame_gap() {
    let (last_end, span) = {
        let mut g = frame_gap();
        match std::mem::replace(&mut *g, FrameGap::Recorded) {
            FrameGap::Recorded => return,
            FrameGap::Idle => (None, None),
            FrameGap::PhaseEnded(since) => (Some(since), None),
            FrameGap::Settling { since, span } => (Some(since), Some(span)),
        }
    };
    drop(span);
    if let Some(last_end) = last_end {
        record_launch_gap("startup.first_frame", last_end.elapsed());
    }
}

/// Opens the settle span once a phase has completed, so the trace carries the wait between the last phase and the confirmed frame.
pub(crate) fn open_first_frame_span() {
    if !matches!(*frame_gap(), FrameGap::PhaseEnded(_)) {
        return;
    }
    // Built off-lock; if the state changed since the check, the span drops after the guard releases.
    // Parent under the startup root when one is open, so the settle wait nests like the phase spans.
    let root = current().and_then(|timer| timer.root_span());
    let span = match &root {
        Some(root) => tracing::info_span!(parent: root, "startup.frame_settle"),
        None => tracing::info_span!("startup.frame_settle"),
    };
    let mut g = frame_gap();
    if let FrameGap::PhaseEnded(since) = *g {
        *g = FrameGap::Settling { since, span };
    }
}
