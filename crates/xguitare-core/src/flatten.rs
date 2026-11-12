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

/// [`ITEM_FADE`] in frames at `sample_rate`.
pub fn item_fade_frames(sample_rate: f32) -> u64 {
    (ITEM_FADE.as_secs_f64() * f64::from(sample_rate)).round() as u64
}

/// The stretches of one Track's `items` that are heard: an Item later in `items` is on top and hides what it covers of those before it. Each stretch fades in and out over `fade` frames, or half its length when shorter; where a top Item cuts into a lower one, the lower stretch goes on under the top one's ramp (as far as the lower Item reaches), so the two cross.
///
/// # Errors
///
/// [`TakeNotLoaded`] when a heard Item's active Take is not in `takes`.
pub fn flatten<S: BuildHasher>(
    items: &[Item],
    takes: &HashMap<String, Arc<[f32]>, S>,
    fade: u64,
) -> Result<Vec<PlacedTake>, TakeNotLoaded> {
    let heard = heard_stretches(items);
    heard
        .iter()
        .map(|stretch| {
            let item = &items[stretch.item];
            let file = &item.takes[item.take].file;
            let samples = takes.get(file).ok_or_else(|| TakeNotLoaded(file.clone()))?;
            Ok(place(item, stretch, &heard, fade, samples))
        })
        .collect()
}

/// Timeline frames `start..end` of the Item at index `item` that nothing above covers.
struct Stretch {
    item: usize,
    start: u64,
    end: u64,
}

impl Stretch {
    fn own_fade(&self, fade: u64) -> u64 {
        fade.min((self.end - self.start) / 2)
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
            item: index,
            start,
            end,
        }));
    }
    stretches
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

fn place(
    item: &Item,
    stretch: &Stretch,
    heard: &[Stretch],
    fade: u64,
    samples: &Arc<[f32]>,
) -> PlacedTake {
    let crossing = |neighbour: Option<&Stretch>| neighbour.map_or(0, |other| other.own_fade(fade));
    let (extend_in, fade_in) = if stretch.start > item.position {
        let ramp = crossing(heard.iter().find(|other| other.end == stretch.start))
            .min(stretch.start - item.position);
        (ramp, ramp)
    } else {
        (0, stretch.own_fade(fade))
    };
    let (extend_out, fade_out) = if stretch.end < item.end() {
        let ramp = crossing(heard.iter().find(|other| other.start == stretch.end))
            .min(item.end() - stretch.end);
        (ramp, ramp)
    } else {
        (0, stretch.own_fade(fade))
    };
    let position = stretch.start - extend_in;
    let offset =
        clamp_frames(item.offset.saturating_add(position - item.position)).min(samples.len());
    let len = clamp_frames(stretch.end + extend_out - position).min(samples.len() - offset);
    let fade_in = clamp_frames(fade_in).min(len);
    PlacedTake {
        position,
        offset,
        len,
        fade_in,
        fade_out: clamp_frames(fade_out).min(len - fade_in),
        samples: Arc::clone(samples),
    }
}

fn clamp_frames(frames: u64) -> usize {
    usize::try_from(frames).unwrap_or(usize::MAX)
}
