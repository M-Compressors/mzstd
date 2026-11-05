// Behaviour of the published `mzstd` (Node.js) and `mzstd-wasm` packages.
//
// Run with MZSTD_MODULE pointing at the package entry point, for example
//   MZSTD_MODULE=../../crates/mzstd-bindings-node/index.js node --test
// Byte identity with zstd is checked in Rust (`mzstd-bindings/tests`); these
// tests check that the binding layer carries every call, default and error.
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { resolve } from "node:path";
import { test } from "node:test";

const target = process.env.MZSTD_MODULE;
assert.ok(target, "set MZSTD_MODULE to the package entry point");
const mzstd = createRequire(import.meta.url)(resolve(target));

const TEXT = new TextEncoder().encode("the quick brown fox jumps over the lazy dog. ".repeat(2000));
const bytes = (value) => new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
const concat = (parts) => bytes(Buffer.concat(parts.map((part) => Buffer.from(bytes(part)))));

test("one-shot round trip", () => {
  const frame = mzstd.compress(TEXT);
  assert.deepEqual([...bytes(frame).subarray(0, 4)], [0x28, 0xb5, 0x2f, 0xfd]);
  assert.ok(frame.length < TEXT.length / 10);
  assert.deepEqual(bytes(mzstd.decompress(frame)), TEXT);
});

test("every level round trips", () => {
  for (const level of [-5, 1, 3, 19, 22]) {
    assert.deepEqual(bytes(mzstd.decompress(mzstd.compress(TEXT, level))), TEXT);
  }
});

test("default level is level three", () => {
  assert.deepEqual(bytes(mzstd.compress(TEXT)), bytes(mzstd.compress(TEXT, 3)));
  assert.deepEqual(bytes(mzstd.compress(TEXT, null)), bytes(mzstd.compress(TEXT, 3)));
});

test("empty input", () => {
  assert.equal(mzstd.decompress(mzstd.compress(new Uint8Array())).length, 0);
  assert.equal(mzstd.decompress(new Uint8Array()).length, 0);
});

test("invalid level throws", () => {
  assert.throws(() => mzstd.compress(TEXT, 23), /compression level 23 is outside/);
});

test("corrupt input throws", () => {
  assert.throws(() => mzstd.decompress(new TextEncoder().encode("not zstd")), /decompression failed/);
});

test("output cap", () => {
  const frame = mzstd.compress(TEXT);
  assert.deepEqual(bytes(mzstd.decompress(frame, TEXT.length)), TEXT);
  assert.throws(() => mzstd.decompress(frame, TEXT.length - 1), /decompression failed/);
});

test("dictionary round trip", () => {
  const dictionary = new TextEncoder().encode("the quick brown fox jumps over the lazy dog");
  const data = new TextEncoder().encode("the lazy dog");
  const frame = mzstd.compressWithDictionary(data, dictionary, 19);
  assert.deepEqual(bytes(mzstd.decompressWithDictionary(frame, dictionary)), data);
  assert.throws(() => mzstd.compressWithDictionary(data, Uint8Array.of(0x37, 0xa4, 0x30, 0xec, 1, 0, 0, 0, 0xff)), /invalid dictionary/);
});

test("compress bound", () => {
  assert.ok(mzstd.compressBound(TEXT.length) >= mzstd.compress(TEXT, 1).length);
});

test("streams round trip in pieces", () => {
  const encoder = mzstd.StreamCompressor.new(7);
  const parts = [];
  for (let i = 0; i < TEXT.length; i += 1000) parts.push(encoder.compress(TEXT.subarray(i, i + 1000)));
  parts.push(encoder.finish());
  assert.equal(encoder.isOpen(), false);
  const frame = concat(parts);
  assert.deepEqual(bytes(mzstd.decompress(frame)), TEXT);

  const decoder = mzstd.StreamDecompressor.new();
  const out = [];
  for (let i = 0; i < frame.length; i += 7) out.push(decoder.decompress(frame.subarray(i, i + 7)));
  out.push(decoder.finish());
  assert.deepEqual(concat(out), TEXT);
});

test("flush makes input decodable", () => {
  const encoder = mzstd.StreamCompressor.new();
  const head = new TextEncoder().encode("first half ");
  const frame = concat([encoder.compress(head), encoder.flush()]);
  assert.deepEqual(bytes(mzstd.StreamDecompressor.new().decompress(frame)), head);
});

test("dictionary streams", () => {
  const dictionary = new TextEncoder().encode("shared history ".repeat(10));
  const data = new TextEncoder().encode("shared history again");
  const encoder = mzstd.StreamCompressor.withDictionary(dictionary);
  const frame = concat([encoder.compress(data), encoder.finish()]);
  const decoder = mzstd.StreamDecompressor.withDictionary(dictionary);
  assert.deepEqual(concat([decoder.decompress(frame), decoder.finish()]), data);
});

test("closed stream throws", () => {
  const encoder = mzstd.StreamCompressor.new();
  encoder.finish();
  assert.throws(() => encoder.compress(TEXT), /the stream is finished or failed/);
});

test("truncated stream throws on finish", () => {
  const frame = bytes(mzstd.compress(TEXT));
  const decoder = mzstd.StreamDecompressor.new();
  decoder.decompress(frame.subarray(0, frame.length - 4));
  assert.throws(() => decoder.finish(), /decompression failed/);
});
