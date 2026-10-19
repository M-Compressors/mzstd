//! The owned framed decoder session: the same events, progress and errors
//! as the borrowed session, plus reinit, recovery and resolver ownership.
#![cfg(all(feature = "framed", feature = "compression", feature = "decompression"))]

mod framed_decoder_support;

use std::sync::Arc;

use framed_decoder_support::*;
use mzstd::DecodeOperation;
use mzstd::framing::*;

/// The calls both session shapes share, recorded as text.
trait Facade {
    fn record(
        &mut self,
        input: &[u8],
        output: &mut [u8],
        operation: DecodeOperation,
    ) -> (String, Option<(usize, bool)>);
    fn record_shorthand(
        &mut self,
        output: &mut [u8],
        operation: DecodeOperation,
    ) -> (String, Option<(usize, bool)>);
    fn counters(&self) -> String;
}

fn describe(
    result: Result<FramedDecodeProgress<'_>, FramedDecodeFailure>,
) -> (String, Option<(usize, bool)>) {
    let text = format!("{result:?}");
    let next = result.ok().map(|p| {
        (
            p.consumed,
            matches!(p.status, FramedDecoderStatus::Finished),
        )
    });
    (text, next)
}

macro_rules! facade {
    ($($generics:tt)*) => {
        fn record(
            &mut self,
            input: &[u8],
            output: &mut [u8],
            operation: DecodeOperation,
        ) -> (String, Option<(usize, bool)>) {
            describe(self.process(input, output, operation))
        }

        fn record_shorthand(
            &mut self,
            output: &mut [u8],
            operation: DecodeOperation,
        ) -> (String, Option<(usize, bool)>) {
            describe(match operation {
                DecodeOperation::Process => self.flush(output),
                DecodeOperation::Finish => self.finish(output),
            })
        }

        fn counters(&self) -> String {
            format!(
                "{} {} {} {} {} {:?}",
                self.total_in(),
                self.total_out(),
                self.total_decoded(),
                self.resources_decoded(),
                self.is_finished(),
                self.input_format()
            )
        }
    };
}

impl Facade for FramedDecoderSession<'_, '_> {
    facade!();
}

impl<R: DictionaryResolver + 'static> Facade for FramedDecoderSessionOwned<R> {
    facade!();
}

/// Feeds `input` in `split`-byte pieces through output buffers cycling over
/// `widths`, declaring EOF on the last piece; stops at the first failure.
fn run(session: &mut impl Facade, input: &[u8], split: usize, widths: &[usize]) -> Vec<String> {
    let mut log = Vec::new();
    let mut offset = 0usize;
    for turn in 0.. {
        let end = input.len().min(offset.saturating_add(split));
        let operation = if end == input.len() {
            DecodeOperation::Finish
        } else {
            DecodeOperation::Process
        };
        let mut output = vec![0; widths[turn % widths.len()]];
        let (text, next) = session.record(&input[offset..end], &mut output, operation);
        log.push(text);
        log.push(session.counters());
        let Some((consumed, finished)) = next else {
            break;
        };
        offset += consumed;
        if finished {
            // Finished is idempotent.
            let (text, _) = session.record(&[], &mut [0; 4], DecodeOperation::Finish);
            log.push(text);
            break;
        }
        assert!(turn < 1 << 20, "no progress");
    }
    log
}

const SCHEDULES: [(usize, &[usize]); 4] = [
    (1, &[0, 1, 2]),
    (3, &[1]),
    (17, &[2, 0, 64]),
    (usize::MAX, &[4096]),
];

fn decoder(config: FramedDecodeConfig) -> FramedDecompressor {
    FramedDecompressor::new(config).expect("config")
}

/// Runs one schedule through a borrowed and an owned session and requires
/// identical records, returning the owned one.
fn parity(
    config: FramedDecodeConfig,
    stream: FramedDecodeStreamConfig,
    input: &[u8],
    split: usize,
    widths: &[usize],
) -> Vec<String> {
    let mut owner = decoder(config);
    let expected = run(
        &mut owner.start(stream).expect("borrowed start"),
        input,
        split,
        widths,
    );
    let mut session = decoder(config).into_session(stream).expect("owned start");
    let actual = run(&mut session, input, split, widths);
    assert_eq!(actual, expected, "split {split}, widths {widths:?}");
    actual
}

