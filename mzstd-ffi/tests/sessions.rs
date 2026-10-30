//! The exported session functions, called through their ABI exactly as C
//! would, against zstd's `ZSTD_compressStream2` and `ZSTD_decompressStream`.

use mzstd_ffi::{
    MZSTD_CONTENT_SIZE_UNKNOWN, MzstdDecodeOperation, MzstdDecoder, MzstdEncoder, MzstdOperation,
    MzstdResult, MzstdStatus, mzstd_compress_bound, mzstd_decoder_create, mzstd_decoder_free,
    mzstd_decoder_process, mzstd_decoder_reset, mzstd_encoder_create, mzstd_encoder_free,
    mzstd_encoder_process, mzstd_encoder_reset,
};
use std::ffi::c_int;
use std::ptr;
use zstd_ffi::corpus;
use zstd_ffi::oracle::{self, Directive, Params};

/// An encoder handle released on drop.
struct Encoder(*mut MzstdEncoder);

impl Encoder {
    fn new(level: c_int) -> Self {
        let mut handle = ptr::null_mut();
        // SAFETY: `handle` is a writable pointer slot.
        let status = unsafe { mzstd_encoder_create(level, &mut handle) };
        assert_eq!(status, MzstdResult::Ok);
        assert!(!handle.is_null());
        Self(handle)
    }

    fn reset(&mut self, pledged: u64) -> MzstdResult {
        // SAFETY: the handle is live and used by this thread only.
        unsafe { mzstd_encoder_reset(self.0, pledged) }
    }

    /// One call with all of `input` offered and `capacity` bytes of room.
    fn process(
        &mut self,
        input: &[u8],
        capacity: usize,
        operation: c_int,
    ) -> (MzstdResult, usize, Vec<u8>, Option<MzstdStatus>) {
        let mut output = vec![0u8; capacity];
        let (mut input_len, mut output_len) = (input.len(), capacity);
        let mut raw = MzstdStatus::NeedsInput;
        // SAFETY: the handle is live and the buffers have the stated lengths.
        let result = unsafe {
            mzstd_encoder_process(
                self.0,
                input.as_ptr(),
                &mut input_len,
                output.as_mut_ptr(),
                &mut output_len,
                operation,
                &mut raw,
            )
        };
        output.truncate(output_len);
        let status = (result == MzstdResult::Ok).then_some(raw);
        (result, input_len, output, status)
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        // SAFETY: the handle is live and released once.
        unsafe { mzstd_encoder_free(self.0) };
    }
}

/// A decoder handle released on drop.
struct Decoder(*mut MzstdDecoder);

impl Decoder {
    fn new(window_log_max: c_int) -> Self {
        let mut handle = ptr::null_mut();
        // SAFETY: `handle` is a writable pointer slot.
        let status = unsafe { mzstd_decoder_create(window_log_max, &mut handle) };
        assert_eq!(status, MzstdResult::Ok);
        Self(handle)
    }

    fn reset(&mut self) -> MzstdResult {
        // SAFETY: the handle is live and used by this thread only.
        unsafe { mzstd_decoder_reset(self.0) }
    }

    fn process(
        &mut self,
        input: &[u8],
        capacity: usize,
        operation: c_int,
    ) -> (MzstdResult, usize, Vec<u8>, Option<MzstdStatus>) {
        let mut output = vec![0u8; capacity];
        let (mut input_len, mut output_len) = (input.len(), capacity);
        let mut raw = MzstdStatus::NeedsInput;
        // SAFETY: the handle is live and the buffers have the stated lengths.
        let result = unsafe {
            mzstd_decoder_process(
                self.0,
                input.as_ptr(),
                &mut input_len,
                output.as_mut_ptr(),
                &mut output_len,
                operation,
                &mut raw,
            )
        };
        output.truncate(output_len);
        let status = (result == MzstdResult::Ok).then_some(raw);
        (result, input_len, output, status)
    }

    /// Decodes `input` fed `chunk` bytes at a time into `capacity`-byte
    /// buffers. More input is offered only once the offered input is
    /// consumed; once the last byte is offered, every call finishes with
    /// exactly the unconsumed rest.
    fn decode(&mut self, input: &[u8], chunk: usize, capacity: usize) -> Result<Vec<u8>, ()> {
        let mut decoded = Vec::new();
        let (mut position, mut offered) = (0, 0);
        loop {
            if position == offered {
                offered = (offered + chunk).min(input.len());
            }
            let operation = if offered == input.len() {
                MzstdDecodeOperation::Finish
            } else {
                MzstdDecodeOperation::Process
            };
            let (result, consumed, output, status) =
                self.process(&input[position..offered], capacity, operation as c_int);
            if result != MzstdResult::Ok {
                return Err(());
            }
            position += consumed;
            decoded.extend_from_slice(&output);
            if status == Some(MzstdStatus::Finished) {
                return Ok(decoded);
            }
        }
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        // SAFETY: the handle is live and released once.
        unsafe { mzstd_decoder_free(self.0) };
    }
}

