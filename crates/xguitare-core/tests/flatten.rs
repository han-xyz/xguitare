use std::collections::HashMap;
use std::sync::Arc;

use xguitare_core::arrangement::TakeNotLoaded;
use xguitare_core::flatten::flatten;
use xguitare_core::project::{Item, Take};

const FADE: u64 = 10;
const TAKE_FRAMES: u64 = 1000;

fn item(id: u64, start: u64, end: u64) -> Item {
    Item {
        id,
        position: start,
        offset: 0,
        length: Some(end - start),
        take: 0,
        takes: vec![Take {
            file: format!("takes/t-{id}.wav"),
            frames: TAKE_FRAMES,
        }],
    }
}

fn takes(items: &[Item]) -> HashMap<String, Arc<[f32]>> {
    items
        .iter()
        .map(|item| {
            (
                item.takes[0].file.clone(),
                vec![0.0; TAKE_FRAMES as usize].into(),
            )
        })
        .collect()
}

/// `(position, offset, len, fade_in, fade_out)` of each span.
fn spans(items: &[Item]) -> Vec<(u64, usize, usize, usize, usize)> {
    flatten(items, &takes(items), FADE)
        .unwrap()
        .iter()
        .map(|span| {
            (
                span.position,
                span.offset,
                span.len,
                span.fade_in,
                span.fade_out,
            )
        })
        .collect()
}

#[test]
fn an_item_alone_fades_over_the_fade_or_half_its_length() {
    assert_eq!(spans(&[item(1, 0, 100)]), [(0, 0, 100, 10, 10)]);
    assert_eq!(spans(&[item(1, 0, 8)]), [(0, 0, 8, 4, 4)]);
}

#[test]
fn a_fully_covered_item_is_not_heard() {
    assert_eq!(
        spans(&[item(1, 0, 100), item(2, 0, 100)]),
        [(0, 0, 100, 10, 10)]
    );
    assert_eq!(
        spans(&[item(1, 20, 50), item(2, 0, 100)]),
        [(0, 0, 100, 10, 10)]
    );
}

#[test]
fn a_top_item_over_the_right_part_crossfades_under_its_fade_in() {
    assert_eq!(
        spans(&[item(1, 0, 100), item(2, 50, 150)]),
        [(0, 0, 60, 10, 10), (50, 0, 100, 10, 10)]
    );
}

#[test]
fn a_top_item_over_the_left_part_crossfades_under_its_fade_out() {
    let mut lower = item(1, 50, 150);
    lower.offset = 5;
    assert_eq!(
        spans(&[lower, item(2, 0, 100)]),
        [(90, 45, 60, 10, 10), (0, 0, 100, 10, 10)]
    );
}

#[test]
fn a_top_item_inside_splits_the_lower_in_two() {
    assert_eq!(
        spans(&[item(1, 0, 300), item(2, 100, 200)]),
        [
            (0, 0, 110, 10, 10),
            (190, 190, 110, 10, 10),
            (100, 0, 100, 10, 10)
        ]
    );
}

#[test]
fn three_deep_each_crossfades_with_the_one_heard_next_to_it() {
    assert_eq!(
        spans(&[item(1, 0, 300), item(2, 100, 250), item(3, 150, 200)]),
        [
            (0, 0, 110, 10, 10),
            (240, 240, 60, 10, 10),
            (100, 0, 60, 10, 10),
            (190, 90, 60, 10, 10),
            (150, 0, 50, 10, 10),
        ]
    );
}

#[test]
fn the_extension_under_the_top_stays_inside_the_lower_item() {
    assert_eq!(
        spans(&[item(1, 95, 200), item(2, 0, 100)]),
        [(95, 0, 105, 5, 10), (0, 0, 100, 10, 10)]
    );
}

#[test]
fn a_leftover_of_one_frame_stays_and_an_item_that_plays_nothing_is_dropped() {
    assert_eq!(
        spans(&[item(1, 0, 100), item(2, 0, 99)]),
        [(89, 89, 11, 10, 0), (0, 0, 99, 10, 10)]
    );
    let mut silent = item(1, 0, 100);
    (silent.offset, silent.length) = (TAKE_FRAMES, None);
    assert_eq!(spans(&[silent]), []);
}

#[test]
fn an_unloaded_take_is_an_error() {
    let items = [item(1, 0, 100)];
    let none: HashMap<String, Arc<[f32]>> = HashMap::new();
    assert!(matches!(
        flatten(&items, &none, FADE),
        Err(TakeNotLoaded(file)) if file == "takes/t-1.wav"
    ));
}
