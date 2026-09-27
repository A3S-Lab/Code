use std::time::{Duration, Instant};

use super::{
    PrintedLayout, REPRINT_DEBOUNCE, ReprintDecision, ReprintState, mark_minimal_history_printed,
};
use crate::app::app_view::tests::test_app;

fn wide(width: u16) -> PrintedLayout {
    PrintedLayout {
        width,
        compact: false,
    }
}

#[test]
fn first_layout_is_taken_as_printed() {
    let mut state = ReprintState::default();
    assert_eq!(
        ReprintDecision::Keep,
        state.observe(wide(100), Instant::now())
    );
    assert!(!state.is_waiting());
}

#[test]
fn width_change_reprints_only_after_the_debounce() {
    let start = Instant::now();
    let mut state = ReprintState::default();
    state.observe(wide(100), start);

    assert_eq!(ReprintDecision::Wait, state.observe(wide(60), start));
    assert!(state.is_waiting());
    assert_eq!(
        ReprintDecision::Wait,
        state.observe(
            wide(60),
            start + REPRINT_DEBOUNCE - Duration::from_millis(1)
        )
    );
    assert_eq!(
        ReprintDecision::Reprint,
        state.observe(wide(60), start + REPRINT_DEBOUNCE)
    );

    state.mark_printed(wide(60));
    assert_eq!(
        ReprintDecision::Keep,
        state.observe(wide(60), start + REPRINT_DEBOUNCE * 2)
    );
    assert!(!state.is_waiting());
}

#[test]
fn compact_flip_at_the_same_width_reprints() {
    let start = Instant::now();
    let mut state = ReprintState::default();
    state.observe(wide(64), start);
    let compact = PrintedLayout {
        width: 64,
        compact: true,
    };

    assert_eq!(ReprintDecision::Wait, state.observe(compact, start));
    assert_eq!(
        ReprintDecision::Reprint,
        state.observe(compact, start + REPRINT_DEBOUNCE)
    );
}

#[test]
fn further_resize_restarts_the_debounce() {
    let start = Instant::now();
    let mut state = ReprintState::default();
    state.observe(wide(100), start);
    state.observe(wide(80), start);

    let later = start + REPRINT_DEBOUNCE - Duration::from_millis(10);
    assert_eq!(ReprintDecision::Wait, state.observe(wide(60), later));
    assert_eq!(
        ReprintDecision::Wait,
        state.observe(wide(60), start + REPRINT_DEBOUNCE)
    );
    assert_eq!(
        ReprintDecision::Reprint,
        state.observe(wide(60), later + REPRINT_DEBOUNCE)
    );
}

#[test]
fn returning_to_the_printed_layout_cancels_the_reprint() {
    let start = Instant::now();
    let mut state = ReprintState::default();
    state.observe(wide(100), start);
    state.observe(wide(60), start);

    assert_eq!(
        ReprintDecision::Keep,
        state.observe(wide(100), start + REPRINT_DEBOUNCE)
    );
    assert!(!state.is_waiting());
}

#[test]
fn unmarked_reprint_repeats_so_a_deferred_frame_retries() {
    let start = Instant::now();
    let mut state = ReprintState::default();
    state.observe(wide(100), start);
    state.observe(wide(60), start);

    let due = start + REPRINT_DEBOUNCE;
    assert_eq!(ReprintDecision::Reprint, state.observe(wide(60), due));
    assert_eq!(
        ReprintDecision::Reprint,
        state.observe(wide(60), due + Duration::from_millis(50))
    );
}

#[test]
fn rows_printed_at_another_width_force_a_reprint_after_returning() {
    let start = Instant::now();
    let mut state = ReprintState::default();
    state.observe(wide(100), start);
    state.observe(wide(60), start);
    state.record_rows(wide(60));

    assert_eq!(ReprintDecision::Wait, state.observe(wide(100), start));
    assert_eq!(
        ReprintDecision::Reprint,
        state.observe(wide(100), start + REPRINT_DEBOUNCE)
    );
}

#[test]
fn rows_printed_in_the_printed_layout_keep_the_history() {
    let start = Instant::now();
    let mut state = ReprintState::default();
    state.observe(wide(100), start);

    state.record_rows(wide(100));

    assert_eq!(
        ReprintDecision::Keep,
        state.observe(wide(100), start + REPRINT_DEBOUNCE)
    );
}

#[test]
fn rows_printed_before_the_first_frame_set_the_printed_layout() {
    let mut state = ReprintState::default();
    state.record_rows(wide(5));

    assert_eq!(
        ReprintDecision::Wait,
        state.observe(wide(100), Instant::now())
    );
}

#[test]
fn reprint_consumes_a_pending_welcome_card() {
    let mut app = test_app();
    app.minimal_state.welcome_pending = true;

    mark_minimal_history_printed(&mut app, 80);

    assert!(!app.minimal_state.welcome_pending);
}
