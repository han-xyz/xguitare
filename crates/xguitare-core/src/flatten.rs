use std::collections::HashMap;
use std::hash::BuildHasher;
use std::sync::Arc;
use std::time::Duration;

use crate::arrangement::TakeNotLoaded;
use crate::project::Item;

/// How long an Item's edge ramps in or out, and how long Items cross where one cuts into another.
pub const ITEM_FADE: Duration = Duration::from_millis(5);

/// One stretch of an Item's active Take as it is heard: `len` frames of `samples` from `offset`, at `position`, ramping in over its first `fade_in` frames and out over its last `fade_out`.
#[derive(Debug, Clone)]
pub struct PlacedTake {
    pub position: u64,
    pub offset: usize,
    pub len: usize,
    pub fade_in: usize,
    pub fade_out: usize,
    pub samples: Arc<[f32]>,
}

impl PlacedTake {
    /// The linear fade gain at `frame` of the `len` it plays, the lower of the fade-in and fade-out ramps; each ramp moves half a step at the span's edge, so even a one-frame span is attenuated.
    pub(crate) fn gain(&self, frame: usize) -> f32 {
        let rising = if frame < self.fade_in {
            (frame as f32 + 0.5) / self.fade_in as f32
        } else {
            1.0
        };
        let left = self.len - frame;
        let falling = if left <= self.fade_out {
            (left as f32 - 0.5) / self.fade_out as f32
        } else {
            1.0
        };
        rising.min(falling)
    }
}

/// [`ITEM_FADE`] in frames at `sample_rate`.
pub fn item_fade_frames(sample_rate: f32) -> u64 {
    (ITEM_FADE.as_secs_f64() * f64::from(sample_rate)).round() as u64
}

/// The stretches of one Track's `items` that are heard, in timeline order: an Item later in `items` is on top and hides what it covers of those before it; neighbouring stretches that continue one Take frame for frame play as one.
///
/// Each stretch's own edges fade over `fade` frames, or over half its uncovered length (rounded up) when that is shorter. Where a top Item cuts into a lower one, the lower stretch goes on under the top one's ramp, as far as the lower Item's Take reaches; under a top shorter than twice `fade` and odd in length, the two ramps meet on its middle frame, which sums above 1 by one ramp step. Where a stretch ends at an Item's end and the next one starts at its Item's start, the first goes on into its Take under the next one's fade-in, as far as its Take reaches, and the next fades in over just that.
///
/// # Errors
///
/// [`TakeNotLoaded`] when a heard Item's active Take is not in `takes`.
pub fn flatten<S: BuildHasher>(
    items: &[Item],
    takes: &HashMap<String, Arc<[f32]>, S>,
    fade: u64,
) -> Result<Vec<PlacedTake>, TakeNotLoaded> {
    let heard = joined(items, heard_stretches(items));
    heard
        .iter()
        .map(|stretch| {
            let item = &items[stretch.first];
            let file = &item.takes[item.take].file;
            let samples = takes.get(file).ok_or_else(|| TakeNotLoaded(file.clone()))?;
            Ok(place(items, stretch, &heard, fade, samples))
        })
        .collect()
}

/// Timeline frames `start..end` that nothing above covers, of the Item at index `first`, continued to `end` by the Item at index `last` when the two play one Take on.
struct Stretch {
    first: usize,
    last: usize,
    start: u64,
    end: u64,
}

impl Stretch {
    fn own_fade(&self, fade: u64) -> u64 {
        fade.min((self.end - self.start).div_ceil(2))
    }

    fn cut_at_start(&self, items: &[Item]) -> bool {
        self.start > items[self.first].position
    }

    fn cut_at_end(&self, items: &[Item]) -> bool {
        self.end < items[self.last].end()
    }

    /// Frames of the first Item's Take before this stretch's start.
    fn take_before_start(&self, items: &[Item]) -> u64 {
        let item = &items[self.first];
        item.offset.saturating_add(self.start - item.position)
    }

    /// Frames of the last Item's Take after this stretch's end.
    fn take_after_end(&self, items: &[Item]) -> u64 {
        let item = &items[self.last];
        let played_to = item.offset.saturating_add(self.end - item.position);
        item.takes[item.take].frames.saturating_sub(played_to)
    }
}

