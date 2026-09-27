// Per-test-case module for the `pty_e2e` integration test crate.
#[allow(unused_imports)]
use crate::common::*;

const ROWS: u16 = 30;
const COLS: u16 = 100;

/// One token per committed row. Each bullet fits on one row at [`COLS`] and spills onto two at 45 columns.
const ROW_TOKENS: &[&str] = &[
    "ROWTOKA", "ROWTOKB", "ROWTOKC", "ROWTOKD", "ROWTOKE", "ROWTOKF",
];

/// Wraps onto two prompt rows at [`COLS`], putting a prompt row above the cursor.
const DRAFT: &str = "DRAFTHEAD the draft keeps going for long enough that the prompt box wraps it onto a second row before DRAFTTAIL";

/// The harness screen is `alacritty_terminal`, which re-wraps rows on a narrowing. `ALACRITTY_SOCKET` tells the pager it runs in Alacritty.
fn spawn_rewrapping(content: &ContentController) -> PtyHarness {
    let ops = [EnvOp::set(
        "ALACRITTY_SOCKET",
        "/tmp/grok-pty-harness-alacritty.sock",
    )];
    spawn_minimal_env_ops(content, ROWS, COLS, &[], &ops, None)
}

const BULLET_BODY: &str = "the committed row carries enough prose to spill past forty-five columns";

