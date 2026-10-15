use zstd_ffi::oracle;

#[test]
fn oracle_round_trips_through_every_entry_point() {
    let input = b"oracle oracle oracle oracle".repeat(64);
    let one_shot = oracle::compress(&input, 3);
    let params = oracle::Params::level(3);
    assert_eq!(oracle::compress2(&input, &params).unwrap(), one_shot);
    let streamed = oracle::compress_stream(&params, &[(&input, oracle::Directive::End)]).unwrap();
    assert_eq!(streamed, one_shot);
    assert_eq!(oracle::decompress(&one_shot, input.len()).unwrap(), input);
    assert_eq!(
        oracle::decompress_stream(&one_shot, 7, None).unwrap(),
        input
    );
    assert_eq!(unsafe { zstd_ffi::ZSTD_versionNumber() }, 10507);
}

#[test]
fn shim_reports_applied_parameters_and_entropy_tables() {
    let (_, applied) =
        oracle::compress2_with_params(&[7u8; 1 << 20], &oracle::Params::level(22)).unwrap();
    assert_eq!(applied.cparams.strategy, zstd_ffi::ZSTD_btultra2);
    // The window shrinks to the 1 MiB input, below the 2^27 LDM threshold
    // (`ZSTD_ps_disable` is 2).
    assert_eq!(applied.cparams.windowLog, 20);
    assert_eq!(applied.enable_ldm, 2);
    let count = [10u32, 0, 3, 1, 1];
    let (normalized, log) = oracle::fse_normalize_count(&count, 15, 5, true).unwrap();
    assert_eq!(log, 5);
    assert_eq!(
        normalized.iter().map(|&n| i32::from(n).abs()).sum::<i32>(),
        32
    );
    let table = oracle::huf_build(&[5, 1, 1, 1], 11).unwrap();
    assert_eq!(table.nb_bits[0], 1);
}

#[test]
fn trained_dictionaries_round_trip_and_shrink_small_records() {
    let records = zstd_ffi::corpus::records(2000, 1);
    let samples: Vec<&[u8]> = records.iter().map(Vec::as_slice).collect();
    let dictionary = zstd_ffi::oracle::train_dictionary(&samples, 16 << 10).expect("trains");
    assert_ne!(zstd_ffi::oracle::dict_id_from_dict(&dictionary), 0);
    let dict = zstd_ffi::oracle::Dictionary::auto(&dictionary);
    let params = zstd_ffi::oracle::Params::level(3).with_dictionary(dict.clone());
    let record = &records[1999];
    let with = zstd_ffi::oracle::compress2(record, &params).expect("compresses");
    let without =
        zstd_ffi::oracle::compress2(record, &zstd_ffi::oracle::Params::level(3)).expect("plain");
    assert!(with.len() < without.len());
    assert_eq!(
        zstd_ffi::oracle::dict_id_from_frame(&with),
        zstd_ffi::oracle::dict_id_from_dict(&dictionary)
    );
    let restored = zstd_ffi::oracle::decompress_with_dictionary(&with, record.len(), &dict)
        .expect("decompresses");
    assert_eq!(&restored, record);
    let streamed = zstd_ffi::oracle::decompress_stream_with_dictionary(&with, 3, None, Some(&dict))
        .expect("streams");
    assert_eq!(&streamed, record);
    zstd_ffi::oracle::load_decoder_dictionary(&dict).expect("loads");
    let truncated = zstd_ffi::oracle::Dictionary::auto(&dictionary[..12]);
    assert!(zstd_ffi::oracle::load_decoder_dictionary(&truncated).is_err());
}

#[test]
fn adjacent_prefixes_shrink_the_input_after_them() {
    let whole = zstd_ffi::corpus::text(200_000, 3);
    let params = zstd_ffi::oracle::Params::level(3);
    let with = zstd_ffi::oracle::compress2_with_prefix(&whole, 100_000, &params).expect("prefix");
    let without = zstd_ffi::oracle::compress2(&whole[100_000..], &params).expect("plain");
    assert!(with.len() < without.len());
}