fn finished(log: &[String]) -> bool {
    log.iter().any(|line| line.contains("status: Finished"))
}

/// A container with metadata, padding, a hidden resource and a directory.
fn corpus() -> Vec<Vec<u8>> {
    let mut structured = Builder::new(4);
    structured.metadata(1, b"id\x04name");
    structured.resource(b"visible payload");
    structured.padding(3);
    structured.hidden(b"hidden");
    structured.metadata(7, b"GL\x01g");
    structured.directory(0);
    structured.footer();
    vec![
        two_resources(),
        single(b"single resource"),
        structured.finish(),
    ]
}

#[test]
fn events_and_payloads_are_identical_through_both_facades() {
    for input in corpus() {
        for (split, widths) in SCHEDULES {
            let log = parity(
                Default::default(),
                Default::default(),
                &input,
                split,
                widths,
            );
            assert!(finished(&log), "split {split}");
        }
    }
}

#[test]
fn owned_payloads_equal_the_borrowed_event_driver() {
    for input in corpus() {
        let (_, expected) =
            drive(&mut decoder(Default::default()), &input, 5, 3).expect("borrowed");
        let mut session = decoder(Default::default())
            .into_session(Default::default())
            .expect("start");
        let mut payload = Vec::new();
        let mut remaining = &input[..];
        let mut output = [0; 3];
        loop {
            let progress = session
                .process(remaining, &mut output, DecodeOperation::Finish)
                .expect("owned");
            remaining = &remaining[progress.consumed..];
            match progress.status {
                FramedDecoderStatus::Event(FramedEvent::ResourceData(data)) => {
                    payload.extend_from_slice(data.bytes);
                }
                FramedDecoderStatus::Finished => break,
                _ => {}
            }
        }
        assert_eq!(payload, expected);
    }
}

#[test]
fn every_truncated_prefix_fails_identically_through_both_facades() {
    let input = two_resources();
    for end in 0..input.len() {
        let log = parity(
            Default::default(),
            Default::default(),
            &input[..end],
            7,
            &[3],
        );
        assert!(!finished(&log), "end {end}");
        assert!(log.iter().any(|line| line.starts_with("Err")), "end {end}");
    }
}

fn shorten_the_final_suffix(session: &mut impl Facade, input: &[u8]) -> Vec<String> {
    let mut output = [0; 8];
    let (first, next) = session.record(input, &mut output, DecodeOperation::Finish);
    let consumed = next.expect("first call").0;
    let rest = &input[consumed..];
    let (second, _) = session.record(
        &rest[..rest.len() - 1],
        &mut output,
        DecodeOperation::Finish,
    );
    vec![first, second, session.counters()]
}

#[test]
fn the_final_suffix_contract_is_identical_through_both_facades() {
    let input = two_resources();
    let mut owner = decoder(Default::default());
    let expected =
        shorten_the_final_suffix(&mut owner.start(Default::default()).expect("start"), &input);
    let mut session = decoder(Default::default())
        .into_session(Default::default())
        .expect("start");
    let actual = shorten_the_final_suffix(&mut session, &input);
    assert_eq!(actual, expected);
    assert!(actual[1].contains("InvalidState"));
}

#[test]
fn auto_input_decodes_raw_frames_identically() {
    let config = FramedDecodeConfig::default().with_input_mode(InputMode::Auto);
    let raw = [frame(b"raw "), EMPTY_FRAME.to_vec(), frame(b"frames")].concat();
    for (split, widths) in SCHEDULES {
        let log = parity(config, Default::default(), &raw, split, widths);
        assert!(finished(&log));
        assert!(log.iter().any(|line| line.contains("Raw")));
    }
}

fn with_dictionaries<R: DictionaryResolver + 'static>(resolver: R, input: &[u8]) -> Vec<String> {
    let mut session = decoder(Default::default())
        .into_session_with_dictionaries(resolver, Default::default())
        .expect("start");
    run(&mut session, input, 97, &[64, 1])
}