fn directive(operation: MzstdOperation) -> Directive {
    match operation {
        MzstdOperation::Continue => Directive::Continue,
        MzstdOperation::Flush => Directive::Flush,
        MzstdOperation::End => Directive::End,
    }
}

/// Streams `steps` through `encoder` exactly as
/// `oracle::compress_stream_with_output` streams them through the reference:
/// each chunk is offered until consumed and, for `Flush` and `End`, until the
/// operation completes, with `capacity` bytes of room per call.
fn stream(encoder: &mut Encoder, steps: &[(&[u8], MzstdOperation)], capacity: usize) -> Vec<u8> {
    let mut frame = Vec::new();
    let mut run = |data: &[u8], operation: MzstdOperation| {
        let mut position = 0;
        loop {
            let (result, consumed, output, status) =
                encoder.process(&data[position..], capacity, operation as c_int);
            assert_eq!(result, MzstdResult::Ok);
            position += consumed;
            frame.extend_from_slice(&output);
            let done = match operation {
                MzstdOperation::Continue => position == data.len(),
                MzstdOperation::Flush | MzstdOperation::End => {
                    status != Some(MzstdStatus::NeedsOutput)
                }
            };
            if done {
                return;
            }
        }
    };
    for &(data, operation) in steps {
        run(data, operation);
    }
    if steps
        .last()
        .is_none_or(|&(_, operation)| operation != MzstdOperation::End)
    {
        run(&[], MzstdOperation::End);
    }
    frame
}

/// `steps` as the reference's directives.
fn reference(params: &Params, steps: &[(&[u8], MzstdOperation)], capacity: usize) -> Vec<u8> {
    let steps: Vec<_> = steps
        .iter()
        .map(|&(data, operation)| (data, directive(operation)))
        .collect();
    oracle::compress_stream_with_output(params, &steps, capacity).expect("reference")
}

/// Chunkings of `input`: whole with `End`, continued pieces, and pieces with
/// a flush in between.
fn schedules(input: &[u8]) -> Vec<Vec<(&[u8], MzstdOperation)>> {
    use MzstdOperation::{Continue, End, Flush};
    let third = input.len() / 3;
    let (a, rest) = input.split_at(third);
    let (b, c) = rest.split_at(third);
    vec![
        vec![(input, End)],
        vec![(input, Continue)],
        vec![(a, Continue), (b, Continue), (c, End)],
        vec![(a, Continue), (&[], Flush), (b, Flush), (c, Continue)],
    ]
}

#[test]
fn sessions_are_byte_identical_to_zstd_compress_stream2() {
    for input in [
        Vec::new(),
        b"a".to_vec(),
        corpus::text(60_000, 1),
        corpus::binary(40_000, 2),
        corpus::random(20_000, 3),
        corpus::mixed(300_000, 4),
    ] {
        for level in [-5, 1, 3, 7, 12, 19] {
            let mut encoder = Encoder::new(level);
            for steps in schedules(&input) {
                for capacity in [1 << 17, 4096, 7] {
                    let ours = stream(&mut encoder, &steps, capacity);
                    let theirs = reference(&Params::level(level), &steps, capacity);
                    assert_eq!(ours, theirs, "level {level}, capacity {capacity}");
                    assert_eq!(encoder.reset(MZSTD_CONTENT_SIZE_UNKNOWN), MzstdResult::Ok);
                }
            }
        }
    }
}

#[test]
fn a_pledged_size_is_recorded_as_the_reference_records_it() {
    let input = corpus::text(200_000, 5);
    for level in [1, 3, 19] {
        let mut encoder = Encoder::new(level);
        for steps in schedules(&input) {
            assert_eq!(encoder.reset(input.len() as u64), MzstdResult::Ok);
            let ours = stream(&mut encoder, &steps, 1 << 17);
            let params = Params::level(level).with_pledged(input.len() as u64);
            assert_eq!(ours, reference(&params, &steps, 1 << 17), "level {level}");
        }
    }
}

