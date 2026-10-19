use std::env;
use std::fs;
use std::path::PathBuf;

const LIB: &str = "vendor/zstd/lib";
const SEEKABLE: &str = "vendor/zstd/contrib/seekable_format";

fn c_sources(directory: &str) -> Vec<PathBuf> {
    let mut sources = fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("failed to read {directory}: {error}"))
        .map(|entry| entry.expect("failed to read vendored source entry").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "c"))
        .collect::<Vec<_>>();
    sources.sort();
    sources
}

/// Applies the definitions every translation unit of the oracle shares.
///
/// The reference is compiled single-threaded, without legacy formats and
/// without its x86-64 Huffman decoder assembly, which `cc` cannot assemble
/// portably. None of these changes compressed bytes: multithreading is opt-in
/// per context, legacy support only widens what the decoder accepts, and the
/// assembly decoder is an alternative to the C one with identical results.
fn configure(build: &mut cc::Build) {
    build
        .include(LIB)
        .include(format!("{LIB}/common"))
        .define("ZSTD_LEGACY_SUPPORT", "0")
        .define("ZSTD_DISABLE_ASM", None)
        .define("XXH_NAMESPACE", "ZSTD_")
        .cargo_metadata(false)
        .warnings(false);
    // Runs index overflow correction as early as the reference allows rather
    // than after gigabytes of input. mzstd reads the same variable, so both
    // sides of a differential test are built alike.
    if env::var_os("MZSTD_OVERFLOW_CORRECT_FREQUENTLY").is_some() {
        build.define("ZSTD_WINDOW_OVERFLOW_CORRECT_FREQUENTLY", "1");
    }
}

fn main() {
    let output_directory = env::var("OUT_DIR").expect("OUT_DIR is not set");

    let mut sources = Vec::new();
    for directory in ["common", "compress", "decompress", "dictBuilder"] {
        sources.extend(c_sources(&format!("{LIB}/{directory}")));
    }
    let mut library = cc::Build::new();
    configure(&mut library);
    library.files(&sources).compile("zstd");

    // The contrib seekable format is the oracle and baseline for framed
    // containers.
    let mut seekable = cc::Build::new();
    configure(&mut seekable);
    seekable
        .include(SEEKABLE)
        .file(format!("{SEEKABLE}/zstdseek_compress.c"))
        .file(format!("{SEEKABLE}/zstdseek_decompress.c"))
        .compile("zstd_seekable");

    let mut shim = cc::Build::new();
    configure(&mut shim);
    shim.file("shim/zstd_shim.c").compile("zstd_shim");

    // Dependents precede their dependencies for single-pass static linkers.
    println!("cargo:rustc-link-search=native={output_directory}");
    println!("cargo:rustc-link-lib=static=zstd_shim");
    println!("cargo:rustc-link-lib=static=zstd_seekable");
    println!("cargo:rustc-link-lib=static=zstd");

    for source in sources {
        println!("cargo:rerun-if-changed={}", source.display());
    }
    println!("cargo:rerun-if-changed=shim");
    println!("cargo:rerun-if-changed={SEEKABLE}");
    println!("cargo:rerun-if-changed={LIB}/zstd.h");
    println!("cargo:rerun-if-changed=UPSTREAM_VERSION");
    println!("cargo:rerun-if-env-changed=MZSTD_OVERFLOW_CORRECT_FREQUENTLY");
}
