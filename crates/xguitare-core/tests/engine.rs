use std::sync::atomic::AtomicU32;

use xguitare_core::engine::{block_peak, raise_peak, take_peak};

#[test]
fn block_peak_is_max_abs() {
    assert_eq!(block_peak(&[0.1, -0.7, 0.3]), 0.7);
    assert_eq!(block_peak(&[0.0; 8]), 0.0);
    assert_eq!(block_peak(&[]), 0.0);
}

#[test]
fn peak_atomic_only_raises_and_swaps_to_zero() {
    let peak = AtomicU32::new(0);
    raise_peak(&peak, 0.5);
    raise_peak(&peak, 0.25);
    assert_eq!(take_peak(&peak), 0.5);
    assert_eq!(take_peak(&peak), 0.0);
}
