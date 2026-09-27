//! Re-binding of `[Image #N]` placeholder text to the image records the composer holds.
//!
//! A chip's text can come back as plain text (yank, undo, plain paste, restore) while its
//! `PastedImage` record still lives in `images` or the undo stash. The functions here re-register
//! the chip element over that text and report the placeholders no record backs. They never edit
//! text, never exceed [`PromptWidget::IMAGE_CAP`], never overlap an element of any kind, and never
//! reorder `images`: a recovered stash record is appended, so a drain keeps insertion order.

use std::collections::HashSet;

use a3s_ratatui_textarea::ElementId;

use crate::views::prompt_widget::{
    KIND_IMAGE, PROMPT_IMAGES_TRACING_TARGET, PromptWidget, chip_line, images_high_water,
    parse_image_display_number,
};

/// Where a placeholder's record was found: an unbound entry of `images`, or the undo stash.
enum Record {
    Held(usize),
    Stashed(usize),
}

impl PromptWidget {
    /// Re-register `KIND_IMAGE` elements over orphan `[Image #N]` text and bind each to the record
    /// that names it, from `images` or the undo stash. Text naming no record, or a number a live chip
    /// already carries, stays plain. Returns the re-chipped display numbers.
    pub(super) fn rechip_orphan_image_placeholders(&mut self) -> Vec<usize> {
        let mut rechipped = Vec::new();
        if !self.textarea.text().contains("[Image #") {
            return rechipped;
        }
        let mut live = self.live_image_element_ids();
        // Live chips count against the cap whether or not a record backs them yet (undo can restore
        // ten recordless chips whose records are all in the stash).
        let mut budget = Self::IMAGE_CAP.saturating_sub(live.len());
        // A number a live chip already carries must not pull a second (stale) record for a later
        // duplicate placeholder. Chip text counts as well as bound records: after kill-line and undo
        // the chips are back before their records re-bind from the stash.
        let mut claimed: HashSet<usize> = self
            .textarea
            .elements()
            .iter()
            .filter(|e| e.kind == KIND_IMAGE)
            .filter_map(|e| {
                self.textarea
                    .get_range(e.range.clone())
                    .and_then(parse_image_display_number)
            })
            .collect();
        claimed.extend(self.attached_display_numbers(&live));
        let regex = canonical_image_placeholder_regex();
        let placeholders: Vec<(std::ops::Range<usize>, usize)> = regex
            .captures_iter(self.textarea.text())
            .filter_map(|caps| {
                let number = caps.get(1)?.as_str().parse().ok()?;
                Some((caps.get(0)?.range(), number))
            })
            .collect();
        for (range, number) in placeholders {
            if self
                .textarea
                .elements()
                .iter()
                .any(|e| e.range.start < range.end && range.start < e.range.end)
            {
                continue;
            }
            if claimed.contains(&number) {
                continue;
            }
            let in_images = self
                .images
                .iter()
                .position(|img| img.display_number == number && !live.contains(&img.element_id));
            let in_stash = self
                .image_undo_stash
                .iter()
                .position(|img| img.display_number == number);
            let record = match (in_images, in_stash) {
                (Some(position), _) => Record::Held(position),
                (None, Some(position)) => Record::Stashed(position),
                (None, None) => continue,
            };
            if budget == 0 {
                tracing::warn!(
                    target: PROMPT_IMAGES_TRACING_TARGET,
                    display_number = number,
                    cap = Self::IMAGE_CAP,
                    "rechip_orphan_image_placeholders: cap reached; placeholder stays plain text",
                );
                break;
            }
            let position = match record {
                Record::Held(position) => position,
                Record::Stashed(stash_position) => {
                    let img = self.image_undo_stash.remove(stash_position);
                    self.images.push(img);
                    self.images.len() - 1
                }
            };
            // The record is resolved first so a refused registration never leaves a chip without one.
            let display = chip_line(format!("Image #{number}"));
            let Some(id) = self
                .textarea
                .restore_element(range, KIND_IMAGE, Some(display))
            else {
                continue;
            };
            if let Some(img) = self.images.get_mut(position) {
                img.element_id = id;
                img.display_number = number;
            }
            live.insert(id);
            claimed.insert(number);
            budget -= 1;
            rechipped.push(number);
        }
        self.image_counter = self.image_counter.max(images_high_water(&self.images));
        if !rechipped.is_empty() {
            tracing::info!(
                target: PROMPT_IMAGES_TRACING_TARGET,
                ?rechipped,
                images_len = self.images.len(),
                stash_len = self.image_undo_stash.len(),
                "prompt_widget: rechip_orphan_image_placeholders",
            );
        }
        rechipped
    }

    /// Re-chip orphan placeholders so `images` reflects the buffer before a send-route decision or
    /// a drain reads it. Binds only; it does not resync, so `images` keeps its insertion order.
    pub(crate) fn rebind_image_placeholders(&mut self) {
        self.rechip_orphan_image_placeholders();
    }

    /// Display numbers of `[Image #N]` placeholders no image record backs: plain text, or a
    /// `KIND_IMAGE` chip whose record is gone (undo after a clear). Text inside an element of
    /// another kind (a paste block, a file ref) is that element's content and is not reported, and
    /// neither is a duplicate of a number an attached chip carries: that image is on the wire once.
    /// Each number is reported once.
    pub(crate) fn unbound_image_placeholders(&self) -> Vec<usize> {
        let text = self.textarea.text();
        if !text.contains("[Image #") {
            return Vec::new();
        }
        let elements = self.textarea.elements();
        let attached = self.attached_display_numbers(&self.live_image_element_ids());
        let mut seen = HashSet::new();
        canonical_image_placeholder_regex()
            .captures_iter(text)
            .filter_map(|caps| {
                let range = caps.get(0)?.range();
                let recordless = elements
                    .iter()
                    .filter(|e| e.range.start < range.end && range.start < e.range.end)
                    .all(|e| {
                        e.kind == KIND_IMAGE
                            && !self.images.iter().any(|img| img.element_id == e.id)
                    });
                if !recordless {
                    return None;
                }
                let number: usize = caps.get(1)?.as_str().parse().ok()?;
                (!attached.contains(&number) && seen.insert(number)).then_some(number)
            })
            .collect()
    }

    /// Ids of the `KIND_IMAGE` elements in the buffer.
    fn live_image_element_ids(&self) -> HashSet<ElementId> {
        self.textarea
            .elements()
            .iter()
            .filter(|e| e.kind == KIND_IMAGE)
            .map(|e| e.id)
            .collect()
    }

    /// Display numbers of the records a live chip binds: the images a send carries.
    fn attached_display_numbers(&self, live: &HashSet<ElementId>) -> HashSet<usize> {
        self.images
            .iter()
            .filter(|img| live.contains(&img.element_id))
            .map(|img| img.display_number)
            .collect()
    }
}

/// Regex matching the canonical path-free chip text `[Image #<digits>]` with the number captured.
/// The path form `[Image #N: path]` is user text, never a chip.
fn canonical_image_placeholder_regex() -> &'static regex::Regex {
    use std::sync::LazyLock;
    #[expect(clippy::unwrap_used, reason = "the pattern is a literal that compiles")]
    static RE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"\[Image #(\d+)\]").unwrap());
    &RE
}
