import Mzstd.FrameHeader
import Mzstd.FrameHeaderWriter

/-! # Frame header round trip

The encoder's `write_frame_header` and the decoder's `FrameHeader::parse` are
each proved against RFC 8878 section 3.1.1.1 alone. Composed, the decoder reads
back from the encoder's header the window, content size, dictionary ID and
checksum flag the encoder was asked to declare.
-/

open Aeneas Aeneas.Std Result
open Mzstd.Rfc

namespace mzstd

/-- For every header the encoder writes, after the four magic bytes, the
decoder's `FrameHeader::parse` of the header bytes returns `declared`: the
window is the pledged size for a single-segment frame and `2 ^ window_log`
otherwise, the content size is present exactly when the encoder declares it,
and the dictionary ID and checksum flag are the encoder's. -/
theorem frame_header_round_trip (dst : Slice U8)
    (header : compressor.core.cctx.FrameHeaderFields) (hdst : 18 ≤ dst.length)
    (hw : 10 ≤ header.window_log.val) (hw' : header.window_log.val ≤ 31) :
    compressor.core.cctx.write_frame_header dst header ⦃
      (r : core.result.Result Usize compressor.core.error.CoreError × Slice U8) =>
      ∃ pos : Usize, r.1 = .Ok pos ∧
        le ((r.2.val.take 4).map UScalar.val) = magicNumber ∧
        ∀ bytes : Slice U8, bytes.val = (r.2.val.drop 4).take (pos.val - 4) →
          decompressor.core.frame.FrameHeader.parse bytes ⦃
            (p : core.result.Result decompressor.core.frame.FrameHeader
              decompressor.error.InvalidDataKind) =>
            FrameHeaderResult (some (declared header)) p ⦄ ⦄ := by
  apply WP.spec_mono (write_frame_header_rfc dst header hdst hw hw')
  rintro r ⟨pos, hres, -, -, -, -, hmagic, hparse, -⟩
  refine ⟨pos, hres, hmagic, fun bytes hbytes => ?_⟩
  apply WP.spec_mono (FrameHeader.parse.spec bytes)
  intro p hp
  rwa [hbytes, hparse] at hp

end mzstd
