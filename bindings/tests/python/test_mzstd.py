"""Behaviour of the published `mzstd` Python package.

Byte identity with zstd is checked in Rust (`mzstd-bindings/tests`); these
tests check that the binding layer carries every call, default and error
across to Python.
"""

import threading

import pytest

import mzstd

TEXT = b"the quick brown fox jumps over the lazy dog. " * 2000
FRAME_MAGIC = bytes.fromhex("28b52ffd")


def test_one_shot_round_trip() -> None:
    frame = mzstd.compress(TEXT)
    assert frame.startswith(FRAME_MAGIC)
    assert len(frame) < len(TEXT) // 10
    assert mzstd.decompress(frame) == TEXT


@pytest.mark.parametrize("level", [-5, 1, 3, 19, 22])
def test_every_level_round_trips(level: int) -> None:
    assert mzstd.decompress(mzstd.compress(TEXT, level=level)) == TEXT


def test_default_level_is_level_three() -> None:
    assert mzstd.compress(TEXT) == mzstd.compress(TEXT, level=3)
    assert mzstd.compress(TEXT, level=0) == mzstd.compress(TEXT, level=3)


def test_accepts_bytes_like_input() -> None:
    frame = mzstd.compress(bytearray(TEXT))
    assert mzstd.decompress(memoryview(frame)) == TEXT


def test_empty_input() -> None:
    assert mzstd.decompress(mzstd.compress(b"")) == b""
    assert mzstd.decompress(b"") == b""


def test_invalid_level_raises() -> None:
    with pytest.raises(mzstd.InvalidLevelError, match="compression level 23 is outside"):
        mzstd.compress(b"x", level=23)
    assert issubclass(mzstd.InvalidLevelError, mzstd.MzstdError)


def test_corrupt_input_raises() -> None:
    with pytest.raises(mzstd.DecompressionError, match="decompression failed"):
        mzstd.decompress(b"definitely not zstd")


def test_output_cap() -> None:
    frame = mzstd.compress(TEXT)
    assert mzstd.decompress(frame, max_output_size=len(TEXT)) == TEXT
    with pytest.raises(mzstd.DecompressionError):
        mzstd.decompress(frame, max_output_size=len(TEXT) - 1)


def test_dictionary_round_trip() -> None:
    dictionary = b"the quick brown fox jumps over the lazy dog"
    frame = mzstd.compress_with_dictionary(b"the lazy dog", dictionary, level=19)
    assert mzstd.decompress_with_dictionary(frame, dictionary) == b"the lazy dog"
    with pytest.raises(mzstd.MzstdError):
        mzstd.decompress(frame)


def test_malformed_trained_dictionary_raises() -> None:
    with pytest.raises(mzstd.DictionaryError):
        mzstd.compress_with_dictionary(b"x", bytes.fromhex("37a430ec01000000ff"))


def test_compress_bound() -> None:
    assert mzstd.compress_bound(len(TEXT)) >= len(mzstd.compress(TEXT, level=1))
    assert mzstd.compress_bound(0) > 0


def test_streams_round_trip_in_pieces() -> None:
    encoder = mzstd.StreamCompressor.new(level=7)
    frame = b"".join(encoder.compress(TEXT[i : i + 1000]) for i in range(0, len(TEXT), 1000))
    frame += encoder.finish()
    assert not encoder.is_open()
    assert mzstd.decompress(frame) == TEXT

    decoder = mzstd.StreamDecompressor.new()
    data = b"".join(decoder.decompress(frame[i : i + 7]) for i in range(0, len(frame), 7))
    assert data + decoder.finish() == TEXT


def test_flush_makes_input_decodable() -> None:
    encoder = mzstd.StreamCompressor.new()
    frame = encoder.compress(b"first half ") + encoder.flush()
    assert mzstd.StreamDecompressor.new().decompress(frame) == b"first half "


def test_dictionary_streams() -> None:
    dictionary = b"shared history " * 10
    encoder = mzstd.StreamCompressor.with_dictionary(dictionary)
    frame = encoder.compress(b"shared history again") + encoder.finish()
    decoder = mzstd.StreamDecompressor.with_dictionary(dictionary)
    assert decoder.decompress(frame) + decoder.finish() == b"shared history again"


def test_closed_stream_raises() -> None:
    encoder = mzstd.StreamCompressor.new()
    encoder.finish()
    with pytest.raises(mzstd.StreamClosedError):
        encoder.compress(b"x")


def test_truncated_stream_raises_on_finish() -> None:
    frame = mzstd.compress(TEXT)[:-4]
    decoder = mzstd.StreamDecompressor.new()
    decoder.decompress(frame)
    with pytest.raises(mzstd.DecompressionError):
        decoder.finish()


def test_compression_runs_in_parallel_threads() -> None:
    results: list[bytes] = []
    threads = [threading.Thread(target=lambda: results.append(mzstd.compress(TEXT))) for _ in range(4)]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join()
    assert len(set(results)) == 1
    assert mzstd.decompress(results[0]) == TEXT