#[test]
fn owned_resolvers_decode_external_references_like_borrowed_ones() {
    let fixture = dictionaries();
    let mut owner = decoder(Default::default());
    let expected = run(
        &mut owner
            .start_with_dictionaries(&fixture, Default::default())
            .expect("borrowed"),
        &fixture.bytes,
        97,
        &[64, 1],
    );
    assert!(finished(&expected));
    let bytes = fixture.bytes.clone();
    let shared = Arc::new(fixture);
    assert_eq!(with_dictionaries(Arc::clone(&shared), &bytes), expected);
    let boxed: Box<dyn DictionaryResolver> = Box::new(dictionaries());
    assert_eq!(with_dictionaries(boxed, &bytes), expected);
    let leaked: &'static Dictionaries = Box::leak(Box::new(dictionaries()));
    assert_eq!(with_dictionaries(leaked, &bytes), expected);
    // Without a resolver the owned session reports the missing dictionary.
    let mut session = decoder(Default::default())
        .into_session(Default::default())
        .expect("start");
    let log = run(&mut session, &bytes, usize::MAX, &[4096]);
    assert!(log.iter().any(|line| line.contains("MissingDictionary")));
    assert_eq!(
        NoDictionaries.resolve(ExternalDictionaryRequest {
            id: EXTERNAL,
            kind: ExternalDictionaryKind::Prefix
        }),
        None
    );
}

#[test]
fn limits_fail_identically_through_both_facades() {
    let input = two_resources();
    for limits in [
        FramedDecodeLimits::default().with_max_output_bytes(Some(4)),
        FramedDecodeLimits::default().with_max_input_bytes(Some(30)),
        FramedDecodeLimits::default().with_max_chunks(Some(1)),
    ] {
        let config = FramedDecodeConfig::default().with_limits(limits);
        for (split, widths) in SCHEDULES {
            let log = parity(config, Default::default(), &input, split, widths);
            assert!(log.iter().any(|line| line.contains("LimitExceeded")));
        }
    }
    for expected in [4u64, 6] {
        let log = parity(
            Default::default(),
            OutputSize::Exact(expected).into(),
            &input,
            5,
            &[2],
        );
        assert!(log.iter().any(|line| line.contains("DeclaredSizeMismatch")));
    }
}

#[test]
fn a_returned_framed_decompressor_is_reusable_after_every_ending() {
    let input = two_resources();
    for ending in ["finished", "needs input", "needs output", "event", "failed"] {
        let mut session = decoder(Default::default())
            .into_session(Default::default())
            .expect("start");
        end_first_object(&mut session, ending);
        let mut decoder = session.into_framed_decompressor();
        assert_eq!(
            decoder.decompress(&input).expect(ending).resources[1].data,
            b"de"
        );
    }
    // A forgotten borrowed session makes the owned start fail as well.
    let mut d = decoder(Default::default());
    std::mem::forget(d.start(Default::default()).expect("start"));
    assert!(matches!(
        d.into_session(Default::default()),
        Err(FramedDecodeError::AbandonedSession)
    ));
}

#[test]
fn owned_framed_sessions_move_between_threads() {
    const fn assert_send<T: Send>() {}
    assert_send::<FramedDecoderSessionOwned>();
    assert_send::<FramedDecoderSessionOwned<Arc<Dictionaries>>>();
    assert_send::<FramedDecoderSessionOwned<&'static Dictionaries>>();

    let bytes = single(b"threaded");
    let mut session = decoder(Default::default())
        .into_session(Default::default())
        .expect("start");
    let mut output = [0; 32];
    let p = session
        .process(&bytes[..13], &mut output, DecodeOperation::Process)
        .expect("signature");
    let rest = bytes[p.consumed..].to_vec();
    let (log, decoder) = std::thread::spawn(move || {
        let log = run(&mut session, &rest, usize::MAX, &[32]);
        (log, session.into_framed_decompressor())
    })
    .join()
    .expect("the worker finished");
    assert!(finished(&log));
    let threaded: Vec<String> = b"threaded".iter().map(u8::to_string).collect();
    assert!(log.iter().any(|line| line.contains(&threaded.join(", "))));
    let mut decoder = decoder;
    assert!(decoder.decompress(&bytes).is_ok());
    assert!(
        format!("{:?}", decoder.into_session(Default::default()))
            .contains("FramedDecoderSessionOwned")
    );
}

fn fresh_borrowed(
    config: FramedDecodeConfig,
    stream: FramedDecodeStreamConfig,
    input: &[u8],
    split: usize,
    widths: &[usize],
) -> Vec<String> {
    let mut owner = decoder(config);
    run(
        &mut owner.start(stream).expect("start"),
        input,
        split,
        widths,
    )
}