#[test]
fn input_contradicting_the_pledged_size_fails_until_the_next_reset() {
    let mut encoder = Encoder::new(3);
    assert_eq!(encoder.reset(4), MzstdResult::Ok);
    let (result, consumed, output, status) =
        encoder.process(b"five!", 64, MzstdOperation::End as c_int);
    assert_eq!(
        (result, consumed, output, status),
        (MzstdResult::Error, 0, Vec::new(), None)
    );
    let (result, ..) = encoder.process(b"four", 64, MzstdOperation::End as c_int);
    assert_eq!(result, MzstdResult::Error);
    assert_eq!(encoder.reset(4), MzstdResult::Ok);
    let (result, consumed, frame, status) =
        encoder.process(b"four", 64, MzstdOperation::End as c_int);
    assert_eq!(
        (result, consumed, status),
        (MzstdResult::Ok, 4, Some(MzstdStatus::Finished))
    );
    assert_eq!(oracle::decompress(&frame, 4).expect("reference"), b"four");
}

#[test]
fn a_finished_frame_reports_finished_again_without_moving_bytes() {
    let mut encoder = Encoder::new(3);
    let (_, _, _, status) = encoder.process(b"done", 64, MzstdOperation::End as c_int);
    assert_eq!(status, Some(MzstdStatus::Finished));
    for operation in [MzstdOperation::Continue, MzstdOperation::End] {
        assert_eq!(
            encoder.process(b"more", 64, operation as c_int),
            (MzstdResult::Ok, 0, Vec::new(), Some(MzstdStatus::Finished))
        );
    }
}

#[test]
fn a_reset_abandons_an_unfinished_frame() {
    let mut encoder = Encoder::new(3);
    let (result, consumed, ..) =
        encoder.process(b"abandoned", 64, MzstdOperation::Continue as c_int);
    assert_eq!((result, consumed), (MzstdResult::Ok, 9));
    assert_eq!(encoder.reset(MZSTD_CONTENT_SIZE_UNKNOWN), MzstdResult::Ok);
    let steps = [(&b"kept"[..], MzstdOperation::End)];
    assert_eq!(
        stream(&mut encoder, &steps, 64),
        reference(&Params::level(3), &steps, 64)
    );
}

#[test]
fn decoding_round_trips_at_every_chunking() {
    let input = corpus::mixed(300_000, 6);
    let frame = {
        let mut encoder = Encoder::new(5);
        stream(&mut encoder, &[(&input, MzstdOperation::End)], 1 << 17)
    };
    let mut decoder = Decoder::new(0);
    for (chunk, capacity) in [(frame.len(), 1 << 20), (4096, 1 << 17), (1, 4096), (7, 1)] {
        assert_eq!(decoder.reset(), MzstdResult::Ok);
        let decoded = decoder.decode(&frame, chunk, capacity).expect("decodes");
        assert_eq!(decoded, input, "chunk {chunk}, capacity {capacity}");
    }
}

#[test]
fn reference_frames_with_checksums_decode() {
    let input = corpus::text(500_000, 7);
    let params = Params::level(19).with(zstd_ffi::ZSTD_c_checksumFlag, 1);
    let frame = oracle::compress2(&input, &params).expect("reference");
    let mut decoder = Decoder::new(0);
    assert_eq!(decoder.decode(&frame, 1000, 1 << 16), Ok(input));
    let mut corrupt = frame.clone();
    let last = corrupt.len() - 1;
    corrupt[last] ^= 1;
    assert_eq!(decoder.reset(), MzstdResult::Ok);
    assert_eq!(decoder.decode(&corrupt, 1000, 1 << 16), Err(()));
}

#[test]
fn concatenated_and_skippable_frames_decode_in_order() {
    let a = oracle::compress(b"first ", 1);
    let b = oracle::compress(b"second", 19);
    let skippable = [0x50, 0x2a, 0x4d, 0x18, 3, 0, 0, 0, 1, 2, 3];
    let input = [&skippable[..], &a, &skippable, &b, &skippable].concat();
    let mut decoder = Decoder::new(0);
    for chunk in [1, 3, input.len()] {
        assert_eq!(decoder.reset(), MzstdResult::Ok);
        assert_eq!(
            decoder.decode(&input, chunk, 4),
            Ok(b"first second".to_vec())
        );
    }
}

