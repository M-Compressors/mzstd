//! `oracle::compress2_capacity` with destinations zstd 1.5.7 overran.
//!
//! With `ZSTD_c_targetCBlockSize`, zstd 1.5.7's sub-block writer copied
//! table descriptions without checking the room left, so `ZSTD_compress2`
//! into a destination somewhat smaller than the frame wrote past it. The
//! output-capacity fuzz target found the heap corruption this caused
//! (2026-11-09), and the oracle keeps a guard band after the destination
//! that reports an overrun as an error. zstd 1.6.0 checks the room (#4981):
//! these destinations now fail with `Destination buffer is too small`.

use zstd_ffi::oracle::{self, CAPACITY_OVERRUN, Params};
use zstd_ffi::{ZSTD_c_checksumFlag, ZSTD_c_contentSizeFlag, ZSTD_c_targetCBlockSize, corpus};

fn target_block_params(target: i32) -> Params {
    Params::level(4)
        .with(ZSTD_c_checksumFlag, 1)
        .with(ZSTD_c_contentSizeFlag, 0)
        .with(ZSTD_c_targetCBlockSize, target)
}

/// Every capacity from zero to the input's bound either returns the frame
/// or fails, none corrupts memory, and the bound itself succeeds. Returns
/// how many capacities the reference overran.
fn sweep(input: &[u8], params: &Params) -> usize {
    let frame = oracle::compress2(input, params).expect("reference frame");
    let bound = oracle::compress_bound(input.len());
    let mut overruns = 0;
    for capacity in 0..=bound {
        match oracle::compress2_capacity(input, params, capacity) {
            Ok(output) => assert_eq!(output, frame, "capacity {capacity}"),
            Err(error) => {
                assert!(capacity < bound, "capacity {capacity}: {error}");
                overruns += usize::from(error == CAPACITY_OVERRUN);
            }
        }
    }
    overruns
}

#[test]
fn target_block_frames_stay_inside_short_destinations() {
    let input = corpus::text(1_300, 7);
    for target in [1340, 2048, 131_072] {
        // zstd 1.5.7 overran some of these capacities for every target.
        assert_eq!(
            sweep(&input, &target_block_params(target)),
            0,
            "target {target}"
        );
    }
}

#[test]
fn capacities_without_a_target_block_size_never_overrun() {
    let input = corpus::text(1_300, 7);
    assert_eq!(sweep(&input, &Params::level(4)), 0);
}

#[test]
fn an_empty_destination_fails_cleanly() {
    let input = corpus::text(1_300, 7);
    let error = oracle::compress2_capacity(&input, &target_block_params(1340), 0).unwrap_err();
    assert_ne!(error, CAPACITY_OVERRUN);
}