/// Leaves `session` in one of the states `reinit` must recover from.
fn end_first_object<R: DictionaryResolver + 'static>(
    session: &mut FramedDecoderSessionOwned<R>,
    ending: &str,
) {
    let first = two_resources();
    let mut output = [0; 64];
    let result = match ending {
        "finished" => {
            assert!(finished(&run(&mut *session, &first, 1000, &[64])));
            return;
        }
        "needs input" => session
            .process(&first[..3], &mut output, DecodeOperation::Process)
            .map(|p| format!("{:?}", p.status)),
        "needs output" => {
            let mut offset = 0;
            loop {
                let p = session
                    .process(&first[offset..], &mut [], DecodeOperation::Process)
                    .expect("zero width");
                offset += p.consumed;
                if matches!(p.status, FramedDecoderStatus::NeedsOutput) {
                    break Ok("NeedsOutput".to_owned());
                }
            }
        }
        "event" => session
            .process(&first, &mut output, DecodeOperation::Process)
            .map(|p| format!("{:?}", p.status)),
        "failed" => session
            .process(
                &[0x5b, 0x2a, 0x4d, 0x19],
                &mut output,
                DecodeOperation::Finish,
            )
            .map(|p| format!("{:?}", p.status)),
        _ => unreachable!(),
    };
    match (ending, result) {
        ("needs input", Ok(status)) => assert_eq!(status, "NeedsInput"),
        ("event", Ok(status)) => assert!(status.starts_with("Event"), "{status}"),
        ("failed", Err(_)) => {}
        ("needs output", Ok(status)) => assert_eq!(status, "NeedsOutput"),
        (ending, other) => panic!("{ending}: {other:?}"),
    }
}

#[test]
fn reinit_starts_an_object_identical_to_a_fresh_borrowed_one() {
    let second = corpus().remove(2);
    for ending in ["finished", "needs input", "needs output", "event", "failed"] {
        for stream in [
            FramedDecodeStreamConfig::default(),
            OutputSize::Exact(21).into(),
        ] {
            let mut session = decoder(Default::default())
                .into_session(Default::default())
                .expect("start");
            end_first_object(&mut session, ending);
            session.reinit(stream).expect("reinit");
            let mut fresh_owner = decoder(Default::default());
            let fresh = fresh_owner.start(stream).expect("start").counters();
            assert_eq!(session.counters(), fresh, "{ending}");
            for (split, widths) in SCHEDULES {
                let expected = fresh_borrowed(Default::default(), stream, &second, split, widths);
                let actual = run(&mut session, &second, split, widths);
                assert_eq!(actual, expected, "{ending}, split {split}");
                assert!(finished(&actual));
                session.reinit(stream).expect("reinit again");
            }
        }
    }
}

#[test]
fn a_rejected_reinit_leaves_a_failed_object_that_a_later_reinit_recovers() {
    let bytes = single(b"abc");
    let config = FramedDecodeConfig::default()
        .with_limits(FramedDecodeLimits::default().with_max_output_bytes(Some(5)));
    let mut session = decoder(config)
        .into_session(Default::default())
        .expect("start");
    let rejected = session.reinit(OutputSize::Exact(6).into());
    let borrowed = decoder(config)
        .start(OutputSize::Exact(6).into())
        .map(|_| ());
    assert_eq!(format!("{rejected:?}"), format!("{borrowed:?}"));
    assert!(rejected.is_err());
    let failure = session
        .process(&bytes, &mut [0; 16], DecodeOperation::Finish)
        .unwrap_err();
    assert!(matches!(failure.error, FramedDecodeError::InvalidState));
    assert_eq!((failure.consumed, failure.produced), (0, 0));

    session.reinit(Default::default()).expect("reinit");
    assert_eq!(
        run(&mut session, &bytes, 3, &[2]),
        fresh_borrowed(config, Default::default(), &bytes, 3, &[2])
    );
    session
        .reinit(OutputSize::Exact(6).into())
        .expect_err("rejected");
    let mut decoder = session.into_framed_decompressor();
    assert!(decoder.decompress(&bytes).is_ok());
}

