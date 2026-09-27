use super::*;

const SLOW_PHASE_WARN_AFTER: Duration = Duration::from_secs(10);

/// Phases already warned about, per timer.
#[derive(Default)]
pub(crate) struct WarnedPhases {
    timer: usize,
    phases: Vec<StartupPhase>,
}

/// A phase left open past the threshold; agent-owned timers idle with a phase open until their first client, so they are skipped.
pub(crate) fn slow_phase_to_warn(
    timer: &Arc<StartupTimer>,
    threshold: Duration,
    warned: &mut WarnedPhases,
) -> Option<(StartupPhase, Duration)> {
    if timer.owner() == Owner::Agent {
        return None;
    }
    let timer_id = Arc::as_ptr(timer) as usize;
    if warned.timer != timer_id {
        *warned = WarnedPhases {
            timer: timer_id,
            phases: Vec::new(),
        };
    }
    let (phase, age) = timer.open_phase_age()?;
    if age < threshold || warned.phases.contains(&phase) {
        return None;
    }
    warned.phases.push(phase);
    Some((phase, age))
}

/// Warns once per phase that runs long.
/// A plain thread, because startup spans runtime construction; exits when startup ends.
pub(crate) fn spawn_slow_phase_warnings() {
    static SPAWNED: std::sync::Once = std::sync::Once::new();
    SPAWNED.call_once(|| {
        std::thread::Builder::new()
            .name("startup-slow-phase".into())
            .spawn(|| {
                let mut warned = WarnedPhases::default();
                while !DONE.load(Ordering::Relaxed) && !INTERACTIVE.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(500));
                    let Some(timer) = current() else { continue };
                    if let Some((phase, age)) =
                        slow_phase_to_warn(&timer, SLOW_PHASE_WARN_AFTER, &mut warned)
                    {
                        let open_ms = duration_ms(age);
                        tracing::warn!(
                            phase = phase.label(),
                            open_ms,
                            "startup phase running long"
                        );
                        let ctx = serde_json::json!({
                            "phase": phase.label(),
                            "open_ms": open_ms,
                        });
                        crate::unified_log::warn(STARTUP_SLOW_PHASE_MSG, None, Some(ctx));
                    }
                }
            })
            .ok();
    });
}

pub(crate) fn warn_over_budget(snapshot: &PhaseSnapshot) {
    let over: Vec<(&str, u64)> = snapshot
        .over_budget()
        .iter()
        .map(|(phase, elapsed)| (phase.label(), duration_ms(*elapsed)))
        .collect();
    if over.is_empty() {
        return;
    }
    tracing::warn!(?over, message = STARTUP_OVER_BUDGET_MSG);
    crate::unified_log::warn(
        STARTUP_OVER_BUDGET_MSG,
        None,
        Some(serde_json::json!({ "over_budget": over })),
    );
}
