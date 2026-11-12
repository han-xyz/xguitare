use std::time::Instant;

use xguitare_gui::model::{
    AddPlace, Command, Direction, Dragged, FixedUnit, LayoutDrop, RackItem, RackLayout, ScreenRect,
    UNDO_FOR, add_command, drag_outcome, layout_drop, rack_ear_labels,
};

fn ids(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_string()).collect()
}

fn drawn(layout: &RackLayout, modules: &[&str]) -> Vec<String> {
    layout
        .order(&ids(modules))
        .into_iter()
        .map(|item| match item {
            RackItem::Fixed(FixedUnit::Meters) => "[meters]".to_string(),
            RackItem::Fixed(FixedUnit::Tuner) => "[tuner]".to_string(),
            RackItem::Module(id) => id,
        })
        .collect()
}

const RACK: [&str; 3] = ["compressor", "preamp", "chorus"];

fn tuner_before_chorus() -> RackLayout {
    let mut layout = RackLayout::default();
    layout.place(FixedUnit::Tuner, 4, &ids(&RACK));
    layout
}

#[test]
fn default_is_meters_then_tuner_at_the_top() {
    let layout = RackLayout::default();
    assert_eq!(
        drawn(&layout, &RACK),
        ["[meters]", "[tuner]", "compressor", "preamp", "chorus"]
    );
    assert_eq!(drawn(&layout, &[]), ["[meters]", "[tuner]"]);
    assert!(layout.is_shown(FixedUnit::Meters) && layout.is_shown(FixedUnit::Tuner));
}

#[test]
fn a_fixed_unit_moves_to_the_drop_slot() {
    assert_eq!(
        drawn(&tuner_before_chorus(), &RACK),
        ["[meters]", "compressor", "preamp", "[tuner]", "chorus"]
    );
    let mut layout = RackLayout::default();
    layout.place(FixedUnit::Meters, 5, &ids(&RACK));
    assert_eq!(
        drawn(&layout, &RACK),
        ["[tuner]", "compressor", "preamp", "chorus", "[meters]"]
    );
    layout.place(FixedUnit::Meters, 0, &ids(&RACK));
    assert_eq!(
        drawn(&layout, &RACK),
        ["[meters]", "[tuner]", "compressor", "preamp", "chorus"]
    );
}

#[test]
fn a_fixed_unit_dropped_beside_itself_stays() {
    for slot in [1, 2] {
        let mut layout = RackLayout::default();
        layout.place(FixedUnit::Tuner, slot, &ids(&RACK));
        assert_eq!(layout, RackLayout::default());
    }
}

#[test]
fn up_and_down_step_past_the_next_unit() {
    let mut layout = RackLayout::default();
    layout.step(FixedUnit::Tuner, Direction::Up, &ids(&RACK));
    assert_eq!(
        drawn(&layout, &RACK),
        ["[tuner]", "[meters]", "compressor", "preamp", "chorus"]
    );
    layout.step(FixedUnit::Meters, Direction::Down, &ids(&RACK));
    assert_eq!(
        drawn(&layout, &RACK),
        ["[tuner]", "compressor", "[meters]", "preamp", "chorus"]
    );
}

#[test]
fn a_fixed_unit_keeps_its_place_when_modules_are_added_or_moved() {
    let layout = tuner_before_chorus();
    assert_eq!(
        drawn(
            &layout,
            &["gate", "compressor", "preamp", "chorus", "delay"]
        ),
        [
            "[meters]",
            "gate",
            "compressor",
            "preamp",
            "[tuner]",
            "chorus",
            "delay"
        ]
    );
    assert_eq!(
        drawn(&layout, &["compressor", "chorus", "preamp"]),
        ["[meters]", "compressor", "[tuner]", "chorus", "preamp"]
    );
}

#[test]
fn a_removed_module_hands_its_fixed_unit_to_the_next_later_one() {
    let mut layout = RackLayout::default();
    layout.place(FixedUnit::Tuner, 3, &ids(&RACK));
    layout.follow(&ids(&RACK), &ids(&["compressor", "chorus"]));
    assert_eq!(
        drawn(&layout, &["compressor", "chorus"]),
        ["[meters]", "compressor", "[tuner]", "chorus"]
    );
    let mut layout = tuner_before_chorus();
    layout.follow(&ids(&RACK), &ids(&["compressor", "preamp"]));
    assert_eq!(
        drawn(&layout, &["compressor", "preamp", "delay"]),
        ["[meters]", "compressor", "preamp", "delay", "[tuner]"]
    );
}

#[test]
fn hide_then_show_at_the_top_or_at_a_slot() {
    let mut layout = RackLayout::default();
    let hidden = layout.hide(FixedUnit::Tuner, Instant::now());
    assert_eq!(hidden.map(|hidden| hidden.unit), Some(FixedUnit::Tuner));
    assert!(!layout.is_shown(FixedUnit::Tuner));
    assert_eq!(
        drawn(&layout, &RACK),
        ["[meters]", "compressor", "preamp", "chorus"]
    );
    assert_eq!(layout.hide(FixedUnit::Tuner, Instant::now()), None);
    layout.place(FixedUnit::Tuner, 0, &ids(&RACK));
    assert_eq!(
        drawn(&layout, &RACK),
        ["[tuner]", "[meters]", "compressor", "preamp", "chorus"]
    );
    layout.hide(FixedUnit::Tuner, Instant::now());
    layout.place(FixedUnit::Tuner, 3, &ids(&RACK));
    assert_eq!(
        drawn(&layout, &RACK),
        ["[meters]", "compressor", "preamp", "[tuner]", "chorus"]
    );
}

