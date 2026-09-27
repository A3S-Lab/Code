use super::*;

mod span_capture {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use tracing::span::{Attributes, Id};
    use tracing_subscriber::layer::{Context, Layer};

    #[derive(Default)]
    pub(super) struct SpanLog {
        open: HashMap<u64, String>,
        pub(super) closed: Vec<String>,
    }

    pub(super) struct SpanTimingLayer(pub(super) Arc<Mutex<SpanLog>>);

    impl<S: tracing::Subscriber> Layer<S> for SpanTimingLayer {
        fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, _ctx: Context<'_, S>) {
            self.0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .open
                .insert(id.into_u64(), attrs.metadata().name().to_owned());
        }

        fn on_close(&self, id: Id, _ctx: Context<'_, S>) {
            let mut log = self.0.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(name) = log.open.remove(&id.into_u64()) {
                log.closed.push(name);
            }
        }
    }
}

#[tracing::instrument(name = "session.spawn", skip_all)]
async fn fake_spawn_session_actor() {
    tracing::info_span!("spawn.actor_setup")
        .in_scope(|| std::thread::sleep(Duration::from_millis(2)));
}

fn drive_fake_spawn_under(parent: &tracing::Span) {
    use tracing::Instrument as _;
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("current-thread runtime");
    rt.block_on(fake_spawn_session_actor().instrument(parent.clone()));
}

#[test]
fn summary_is_byte_stable_for_fixed_inputs() {
    let snap = PhaseSnapshot {
        completed: vec![
            (StartupPhase::ConfigLoad, Duration::from_millis(12)),
            (StartupPhase::Bootstrap, Duration::from_millis(1500)),
        ],
        open: Some((StartupPhase::SessionCreate, Duration::from_millis(3))),
    };
    assert_eq!(
        snap.summary(),
        "config_load=12ms, bootstrap=1.5s, session_create>=3ms"
    );
}

#[test]
fn interactive_frame_bounds_the_root_span_and_records_once() {
    use tracing_subscriber::layer::SubscriberExt as _;

    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    reset_for_tests();
    crate::unified_log::redirect_to_temp_for_tests();

    let log = Arc::new(Mutex::new(span_capture::SpanLog::default()));
    let subscriber =
        tracing_subscriber::registry().with(span_capture::SpanTimingLayer(Arc::clone(&log)));
    let _guard = tracing::subscriber::set_default(subscriber);

    let _p = begin(Owner::Client);
    enter(StartupPhase::ConfigLoad);
    assert!(record_interactive_frame(), "first frame records");
    assert!(!record_interactive_frame(), "second frame is a no-op");

    let closed = log.lock().unwrap_or_else(|e| e.into_inner());
    assert!(
        closed.closed.iter().any(|name| name == "startup"),
        "the first interactive frame closes the root span without a total"
    );
    drop(closed);
    clear();
}

#[test]
fn record_subphase_routes_each_arm_and_init_process_first_write_wins() {
    use strum::IntoEnumIterator as _;
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());

    for sp in Subphase::iter() {
        reset_for_tests();
        record_subphase(sp, Duration::from_millis(7));
        let sub = *subphases();
        assert_eq!(sub.get(sp), Some(7), "{sp:?} routes to its own slot");
        let set = Subphase::iter()
            .filter(|&other| sub.get(other) == Some(7))
            .count();
        assert_eq!(set, 1, "{sp:?} sets exactly one slot");
    }

    reset_for_tests();
    record_subphase(Subphase::InitProcess, Duration::from_millis(7));
    record_subphase(Subphase::InitProcess, Duration::from_millis(999));
    assert_eq!(subphases().get(Subphase::InitProcess), Some(7));

    reset_for_tests();
    record_subphase(Subphase::SessionLoad, Duration::from_millis(7));
    record_subphase(Subphase::SessionLoad, Duration::from_millis(999));
    assert_eq!(subphases().get(Subphase::SessionLoad), Some(999));
}

#[test]
fn startup_children_fold_under_their_phase_and_subphase() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    reset_for_tests();
    crate::unified_log::redirect_to_temp_for_tests();

    let folded = crate::span_profile::test_support::folded_with_layer(|| {
        let _p = begin(Owner::Client);

        enter(StartupPhase::SessionCreate);
        {
            let parent = current_phase_span().expect("session_create phase span is open");
            let _rpc = crate::region!(
                "startup.session_create.backend_rpc",
                crate::region::Parent::Explicit(&parent)
            );
            std::thread::sleep(Duration::from_millis(2));
        }

        {
            let mut timer = crate::instrumentation::timer("session.load_session_replay");
            timer.with_subphase(Subphase::SessionReplay);
            let parent = timer
                .subphase_span()
                .expect("session_replay subphase span is open");
            let read = tracing::info_span!(parent: &parent, "startup.session_replay.read_file");
            read.in_scope(|| std::thread::sleep(Duration::from_millis(2)));
        }

        {
            let mut timer = crate::instrumentation::timer("session.spawn_timer");
            timer.with_subphase(Subphase::SessionSpawn);
            let ctx = SpawnTraceContext::new(timer.subphase_span(), tracing::Span::current());
            drive_fake_spawn_under(&ctx.parent);
        }

        report_total(StartupOutcome::Ok);
    });

    let paths: Vec<&str> = folded
        .lines()
        .filter_map(|l| l.rsplit_once(' ').map(|(p, _)| p))
        .collect();
    assert!(
        paths
            .iter()
            .any(|p| p.ends_with("startup.session_create;startup.session_create.backend_rpc")),
        "backend_rpc must fold under startup.session_create:\n{folded}"
    );
    assert!(
        paths
            .iter()
            .any(|p| p.ends_with("startup.session_replay;startup.session_replay.read_file")),
        "replay steps must fold under startup.session_replay:\n{folded}"
    );
    assert!(
        paths
            .iter()
            .any(|p| p.ends_with("startup.session_spawn;session.spawn;spawn.actor_setup")),
        "spawn work must fold under startup.session_spawn:\n{folded}"
    );
}