#[test]
fn reinit_keeps_the_resolver_of_the_session() {
    let fixture = Arc::new(dictionaries());
    let input = fixture.bytes.clone();
    let mut owner = decoder(Default::default());
    let expected = run(
        &mut owner
            .start_with_dictionaries(&*fixture, Default::default())
            .expect("borrowed"),
        &input,
        501,
        &[1, 0, 300],
    );
    assert!(finished(&expected));
    let mut session = decoder(Default::default())
        .into_session_with_dictionaries(Arc::clone(&fixture), Default::default())
        .expect("owned");
    assert_eq!(run(&mut session, &input, 501, &[1, 0, 300]), expected);
    session.reinit(Default::default()).expect("reinit");
    let mut payload = Vec::new();
    let mut remaining = &input[..];
    let mut output = [0; 512];
    loop {
        let progress = session
            .process(remaining, &mut output, DecodeOperation::Finish)
            .expect("decode");
        remaining = &remaining[progress.consumed..];
        match progress.status {
            FramedDecoderStatus::Event(FramedEvent::ResourceData(data)) => {
                payload.extend_from_slice(data.bytes);
            }
            FramedDecoderStatus::Finished => break,
            _ => {}
        }
    }
    assert_eq!(payload, fixture.payloads.concat());
    session.reinit(Default::default()).expect("reinit");
    assert_eq!(run(&mut session, &input, 501, &[1, 0, 300]), expected);
    drop(session);
    assert_eq!(Arc::strong_count(&fixture), 1);
}

fn with_shorthands(session: &mut impl Facade, input: &[u8], shorthand: bool) -> Vec<String> {
    let mut log = Vec::new();
    let mut offset = 0;
    while offset < input.len() {
        let (text, next) = session.record(&input[offset..], &mut [0; 5], DecodeOperation::Process);
        log.push(text);
        offset += next.expect("process").0;
    }
    for operation in [DecodeOperation::Process, DecodeOperation::Finish] {
        loop {
            let (text, next) = if shorthand {
                session.record_shorthand(&mut [0; 5], operation)
            } else {
                session.record(&[], &mut [0; 5], operation)
            };
            let done = text.contains("NeedsInput") || text.contains("Finished") || next.is_none();
            log.push(text);
            if done {
                break;
            }
        }
    }
    log.push(session.counters());
    log
}

#[test]
fn flush_and_finish_are_process_without_input_in_both_session_shapes() {
    // The single-resource profile without its seek table waits for input.
    let mut input = single(b"short payload");
    input.truncate(input.len() - 20);
    for shorthand in [false, true] {
        let mut owner = decoder(Default::default());
        let borrowed = with_shorthands(
            &mut owner.start(Default::default()).expect("start"),
            &input,
            shorthand,
        );
        let mut owned = decoder(Default::default())
            .into_session(Default::default())
            .expect("start");
        assert_eq!(with_shorthands(&mut owned, &input, shorthand), borrowed);
        assert!(
            borrowed
                .iter()
                .any(|line| line.contains("UnexpectedEndOfInput"))
        );
    }
    let mut owner = decoder(Default::default());
    let plain = with_shorthands(
        &mut owner.start(Default::default()).expect("start"),
        &input,
        false,
    );
    let mut owner = decoder(Default::default());
    let short = with_shorthands(
        &mut owner.start(Default::default()).expect("start"),
        &input,
        true,
    );
    assert_eq!(plain, short);
}

#[test]
fn the_framed_shorthands_keep_the_finish_contract() {
    let input = two_resources();
    let mut session = decoder(Default::default())
        .into_session(Default::default())
        .expect("start");
    let mut output = [0; 4];
    session
        .process(&input[..5], &mut output, DecodeOperation::Finish)
        .expect_err("truncated with Finish");
    let failure = session
        .flush(&mut output)
        .expect_err("flush after a failure");
    assert!(matches!(failure.error, FramedDecodeError::InvalidState));
    let mut session = decoder(Default::default())
        .into_session(Default::default())
        .expect("start");
    let progress = session
        .process(&input[..13], &mut output, DecodeOperation::Finish)
        .expect("header under Finish");
    assert_eq!(progress.consumed, 13);
    // The remaining suffix is empty, as `finish` declares.
    let failure = session.finish(&mut output).expect_err("ends early");
    assert!(matches!(
        failure.error,
        FramedDecodeError::UnexpectedEndOfInput
    ));
    let failure = session.flush(&mut output).expect_err("flush after Finish");
    assert!(matches!(failure.error, FramedDecodeError::InvalidState));
}