fn heard_stretches(items: &[Item]) -> Vec<Stretch> {
    let mut stretches = Vec::new();
    for (index, item) in items.iter().enumerate() {
        if item.played_frames() == 0 {
            continue;
        }
        let mut parts = vec![(item.position, item.end())];
        for above in &items[index + 1..] {
            let cover = (above.position, above.end());
            parts = parts
                .into_iter()
                .flat_map(|part| uncovered(part, cover))
                .collect();
        }
        stretches.extend(parts.into_iter().map(|(start, end)| Stretch {
            first: index,
            last: index,
            start,
            end,
        }));
    }
    stretches.sort_by_key(|stretch| stretch.start);
    stretches
}

/// `stretches` in order, each run that plays one Take on without a skip merged into one.
fn joined(items: &[Item], stretches: Vec<Stretch>) -> Vec<Stretch> {
    let mut joined: Vec<Stretch> = Vec::with_capacity(stretches.len());
    for stretch in stretches {
        match joined.last_mut() {
            Some(before) if continues(items, before, &stretch) => {
                before.end = stretch.end;
                before.last = stretch.last;
            }
            _ => joined.push(stretch),
        }
    }
    joined
}

fn continues(items: &[Item], before: &Stretch, after: &Stretch) -> bool {
    let (from, to) = (&items[before.last], &items[after.first]);
    before.end == after.start
        && from.takes[from.take].file == to.takes[to.take].file
        && from.offset.saturating_add(before.end - from.position)
            == to.offset.saturating_add(after.start - to.position)
}

/// What is left of `start..end` outside `cover`: none, one or two non-empty ranges.
fn uncovered((start, end): (u64, u64), cover: (u64, u64)) -> Vec<(u64, u64)> {
    let (cover_start, cover_end) = cover;
    if cover_start >= cover_end {
        return vec![(start, end)];
    }
    [(start, end.min(cover_start)), (start.max(cover_end), end)]
        .into_iter()
        .filter(|(from, to)| from < to)
        .collect()
}

/// Frames the stretch before `after` plays on past its end under `after`'s fade-in: none unless it ends at its Item's end and `after` starts at its Item's start.
fn join_overlap(items: &[Item], before: &Stretch, after: &Stretch, fade: u64) -> u64 {
    if before.cut_at_end(items) || after.cut_at_start(items) {
        return 0;
    }
    after.own_fade(fade).min(before.take_after_end(items))
}

fn place(
    items: &[Item],
    stretch: &Stretch,
    heard: &[Stretch],
    fade: u64,
    samples: &Arc<[f32]>,
) -> PlacedTake {
    let before = heard.iter().find(|other| other.end == stretch.start);
    let after = heard.iter().find(|other| other.start == stretch.end);
    let (extend_in, fade_in) = if stretch.cut_at_start(items) {
        let ramp = before
            .map_or(0, |top| top.own_fade(fade))
            .min(stretch.take_before_start(items));
        (ramp, ramp)
    } else {
        let joined = before.map_or(0, |before| join_overlap(items, before, stretch, fade));
        (
            0,
            if joined > 0 {
                joined
            } else {
                stretch.own_fade(fade)
            },
        )
    };
    let (extend_out, fade_out) = if stretch.cut_at_end(items) {
        let ramp = after
            .map_or(0, |top| top.own_fade(fade))
            .min(stretch.take_after_end(items));
        (ramp, ramp)
    } else {
        match after.map_or(0, |after| join_overlap(items, stretch, after, fade)) {
            0 => (0, stretch.own_fade(fade)),
            joined => (joined, joined),
        }
    };
    let position = stretch.start - extend_in;
    let offset = clamp_frames(stretch.take_before_start(items) - extend_in).min(samples.len());
    let len = clamp_frames(stretch.end + extend_out - position).min(samples.len() - offset);
    PlacedTake {
        position,
        offset,
        len,
        fade_in: clamp_frames(fade_in).min(len),
        fade_out: clamp_frames(fade_out).min(len),
        samples: Arc::clone(samples),
    }
}

fn clamp_frames(frames: u64) -> usize {
    usize::try_from(frames).unwrap_or(usize::MAX)
}