#[test]
fn a_frame_boundary_waits_for_finish() {
    let frame = oracle::compress(b"boundary", 3);
    let mut decoder = Decoder::new(0);
    let (result, consumed, output, status) =
        decoder.process(&frame, 64, MzstdDecodeOperation::Process as c_int);
    assert_eq!(
        (result, consumed, output.as_slice(), status),
        (
            MzstdResult::Ok,
            frame.len(),
            &b"boundary"[..],
            Some(MzstdStatus::NeedsInput)
        )
    );
    assert_eq!(
        decoder.process(&[], 64, MzstdDecodeOperation::Finish as c_int),
        (MzstdResult::Ok, 0, Vec::new(), Some(MzstdStatus::Finished))
    );
    assert_eq!(
        decoder.process(&frame, 64, MzstdDecodeOperation::Finish as c_int),
        (MzstdResult::Ok, 0, Vec::new(), Some(MzstdStatus::Finished))
    );
}

#[test]
fn truncated_input_fails_until_the_next_reset() {
    let frame = oracle::compress(&corpus::text(10_000, 8), 3);
    let mut decoder = Decoder::new(0);
    let truncated = &frame[..frame.len() - 1];
    assert_eq!(
        decoder.process(truncated, 1 << 16, MzstdDecodeOperation::Finish as c_int),
        (MzstdResult::Error, 0, Vec::new(), None)
    );
    assert_eq!(
        decoder
            .process(&[], 1 << 16, MzstdDecodeOperation::Finish as c_int)
            .0,
        MzstdResult::Error
    );
    assert_eq!(decoder.reset(), MzstdResult::Ok);
    assert!(decoder.decode(&frame, frame.len(), 1 << 16).is_ok());
}

#[test]
fn the_window_limit_is_the_reference_default_unless_raised() {
    // Streamed without a known size, the frame declares the full window.
    let input = corpus::text(1000, 9);
    let params = Params::level(3).with(zstd_ffi::ZSTD_c_windowLog, 28);
    let steps = [(&input[..], Directive::Continue)];
    let frame = oracle::compress_stream(&params, &steps).expect("reference");
    assert!(oracle::decompress_stream(&frame, frame.len(), None).is_err());
    assert_eq!(Decoder::new(0).decode(&frame, frame.len(), 4096), Err(()));
    assert_eq!(Decoder::new(27).decode(&frame, frame.len(), 4096), Err(()));
    assert_eq!(
        Decoder::new(28).decode(&frame, frame.len(), 4096),
        Ok(input)
    );
}

#[test]
fn create_rejects_out_of_range_arguments_and_stores_null() {
    for level in [-131_073, 23, c_int::MIN, c_int::MAX] {
        let mut handle = ptr::dangling_mut::<MzstdEncoder>();
        // SAFETY: `handle` is a writable pointer slot.
        let status = unsafe { mzstd_encoder_create(level, &mut handle) };
        assert_eq!(status, MzstdResult::InvalidParameter, "{level}");
        assert!(handle.is_null());
    }
    for log in [-1, 9, 32, c_int::MAX] {
        let mut handle = ptr::dangling_mut::<MzstdDecoder>();
        // SAFETY: as above.
        let status = unsafe { mzstd_decoder_create(log, &mut handle) };
        assert_eq!(status, MzstdResult::InvalidParameter, "{log}");
        assert!(handle.is_null());
    }
}

#[test]
fn create_rejects_a_null_or_misaligned_out_pointer() {
    // SAFETY: the null out-pointer is detected before anything is written.
    let status = unsafe { mzstd_encoder_create(3, ptr::null_mut()) };
    assert_eq!(status, MzstdResult::InvalidParameter);
    // SAFETY: as above.
    let status = unsafe { mzstd_decoder_create(0, ptr::null_mut()) };
    assert_eq!(status, MzstdResult::InvalidParameter);
    let mut storage = [0u8; 16];
    let misaligned = storage.as_mut_ptr().wrapping_add(1).cast();
    // SAFETY: the misaligned out-pointer is detected before it is written.
    let status = unsafe { mzstd_encoder_create(3, misaligned) };
    assert_eq!(status, MzstdResult::InvalidParameter);
    assert_eq!(storage, [0; 16]);
}