#[test]
fn undo_of_a_hide_puts_the_unit_back_in_its_place() {
    let mut layout = tuner_before_chorus();
    let now = Instant::now();
    let hidden = layout.hide(FixedUnit::Tuner, now).unwrap();
    assert!(hidden.is_shown(now));
    assert!(!hidden.is_shown(now + UNDO_FOR));
    layout.undo_hide(&hidden);
    assert_eq!(layout, tuner_before_chorus());
    let mut layout = RackLayout::default();
    let hidden = layout.hide(FixedUnit::Meters, now).unwrap();
    layout.undo_hide(&hidden);
    assert_eq!(layout, RackLayout::default());
}

#[test]
fn stored_layout_round_trips() {
    let mut both_hidden = RackLayout::default();
    both_hidden.hide(FixedUnit::Meters, Instant::now());
    both_hidden.hide(FixedUnit::Tuner, Instant::now());
    let mut at_end = RackLayout::default();
    at_end.place(FixedUnit::Meters, 5, &ids(&RACK));
    for layout in [
        RackLayout::default(),
        tuner_before_chorus(),
        both_hidden,
        at_end,
    ] {
        assert_eq!(RackLayout::from_stored(&layout.to_stored()), layout);
    }
}

#[test]
fn a_bad_stored_layout_reads_as_the_default() {
    for stored in [
        "",
        "nonsense",
        "{}",
        r#"[{"unit": "amp", "at": "front"}]"#,
        r#"[{"unit": "tuner", "at": "middle"}]"#,
        r#"[{"unit": "tuner", "at": "front"}, {"unit": "tuner", "at": "end"}]"#,
        r#"[{"unit": "tuner", "before": 3}]"#,
    ] {
        assert_eq!(
            RackLayout::from_stored(stored),
            RackLayout::default(),
            "{stored}"
        );
    }
}

#[test]
fn ear_numbers_count_modules_only() {
    let order = tuner_before_chorus().order(&ids(&RACK));
    assert_eq!(
        rack_ear_labels(&order),
        [
            None,
            Some("1".to_string()),
            Some("2".to_string()),
            None,
            Some("3".to_string())
        ]
    );
}

const WINDOW: ScreenRect = ScreenRect {
    left: 0.0,
    top: 0.0,
    right: 1440.0,
    bottom: 900.0,
};

const RACK_AREA: ScreenRect = ScreenRect {
    left: 300.0,
    top: 80.0,
    right: 1300.0,
    bottom: 800.0,
};

/// Meters, Tuner, compressor, preamp, chorus, each 70 px tall.
const EDGES: [f32; 6] = [100.0, 170.0, 240.0, 310.0, 380.0, 450.0];

#[test]
fn a_fixed_unit_released_in_the_rack_is_placed_and_outside_it_hidden() {
    let drop =
        |dragged: Dragged, pointer| layout_drop(&dragged, pointer, &WINDOW, &RACK_AREA, &EDGES);
    let tuner = || Dragged::Fixed(FixedUnit::Tuner);
    assert_eq!(
        drop(tuner(), Some((600.0, 360.0))),
        Some(LayoutDrop::Place(FixedUnit::Tuner, 4))
    );
    assert_eq!(
        drop(Dragged::Fixed(FixedUnit::Meters), Some((100.0, 300.0))),
        Some(LayoutDrop::Hide(FixedUnit::Meters))
    );
    assert_eq!(drop(tuner(), None), None);
    assert_eq!(drop(tuner(), Some((1500.0, 300.0))), None);
    assert_eq!(
        drop(Dragged::FixedChip(FixedUnit::Tuner), Some((600.0, 90.0))),
        Some(LayoutDrop::Place(FixedUnit::Tuner, 0))
    );
    assert_eq!(
        drop(Dragged::FixedChip(FixedUnit::Tuner), Some((100.0, 300.0))),
        None
    );
    assert_eq!(drop(Dragged::Unit(0), Some((600.0, 360.0))), None);
}

#[test]
fn a_module_dragged_past_fixed_units_counts_modules_only() {
    let order = RackLayout::default().order(&ids(&RACK));
    let outcome = |dragged: Dragged, y| {
        drag_outcome(
            &dragged,
            Some((600.0, y)),
            &WINDOW,
            &RACK_AREA,
            &EDGES,
            &order,
        )
    };
    assert_eq!(
        outcome(Dragged::Unit(0), 440.0),
        Some(Command::RackMove {
            id: "compressor".to_string(),
            to: 2,
        })
    );
    assert_eq!(outcome(Dragged::Unit(0), 90.0), None);
    assert_eq!(
        outcome(Dragged::Unit(2), 120.0),
        Some(Command::RackMove {
            id: "chorus".to_string(),
            to: 0,
        })
    );
    assert_eq!(
        outcome(Dragged::Tile("gate".to_string()), 220.0),
        Some(add_command("gate", &AddPlace::Front))
    );
    assert_eq!(outcome(Dragged::Fixed(FixedUnit::Tuner), 440.0), None);
}
