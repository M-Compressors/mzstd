use zstd_ffi::*;

fn data(n: usize, seed: u64) -> Vec<u8> {
    let mut x = seed;
    let words: Vec<&[u8]> = vec![
        b"alpha ",
        b"beta ",
        b"gamma ",
        b"delta ",
        b"epsilon ",
        b"zeta ",
        b"eta ",
    ];
    let mut out = Vec::new();
    while out.len() < n {
        x = x
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        out.extend_from_slice(words[(x >> 33) as usize % words.len()]);
        if (x >> 20).is_multiple_of(7) {
            out.push((x >> 40) as u8);
        }
    }
    out.truncate(n);
    out
}

#[test]
fn reused_context_output_versus_fresh() {
    let input = data(300_000, 7);
    for level in [1, 3, 5, 6, 9, 12, 13, 16, 19] {
        let fresh = oracle::compress2(&input, &oracle::Params::level(level)).unwrap();
        unsafe {
            let cctx = ZSTD_createCCtx();
            ZSTD_CCtx_setParameter(cctx, ZSTD_c_compressionLevel, level);
            let mut out = vec![0u8; ZSTD_compressBound(input.len())];
            let mut sizes = Vec::new();
            for _ in 0..3 {
                let n = ZSTD_compress2(
                    cctx,
                    out.as_mut_ptr().cast(),
                    out.len(),
                    input.as_ptr().cast(),
                    input.len(),
                );
                sizes.push((n, out[..n] == fresh[..]));
            }
            ZSTD_freeCCtx(cctx);
            println!("level {level}: fresh {} reused {:?}", fresh.len(), sizes);
        }
    }
}