#[test]
fn null_handles_are_rejected_and_freeing_them_is_a_no_op() {
    // SAFETY: null handles are detected before they are dereferenced.
    unsafe {
        assert_eq!(
            mzstd_encoder_reset(ptr::null_mut(), 0),
            MzstdResult::InvalidParameter
        );
        assert_eq!(
            mzstd_decoder_reset(ptr::null_mut()),
            MzstdResult::InvalidParameter
        );
        mzstd_encoder_free(ptr::null_mut());
        mzstd_decoder_free(ptr::null_mut());
    }
    let mut input_len = 1;
    let mut output_len = 8;
    let mut output = [0u8; 8];
    let mut status = MzstdStatus::NeedsOutput;
    // SAFETY: as above.
    let result = unsafe {
        mzstd_decoder_process(
            ptr::null_mut(),
            b"x".as_ptr(),
            &mut input_len,
            output.as_mut_ptr(),
            &mut output_len,
            MzstdDecodeOperation::Process as c_int,
            &mut status,
        )
    };
    assert_eq!(result, MzstdResult::InvalidParameter);
    assert_eq!((input_len, output_len), (0, 0));
    assert_eq!(status, MzstdStatus::NeedsOutput);
}

#[test]
fn undefined_operations_are_rejected_without_affecting_the_session() {
    let mut encoder = Encoder::new(3);
    for operation in [-1, 3, c_int::MAX] {
        assert_eq!(
            encoder.process(b"data", 64, operation),
            (MzstdResult::InvalidParameter, 0, Vec::new(), None)
        );
    }
    let steps = [(&b"data"[..], MzstdOperation::End)];
    assert_eq!(
        stream(&mut encoder, &steps, 64),
        reference(&Params::level(3), &steps, 64)
    );
    let mut decoder = Decoder::new(0);
    assert_eq!(
        decoder.process(b"data", 64, 2),
        (MzstdResult::InvalidParameter, 0, Vec::new(), None)
    );
    let frame = oracle::compress(b"data", 3);
    assert_eq!(decoder.decode(&frame, 1, 1), Ok(b"data".to_vec()));
}

#[test]
fn a_null_out_pointer_is_rejected_without_any_write() {
    let mut encoder = Encoder::new(3);
    let mut output = [0u8; 64];
    let mut input_len = 4;
    let mut output_len = output.len();
    let mut status = MzstdStatus::NeedsOutput;
    // SAFETY: the null status pointer is detected before anything is written.
    let result = unsafe {
        mzstd_encoder_process(
            encoder.0,
            b"data".as_ptr(),
            &mut input_len,
            output.as_mut_ptr(),
            &mut output_len,
            MzstdOperation::End as c_int,
            ptr::null_mut(),
        )
    };
    assert_eq!(result, MzstdResult::InvalidParameter);
    assert_eq!((input_len, output_len), (4, 64));
    // SAFETY: the null length pointer is detected likewise.
    let result = unsafe {
        mzstd_encoder_process(
            encoder.0,
            b"data".as_ptr(),
            ptr::null_mut(),
            output.as_mut_ptr(),
            &mut output_len,
            MzstdOperation::End as c_int,
            &mut status,
        )
    };
    assert_eq!(result, MzstdResult::InvalidParameter);
    assert_eq!((output_len, status), (64, MzstdStatus::NeedsOutput));
    let (result, consumed, _, status) = encoder.process(b"data", 64, MzstdOperation::End as c_int);
    assert_eq!(
        (result, consumed, status),
        (MzstdResult::Ok, 4, Some(MzstdStatus::Finished))
    );
}

#[test]
fn overlapping_arguments_are_rejected() {
    let mut encoder = Encoder::new(3);
    let mut buffer = [0u8; 256];
    let base = buffer.as_mut_ptr();
    let mut input_len = 128;
    let mut output_len = 128;
    let mut status = MzstdStatus::NeedsInput;
    // SAFETY: the overlap is detected before either buffer is touched.
    let result = unsafe {
        mzstd_encoder_process(
            encoder.0,
            base,
            &mut input_len,
            base.wrapping_add(64),
            &mut output_len,
            MzstdOperation::End as c_int,
            &mut status,
        )
    };
    assert_eq!(result, MzstdResult::InvalidParameter);
    assert_eq!((input_len, output_len), (0, 0));
    let mut slots = [0usize; 16];
    slots[0] = 64;
    slots[1] = 1;
    let slot = slots.as_mut_ptr();
    // SAFETY: the output overlapping the input length slot is detected
    // before any write.
    let result = unsafe {
        mzstd_encoder_process(
            encoder.0,
            b"x".as_ptr(),
            slot.wrapping_add(1),
            slot.cast::<u8>(),
            slot,
            MzstdOperation::End as c_int,
            &mut status,
        )
    };
    assert_eq!(result, MzstdResult::InvalidParameter);
    let mut shared = 4;
    // SAFETY: one slot passed as both lengths is detected before any write.
    let result = unsafe {
        mzstd_encoder_process(
            encoder.0,
            b"data".as_ptr(),
            &raw mut shared,
            ptr::null_mut(),
            &raw mut shared,
            MzstdOperation::End as c_int,
            &mut status,
        )
    };
    assert_eq!(result, MzstdResult::InvalidParameter);
    let (result, ..) = encoder.process(b"data", 64, MzstdOperation::End as c_int);
    assert_eq!(result, MzstdResult::Ok);
}