fn bullet_response() -> String {
    ROW_TOKENS
        .iter()
        .map(|t| format!("- {t} {BULLET_BODY}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Tokens that do not appear exactly once in native select-copy, which joins terminal re-wraps back into one line.
/// A missing token is a cleared committed row. A doubled token is a stale live row.
fn miscounted(harness: &PtyHarness, tokens: &[&str]) -> Vec<(String, usize)> {
    let copy = harness.native_copy_text();
    tokens
        .iter()
        .map(|t| ((*t).to_owned(), copy.matches(t).count()))
        .filter(|(_, n)| *n != 1)
        .collect()
}

/// Whether the history was reprinted at `cols`. One welcome card remains, no terminal re-wrap splits `prose`, and a wide width puts each bullet on one row.
fn is_reprinted_at(harness: &PtyHarness, cols: u16) -> bool {
    let full = harness.full_text();
    let bullets_intact = full.matches("prose").count() == ROW_TOKENS.len();
    let bullets_single_row = cols < 90
        || ROW_TOKENS.iter().all(|t| {
            full.lines()
                .any(|l| l.contains(&format!("{t} {BULLET_BODY}")))
        });
    full.matches("A3S Code").count() == 1 && bullets_intact && bullets_single_row
}

/// Cursor-position queries (`CSI 6 n`) sent between a synchronized-update begin and its end.
/// A terminal holds the reply until the update ends.
fn cursor_queries_inside_sync(raw: &[u8]) -> usize {
    const BEGIN: &[u8] = b"\x1b[?2026h";
    const END: &[u8] = b"\x1b[?2026l";
    const QUERY: &[u8] = b"\x1b[6n";
    let mut in_sync = false;
    let mut inside = 0;
    for i in 0..raw.len() {
        let rest = &raw[i..];
        if rest.starts_with(BEGIN) {
            in_sync = true;
        } else if rest.starts_with(END) {
            in_sync = false;
        } else if in_sync && rest.starts_with(QUERY) {
            inside += 1;
        }
    }
    inside
}

fn assert_single_after_resize(harness: &mut PtyHarness, rows: u16, cols: u16, tokens: &[&str]) {
    harness.resize(rows, cols).expect("resize");
    let settled = harness.wait_until_stable(
        "history reprinted with every committed and live token exactly once",
        Duration::from_secs(10),
        Duration::from_millis(600),
        |h| miscounted(h, tokens).is_empty() && is_reprinted_at(h, cols),
    );
    assert!(
        settled.is_ok(),
        "resize to {rows}x{cols} miscounted {:?}, reprinted {}\nfull:\n{}",
        miscounted(harness, tokens),
        is_reprinted_at(harness, cols),
        harness.full_text()
    );
    assert!(
        harness.is_running().expect("poll pager liveness"),
        "pager exited during resize to {rows}x{cols}\nscreen:\n{}",
        harness.screen_contents()
    );
}

/// A resize re-anchors the live region on where the terminal reflowed the cursor, then reprints the history at the new width.
/// Neither step may drop a committed row or leave a stale copy of the status row or prompt behind.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn minimal_resize_reanchors_live_region() {
    let content = ContentController::start().await.expect("start content");
    content.set_response(bullet_response());

    let mut harness = spawn_rewrapping(&content);
    wait_minimal_ready(&mut harness);
    harness
        .inject_keys(format!("{PROMPT}\r").as_bytes())
        .expect("submit prompt");
    harness
        .wait_for_turn_idle(Duration::from_secs(30))
        .expect("turn committed");
    harness.inject_keys(DRAFT.as_bytes()).expect("type draft");
    harness
        .wait_for_text("DRAFTTAIL", Duration::from_secs(10))
        .expect("draft drawn");

    let mut tokens = ROW_TOKENS.to_vec();
    tokens.extend([
        MINIMAL_IDLE_SENTINEL,
        "DRAFTHEAD",
        "DRAFTTAIL",
        "Worked for",
    ]);
    assert_single_after_resize(&mut harness, ROWS, 45, &tokens);

    let full = harness.full_text();
    let stray_borders: Vec<&str> = full
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && l.chars().all(|c| matches!(c, '│' | '─' | '╮' | '╯')))
        .collect();
    assert!(
        stray_borders.is_empty(),
        "welcome card border re-wrapped at 45 columns: {stray_borders:?}\nfull:\n{full}"
    );
    for (rows, cols) in [(ROWS, COLS), (ROWS - 8, COLS), (ROWS, 130), (ROWS, 60)] {
        assert_single_after_resize(&mut harness, rows, cols, &tokens);
    }

    // Clear the draft, then a second turn streams and commits below the resized history
    harness.inject_keys(b"\x15").expect("clear draft");
    content.set_response(format!("{} after resize.", turn_sentinel(2)));
    harness.inject_keys(b"again\r").expect("submit second turn");
    harness
        .wait_for_full_text(&turn_sentinel(2), Duration::from_secs(30))
        .expect("second turn streams after the resizes");
    harness
        .wait_for_turn_idle(Duration::from_secs(30))
        .expect("second turn committed");
    let mut after = ROW_TOKENS.to_vec();
    after.push(MINIMAL_IDLE_SENTINEL);
    assert!(
        miscounted(&harness, &after).is_empty(),
        "second turn miscounted {:?}\nfull:\n{}",
        miscounted(&harness, &after),
        harness.full_text()
    );

    quit_minimal(&mut harness);
}

/// A narrowing drag mid-stream re-wraps the live tail rows above the prompt. The redraw must start above all of them.
/// No resize may query the cursor inside an open synchronized update.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn minimal_resize_mid_stream_reanchors_live_region() {
    let content = ContentController::start().await.expect("start content");
    content.set_response(bullet_response());
    content.set_chunk_delay(Some(Duration::from_millis(40)));

    let mut harness = spawn_rewrapping(&content);
    wait_minimal_ready(&mut harness);
    harness
        .inject_keys(format!("{PROMPT}\r").as_bytes())
        .expect("submit prompt");
    harness
        .wait_for_full_text("ROWTOKC", Duration::from_secs(30))
        .expect("tail streaming");
    // Resize like a window drag, while frames are still being drawn
    for cols in (45..COLS).rev().step_by(5) {
        harness.resize(ROWS, cols).expect("narrow mid-stream");
        harness.update(Duration::from_millis(40));
    }
    harness
        .wait_for_turn_idle(Duration::from_secs(60))
        .expect("turn committed after the resize");
    assert_eq!(
        0,
        cursor_queries_inside_sync(harness.raw_output()),
        "a cursor query went out inside an open synchronized update"
    );

    let mut tokens = ROW_TOKENS.to_vec();
    tokens.push(MINIMAL_IDLE_SENTINEL);
    let settled = harness.wait_until_stable(
        "every committed and live token exactly once",
        Duration::from_secs(10),
        Duration::from_millis(600),
        |h| miscounted(h, &tokens).is_empty(),
    );
    assert!(
        settled.is_ok(),
        "mid-stream narrowing miscounted {:?}\nfull:\n{}",
        miscounted(&harness, &tokens),
        harness.full_text()
    );

    quit_minimal(&mut harness);
}