#[test]
fn empty_buffers_may_be_null() {
    let encoder = Encoder::new(3);
    let mut input_len = 0;
    let mut output_len = 0;
    let mut status = MzstdStatus::Finished;
    // SAFETY: both buffers are empty and may be null.
    let result = unsafe {
        mzstd_encoder_process(
            encoder.0,
            ptr::null(),
            &mut input_len,
            ptr::null_mut(),
            &mut output_len,
            MzstdOperation::End as c_int,
            &mut status,
        )
    };
    assert_eq!(
        (result, input_len, output_len, status),
        (MzstdResult::Ok, 0, 0, MzstdStatus::NeedsOutput)
    );
    let mut decoder = Decoder::new(0);
    // SAFETY: as above.
    let result = unsafe {
        mzstd_decoder_process(
            decoder.0,
            ptr::null(),
            &mut input_len,
            ptr::null_mut(),
            &mut output_len,
            MzstdDecodeOperation::Process as c_int,
            &mut status,
        )
    };
    assert_eq!(
        (result, input_len, output_len, status),
        (MzstdResult::Ok, 0, 0, MzstdStatus::NeedsInput)
    );
    assert_eq!(decoder.reset(), MzstdResult::Ok);
}

#[test]
fn handles_move_between_threads_and_run_side_by_side() {
    struct Send<T>(T);
    // SAFETY: each handle is used by one thread at a time, as documented.
    unsafe impl<T> std::marker::Send for Send<T> {}
    let input = corpus::mixed(200_000, 10);
    let steps = [(&input[..], MzstdOperation::End)];
    let expected = reference(&Params::level(7), &steps, 1 << 16);
    std::thread::scope(|scope| {
        for _ in 0..8 {
            let encoder = Send(Encoder::new(7));
            let decoder = Send(Decoder::new(0));
            let (input, steps, expected) = (&input, &steps, &expected);
            scope.spawn(move || {
                let (Send(mut encoder), Send(mut decoder)) = (encoder, decoder);
                let frame = stream(&mut encoder, steps, 1 << 16);
                assert_eq!(&frame, expected);
                assert_eq!(decoder.decode(&frame, 4096, 1 << 16).as_ref(), Ok(input));
            });
        }
    });
}

#[test]
fn a_bound_sized_buffer_ends_a_frame_in_one_call() {
    let input = corpus::text(100_000, 11);
    let mut encoder = Encoder::new(3);
    let (result, consumed, frame, status) = encoder.process(
        &input,
        mzstd_compress_bound(input.len()),
        MzstdOperation::End as c_int,
    );
    assert_eq!(
        (result, consumed, status),
        (MzstdResult::Ok, input.len(), Some(MzstdStatus::Finished))
    );
    assert_eq!(frame, oracle::compress(&input, 3));
}

#[test]
fn session_constants_match_the_c_header() {
    let header = include_str!("../include/mzstd.h");
    let values = [
        ("MZSTD_NEEDS_INPUT", MzstdStatus::NeedsInput as i32),
        ("MZSTD_NEEDS_OUTPUT", MzstdStatus::NeedsOutput as i32),
        ("MZSTD_FINISHED", MzstdStatus::Finished as i32),
        ("MZSTD_CONTINUE", MzstdOperation::Continue as i32),
        ("MZSTD_FLUSH", MzstdOperation::Flush as i32),
        ("MZSTD_END", MzstdOperation::End as i32),
        ("MZSTD_DECODE_PROCESS", MzstdDecodeOperation::Process as i32),
        ("MZSTD_DECODE_FINISH", MzstdDecodeOperation::Finish as i32),
    ];
    for (name, value) in values {
        assert!(header.contains(&format!("{name} = {value}")), "{name}");
    }
    assert!(header.contains("#define MZSTD_CONTENT_SIZE_UNKNOWN UINT64_MAX"));
    assert_eq!(MZSTD_CONTENT_SIZE_UNKNOWN, u64::MAX);
}
