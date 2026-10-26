import Mzstd.Basic

/-! # The encoder's frame header

`write_frame_header` (`compressor/core/cctx.rs`, the reference's
`ZSTD_writeFrameHeader`) writes the magic number and a frame header. This file
proves, against the RFC 8878 transcription in `Mzstd.Rfc` only, that the bytes
it writes are a frame header the RFC reads back as the fields the encoder was
asked to declare: the window (`2 ^ window_log`, or the content size in a
single-segment frame), the content size when declared, the dictionary ID unless
left out, and the checksum flag.

The proof first describes the written bytes with a pure model, `headerBytes`,
then reads that model with `Mzstd.Rfc.parseFrameHeader`.
-/

open Aeneas Aeneas.Std Result
open Mzstd.Rfc

namespace mzstd

/-! ## Little-endian fields -/

/-- The `n` low bytes of `v`, least significant first. -/
def leBytes (n v : Nat) : List Nat := (List.range n).map (fun i => v / 256 ^ i % 256)

@[simp] theorem leBytes_length (n v : Nat) : (leBytes n v).length = n := by simp [leBytes]

/-- Reading `n` written bytes gives the value modulo `256 ^ n`. -/
theorem le_leBytes (n v : Nat) : le (leBytes n v) = v % 256 ^ n := by
  induction n generalizing v with
  | zero => simp [leBytes, le, Nat.mod_one]
  | succ n ih =>
    simp only [leBytes, List.range_succ_eq_map, List.map_cons, List.map_map, le]
    have : (fun i => v / 256 ^ i % 256) ∘ Nat.succ = fun i => v / 256 / 256 ^ i % 256 := by
      funext i; simp [Function.comp, pow_succ, Nat.div_div_eq_div_mul, Nat.mul_comm]
    rw [this]
    have h := ih (v / 256)
    simp only [leBytes] at h
    rw [h, pow_succ, Nat.mul_comm (256 ^ n) 256, Nat.mod_mul]
    simp

/-- `toLEBytes` of a whole number of bytes, as numbers, is `leBytes`. -/
theorem toLEBytes_map (w : Nat) (b : BitVec w) (hw : w % 8 = 0) :
    b.toLEBytes.map BitVec.toNat = leBytes (w / 8) b.toNat := by
  apply List.ext_getElem
  · simp; omega
  · intro i h1 h2
    simp only [List.getElem_map, leBytes, List.getElem_range]
    have hi : i < (w + 7) / 8 := by simpa using h1
    have := toLEBytes_toNat w b i hi
    rw [← this, getElem!_pos b.toLEBytes i (by simpa using hi)]

/-! ## The bytes the encoder writes -/

/-- `Dictionary_ID_Flag`: no field when the ID is left out or zero, else the
smallest of the 1-, 2- and 4-byte fields that holds it. -/
def didCode (noDictId : Bool) (dictId : Nat) : Nat :=
  if noDictId then 0
  else (if dictId > 0 then 1 else 0) + (if dictId ≥ 256 then 1 else 0) + (if dictId ≥ 65536 then 1 else 0)

/-- `Single_Segment_Flag`: the content size is declared and the window covers it. -/
def singleOf (contentSize : Bool) (windowLog pledged : Nat) : Bool :=
  contentSize && decide (pledged ≤ 2 ^ windowLog)

/-- `Frame_Content_Size_Flag`: 0 for none (or a 1-byte single-segment size),
1 for the 2-byte form from 256, 2 for 4 bytes from 65792, 3 for 8 bytes from
`0xFFFFFFFF`. -/
def fcsCode (contentSize : Bool) (pledged : Nat) : Nat :=
  if contentSize then
    (if pledged ≥ 256 then 1 else 0) + (if pledged ≥ 65792 then 1 else 0) +
      (if pledged ≥ 0xFFFFFFFF then 1 else 0)
  else 0

/-- The `Frame_Content_Size` field for `fcsCode` `c`. -/
def fcsBytes (c : Nat) (single : Bool) (pledged : Nat) : List Nat :=
  match c with
  | 0 => if single then leBytes 1 pledged else []
  | 1 => leBytes 2 (pledged - 256)
  | 2 => leBytes 4 pledged
  | _ => leBytes 8 pledged

/-- The frame header the encoder writes after the magic number. -/
def headerBytes (h : compressor.core.cctx.FrameHeaderFields) : List Nat :=
  let single := singleOf h.content_size h.window_log.val h.pledged.val
  let dc := didCode h.no_dict_id h.dict_id.val
  let fc := fcsCode h.content_size h.pledged.val
  [dc + 4 * h.checksum.toNat + 32 * single.toNat + 64 * fc] ++
    (if single then [] else [(h.window_log.val - 10) * 8]) ++
    leBytes (didFieldSize dc) h.dict_id.val ++ fcsBytes fc single h.pledged.val

/-- What the header declares, in the RFC's terms. -/
def declared (h : compressor.core.cctx.FrameHeaderFields) : FrameHeader where
  windowSize :=
    if singleOf h.content_size h.window_log.val h.pledged.val then h.pledged.val
    else 2 ^ h.window_log.val
  contentSize := if h.content_size then some h.pledged.val else none
  dictionaryId := if h.no_dict_id then 0 else h.dict_id.val
  checksum := h.checksum

/-- `Dictionary_ID_Flag` is a two-bit field. -/
theorem didCode_le (n : Bool) (d : Nat) : didCode n d ≤ 3 := by
  unfold didCode; split_ifs <;> omega

/-- `Frame_Content_Size_Flag` is a two-bit field. -/
theorem fcsCode_le (c : Bool) (p : Nat) : fcsCode c p ≤ 3 := by
  unfold fcsCode; split_ifs <;> omega

/-- The dictionary ID field reads back as the declared ID. -/
theorem le_didBytes (n : Bool) (d : Nat) (hd : d < 2 ^ 32) :
    le (leBytes (didFieldSize (didCode n d)) d) = if n then 0 else d := by
  rw [le_leBytes]
  unfold didCode
  by_cases hn : n
  · simp [hn, didFieldSize, Nat.mod_one]
  · simp only [hn, Bool.false_eq_true, ite_false]
    by_cases h0 : d > 0 <;> by_cases h1 : d ≥ 256 <;> by_cases h2 : d ≥ 65536 <;>
      simp [h0, h1, h2, didFieldSize] <;> omega

/-- The header's length, as the RFC computes it from the descriptor. -/
theorem headerBytes_length (h : compressor.core.cctx.FrameHeaderFields) :
    (headerBytes h).length =
      1 + (if singleOf h.content_size h.window_log.val h.pledged.val then 0 else 1) +
        didFieldSize (didCode h.no_dict_id h.dict_id.val) +
        fcsFieldSize (fcsCode h.content_size h.pledged.val)
          (singleOf h.content_size h.window_log.val h.pledged.val) := by
  simp only [headerBytes, List.length_append, List.length_cons, List.length_nil, leBytes_length]
  have := fcsCode_le h.content_size h.pledged.val
  generalize fcsCode h.content_size h.pledged.val = c at *
  generalize singleOf h.content_size h.window_log.val h.pledged.val = s
  obtain rfl | rfl | rfl | rfl : c = 0 ∨ c = 1 ∨ c = 2 ∨ c = 3 := (by omega)
  all_goals (cases s <;> simp [fcsBytes, fcsFieldSize])

/-- `Mzstd.Rfc.parseFrameHeader` of a header laid out as the RFC describes it:
a descriptor whose flags announce the window byte `W` (unless single
segment), the dictionary ID field `D` and the content size field `F`. -/
theorem parse_layout (d : Nat) (W D F : List Nat) (hres : reservedBit d = 0)
    (hW : W.length = windowDescriptorSize (singleSegment d))
    (hD : D.length = didFieldSize (didFlag d))
    (hF : F.length = fcsFieldSize (fcsFlag d) (singleSegment d)) :
    parseFrameHeader ([d] ++ W ++ D ++ F) = some {
      windowSize := if singleSegment d then (if F = [] then none else some (frameContentSize F)).getD 0
        else windowSize (W.getD 0 0)
      contentSize := if F = [] then none else some (frameContentSize F)
      dictionaryId := le D
      checksum := checksumFlag d } := by
  have hlen : ([d] ++ W ++ D ++ F).length = headerSize d := by
    simp [headerSize, hW, hD, hF]; omega
  simp only [List.cons_append, List.nil_append, List.append_assoc] at hlen ⊢
  unfold parseFrameHeader
  dsimp only
  split
  · rename_i hc; rw [hlen, hres] at hc; simp at hc
  have hdrop1 : (d :: (W ++ (D ++ F))).drop (1 + windowDescriptorSize (singleSegment d)) = D ++ F := by
    rw [Nat.add_comm, List.drop_succ_cons, ← hW]; simp
  have hdrop2 : (d :: (W ++ (D ++ F))).drop
      (1 + windowDescriptorSize (singleSegment d) + didFieldSize (didFlag d)) = F := by
    rw [← List.drop_drop, hdrop1, ← hD]; simp
  rw [hdrop1, hdrop2, ← hD, List.take_left]
  cases hs : singleSegment d
  · have : W.length = 1 := by rw [hW]; simp [windowDescriptorSize, hs]
    obtain ⟨w, rfl⟩ : ∃ w, W = [w] := List.length_eq_one_iff.mp this
    simp
  · simp

/-- **The RFC reads the encoder's header back.** For a window of `2 ^ 10` to
`2 ^ 31` bytes (the encoder's limits), `Mzstd.Rfc.parseFrameHeader` of
`headerBytes` gives exactly the declared fields. -/
theorem parse_headerBytes (h : compressor.core.cctx.FrameHeaderFields)
    (hw : 10 ≤ h.window_log.val) (hw' : h.window_log.val ≤ 31) :
    parseFrameHeader (headerBytes h) = some (declared h) := by
  have hdc := didCode_le h.no_dict_id h.dict_id.val
  have hfc := fcsCode_le h.content_size h.pledged.val
  have hid : h.dict_id.val < 2 ^ 32 := by scalar_tac
  have hp : h.pledged.val < 2 ^ 64 := by scalar_tac
  have hwin : windowSize ((h.window_log.val - 10) * 8) = 2 ^ h.window_log.val := by
    simp only [windowSize]
    rw [show (h.window_log.val - 10) * 8 / 8 = h.window_log.val - 10 by omega,
      show (h.window_log.val - 10) * 8 % 8 = 0 by omega, Nat.mul_zero, Nat.add_zero]
    congr 1; omega
  have h1024 : 1024 ≤ 2 ^ h.window_log.val := by
    calc 1024 = 2 ^ 10 := by norm_num
      _ ≤ _ := Nat.pow_le_pow_right (by decide) hw
  simp only [headerBytes, declared]
  generalize hs : singleOf h.content_size h.window_log.val h.pledged.val = s
  generalize hfcd : fcsCode h.content_size h.pledged.val = fc at hfc
  set dc := didCode h.no_dict_id h.dict_id.val with hdcd
  have hcs : h.checksum.toNat ≤ 1 := Bool.toNat_le _
  have hss : s.toNat ≤ 1 := Bool.toNat_le _
  have hres : reservedBit (dc + 4 * h.checksum.toNat + 32 * s.toNat + 64 * fc) = 0 := by
    simp only [reservedBit]; omega
  have hsb : singleSegment (dc + 4 * h.checksum.toNat + 32 * s.toNat + 64 * fc) = s := by
    cases s <;> simp [singleSegment] <;> omega
  have hdf : didFlag (dc + 4 * h.checksum.toNat + 32 * s.toNat + 64 * fc) = dc := by
    simp only [didFlag]; omega
  have hff : fcsFlag (dc + 4 * h.checksum.toNat + 32 * s.toNat + 64 * fc) = fc := by
    simp only [fcsFlag]; omega
  have hcf : checksumFlag (dc + 4 * h.checksum.toNat + 32 * s.toNat + 64 * fc) = h.checksum := by
    cases hc : h.checksum <;> simp [checksumFlag, hc] at hcs ⊢ <;> omega
  have hFlen : (fcsBytes fc s h.pledged.val).length = fcsFieldSize fc s := by
    obtain rfl | rfl | rfl | rfl : fc = 0 ∨ fc = 1 ∨ fc = 2 ∨ fc = 3 := (by omega)
    all_goals (cases s <;> simp [fcsBytes, fcsFieldSize])
  rw [parse_layout _ _ _ _ hres (by rw [hsb]; cases s <;> simp [windowDescriptorSize])
    (by rw [hdf]; simp) (by rw [hff, hsb, hFlen]), hsb, hcf, le_didBytes _ _ hid]
  simp only [Option.some.injEq, FrameHeader.mk.injEq, and_true]
  -- The content size field and the window, by the size class of `pledged`.
  simp only [fcsCode] at hfcd
  simp only [singleOf] at hs
  cases hcsz : h.content_size <;> simp only [hcsz, Bool.false_and, Bool.true_and] at hfcd hs
  · subst hfcd; subst hs
    simp [fcsBytes, hwin]
  · by_cases h0 : h.pledged.val ≥ 256 <;> by_cases h1 : h.pledged.val ≥ 65792 <;>
      by_cases h2 : h.pledged.val ≥ 0xFFFFFFFF <;>
      simp only [h0, h1, h2, ite_true, ite_false] at hfcd <;>
      (try omega) <;> subst hfcd <;>
      (have hsv : s = decide (h.pledged.val ≤ 2 ^ h.window_log.val) := hs.symm) <;>
      simp only [fcsBytes, frameContentSize, le_leBytes, leBytes_length] <;>
      (cases s <;> simp [leBytes, hwin, le] at hsv ⊢ <;> omega)

/-- Only the `n` low bytes of a value reach `leBytes n`. -/
theorem leBytes_mod (n v : Nat) : leBytes n (v % 256 ^ n) = leBytes n v := by
  apply List.ext_getElem (by simp)
  intro i h1 _
  simp only [leBytes, List.getElem_map, List.getElem_range]
  simp only [leBytes_length] at h1
  have : 256 ^ n = 256 ^ i * 256 ^ (n - i) := by rw [← pow_add]; congr 1; omega
  rw [this, Nat.mod_mul_right_div_self, Nat.mod_mod_of_dvd]
  exact dvd_pow_self 256 (by omega)

/-! ## Writing into the destination -/

/-- Overwriting commutes with reading the bytes' values. -/
theorem map_setSlice {α β : Type} (f : α → β) (l : List α) (i : Nat) (s : List α) :
    (l.setSlice! i s).map f = (l.map f).setSlice! i (s.map f) := by
  simp [List.setSlice!, List.map_take, List.map_drop]

/-- Overwriting just past a prefix appends to it. -/
theorem setSlice_append {α : Type} (B R S : List α) (h : S.length ≤ R.length) :
    (B ++ R).setSlice! B.length S = B ++ S ++ R.drop S.length := by
  simp only [List.setSlice!, List.take_left, List.length_append]
  rw [show min S.length (B.length + R.length - B.length) = S.length by omega,
    List.take_of_length_le (le_refl _), List.drop_append, Nat.add_sub_cancel_left,
    List.drop_eq_nil_of_le (by omega : B.length ≤ B.length + S.length)]
  simp

/-- `x` holds `bytes` at its start, then what `dst` held past them. -/
def Written (dst x : Slice U8) (bytes : List Nat) : Prop :=
  x.val.map UScalar.val = bytes ++ (dst.val.drop bytes.length).map UScalar.val ∧
    x.length = dst.length

theorem Written.setSlice {dst x : Slice U8} {B : List Nat} (h : Written dst x B) (S : List U8)
    (hlen : B.length + S.length ≤ dst.length) :
    Written dst (x.setSlice! B.length S) (B ++ S.map UScalar.val) := by
  obtain ⟨hx, hl⟩ := h
  refine ⟨?_, by simp [hl]⟩
  simp only [Slice.setSlice!_val, map_setSlice, hx]
  rw [setSlice_append _ _ _ (by simp [Slice.length] at hlen ⊢; omega)]
  simp [List.drop_drop, List.map_drop]

theorem Written.set {dst x : Slice U8} {B : List Nat} (h : Written dst x B) (v : U8) (i : Usize)
    (hi : i.val = B.length) (hlen : B.length + 1 ≤ dst.length) :
    Written dst (x.set i v) (B ++ [v.val]) := by
  have hs := h.setSlice [v] (by simpa using hlen)
  have : x.set i v = x.setSlice! B.length [v] := by
    rw [Slice.eq_iff]
    simp only [Slice.set_val_eq, Slice.setSlice!_val, hi]
    have hxl : B.length < x.val.length := by
      have := h.2; simp [Slice.length] at this hlen; omega
    simp only [List.setSlice!, List.length_singleton]
    rw [show min 1 (x.val.length - B.length) = 1 by omega]
    rw [List.set_eq_take_append_cons_drop]
    simp only [hxl, ite_true]
    simp
  rw [this]; simpa using hs

/-- `to_le_bytes` of a 16-, 32- or 64-bit value, as written bytes. -/
theorem toLEBytes_vals {w : Nat} (b : BitVec w) (hw : w % 8 = 0) :
    (b.toLEBytes.map (@UScalar.mk UScalarTy.U8)).map UScalar.val = leBytes (w / 8) b.toNat := by
  rw [List.map_map]
  exact toLEBytes_map w b hw

/-- `u8::from(bool)`. -/
@[step]
theorem FromU8Bool.from.spec (b : Bool) :
    lift (core.convert.num.FromU8Bool.from b) ⦃ (x : U8) => x.val = b.toNat ⦄ := by
  cases b <;> simp [lift, core.convert.num.FromU8Bool.from]

-- The generated function is one long body with four nested case splits.
set_option maxHeartbeats 4000000 in
/-- **Spec theorem for `mzstd::compressor::core::cctx::write_frame_header`**
With room for the largest header and a window of `2 ^ 10` to `2 ^ 31` bytes,
the encoder writes the magic number, little-endian, and then `headerBytes`,
leaves the rest of `dst` as it was and returns the end of the header. -/
@[step]
theorem compressor.core.cctx.write_frame_header.spec (dst : Slice U8)
    (header : compressor.core.cctx.FrameHeaderFields) (hdst : 18 ≤ dst.length)
    (hw : 10 ≤ header.window_log.val) (hw' : header.window_log.val ≤ 31) :
    compressor.core.cctx.write_frame_header dst header ⦃
      (r : core.result.Result Usize compressor.core.error.CoreError × Slice U8) =>
      ∃ pos : Usize, r.1 = .Ok pos ∧ pos.val = 4 + (headerBytes header).length ∧
        Written dst r.2 (leBytes 4 magicNumber ++ headerBytes header) ⦄ := by
  have hdc := didCode_le header.no_dict_id header.dict_id.val
  have hfc := fcsCode_le header.content_size header.pledged.val
  unfold compressor.core.cctx.write_frame_header
  -- The descriptor's fields.
  have hdcs : (if header.no_dict_id then ok 0#u8 else do
      let i ← lift (core.convert.num.FromU8Bool.from (header.dict_id > 0#u32))
      let i1 ← lift (core.convert.num.FromU8Bool.from (header.dict_id >= 256#u32))
      let i2 ← i + i1
      let i3 ← lift (core.convert.num.FromU8Bool.from (header.dict_id >= 65536#u32))
      i2 + i3) ⦃ (c : U8) => c.val = didCode header.no_dict_id header.dict_id.val ⦄ := by
    split
    · simp [didCode, *]
    · have hb : ∀ b : Bool, b.toNat ≤ 1 := Bool.toNat_le
      step as ⟨ i, hi ⟩
      step as ⟨ i1, hi1 ⟩
      step as ⟨ i2, hi2 ⟩
      step as ⟨ i3, hi3 ⟩
      step as ⟨ c, hc ⟩
      rw [hc, hi2, hi, hi1, hi3]
      rename_i hn
      simp only [didCode, hn, Bool.false_eq_true, ite_false]
      by_cases h0 : header.dict_id.val > 0 <;> by_cases h1 : header.dict_id.val ≥ 256 <;>
        by_cases h2 : header.dict_id.val ≥ 65536 <;> simp [h0, h1, h2, UScalar.lt_equiv, UScalar.le_equiv]
  apply WP.spec_bind hdcs
  intro dc hdcv
  step as ⟨ cs, cs_post ⟩
  step as ⟨ window_size, ws_post ⟩
  have hpow : 2 ^ header.window_log.val ≤ 2 ^ 31 := Nat.pow_le_pow_right (by decide) hw'
  have hws : window_size.val = 2 ^ header.window_log.val := by
    rw [ws_post, Nat.one_shiftLeft, Nat.mod_eq_of_lt (by scalar_tac)]
  have hsss : (if header.content_size = true then ok (decide (window_size ≥ header.pledged)) else ok false)
      ⦃ (b : Bool) => b = singleOf header.content_size header.window_log.val header.pledged.val ⦄ := by
    split <;> simp_all [singleOf, UScalar.le_equiv]
  apply WP.spec_bind hsss
  intro single hsv
  step as ⟨ i, i_post ⟩
  step as ⟨ i1, i1_post ⟩
  step as ⟨ wbyte, wbyte_post ⟩
  have hwb : wbyte.val = (header.window_log.val - 10) * 8 := by
    rw [wbyte_post, UScalar.cast_val_eq, i1_post, i_post, Nat.shiftLeft_eq]
    simp only [shared.format.WINDOW_LOG_ABSOLUTE_MIN_val]
    rw [Nat.mod_eq_of_lt (by scalar_tac), Nat.mod_eq_of_lt (by scalar_tac)]
  have hfcs : (if header.content_size = true then do
      let i2 ← lift (core.convert.num.FromU8Bool.from (decide (header.pledged ≥ 256#u64)))
      let i3 ← 65536#u64 + 256#u64
      let i4 ← lift (core.convert.num.FromU8Bool.from (decide (header.pledged ≥ i3)))
      let i5 ← i2 + i4
      let i6 ← lift (core.convert.num.FromU8Bool.from (decide (header.pledged ≥ 4294967295#u64)))
      i5 + i6 else ok 0#u8) ⦃ (c : U8) => c.val = fcsCode header.content_size header.pledged.val ⦄ := by
    split
    · have hb : ∀ b : Bool, b.toNat ≤ 1 := Bool.toNat_le
      step as ⟨ i, hi ⟩
      step as ⟨ i3, hi3 ⟩
      step as ⟨ i4, hi4 ⟩
      step as ⟨ i5, hi5 ⟩
      step as ⟨ i6, hi6 ⟩
      step as ⟨ c, hc ⟩
      rw [hc, hi5, hi, hi4, hi6]
      rename_i hn
      simp only [fcsCode, hn, ite_true]
      by_cases h0 : header.pledged.val ≥ 256 <;> by_cases h1 : header.pledged.val ≥ 65792 <;>
        by_cases h2 : header.pledged.val ≥ 0xFFFFFFFF <;>
        simp [h0, h1, h2, UScalar.le_equiv, hi3]
    · simp_all [fcsCode]
  apply WP.spec_bind hfcs
  intro fc hfcv
  step as ⟨ i2, i2_post ⟩
  step as ⟨ i3, i3_post ⟩
  step as ⟨ i4, i4_post ⟩
  step as ⟨ i5, i5_post ⟩
  have hcs1 : cs.val ≤ 1 := by rw [cs_post]; exact Bool.toNat_le _
  have hs1 : single.toNat ≤ 1 := Bool.toNat_le _
  have hi2 : i2.val = 4 * header.checksum.toNat := by
    rw [i2_post, Nat.shiftLeft_eq, U8.size, Nat.mod_eq_of_lt (by simp [U8.numBits]; omega), cs_post]; ring
  have hi5 : i5.val = 32 * single.toNat := by
    rw [i5_post, Nat.shiftLeft_eq, U8.size, Nat.mod_eq_of_lt (by simp [U8.numBits]; omega), i4_post]; ring
  step as ⟨ i6, i6_post ⟩
  step as ⟨ i7, i7_post ⟩
  have hi7 : i7.val = 64 * fc.val := by
    rw [i7_post, Nat.shiftLeft_eq, U8.size, Nat.mod_eq_of_lt (by simp [U8.numBits]; omega)]; ring
  step as ⟨ descriptor, d_post ⟩
  have hd : descriptor.val = didCode header.no_dict_id header.dict_id.val + 4 * header.checksum.toNat +
      32 * single.toNat + 64 * fcsCode header.content_size header.pledged.val := by
    rw [d_post, i6_post, i3_post, hi2, hi5, hi7, hdcv, hfcv]
  have hlen18 : 18 ≤ dst.val.length := by simpa [Slice.length] using hdst
  split
  · rename_i hlt; simp [Slice.len] at hlt; omega
  step as ⟨ s, back, s_val, s_len, s_back ⟩
  step as ⟨ a, a_post ⟩
  step as ⟨ s1, s1_post ⟩
  step as ⟨ s2, s2_post ⟩
  -- The magic number.
  have W1 : Written dst (back s2) (leBytes 4 magicNumber) := by
    have h0 : Written dst dst [] := ⟨by simp, rfl⟩
    have := h0.setSlice s2.val (by simp [s2_post, s1_post]; scalar_tac)
    have heq : back s2 = dst.setSlice! 0 s2.val := by rw [Slice.eq_iff]; simp [s_back]
    rw [heq]
    convert this using 2
    rw [s2_post, s1_post]
    simp only [Array.to_slice, Slice.from_val, a_post, List.nil_append]
    rw [toLEBytes_vals _ (by decide)]
    simp [shared.format.MAGIC_NUMBER, magicNumber]
  step as ⟨ x, back1, x_post, back1_post ⟩
  step as ⟨ pos, pos_post ⟩
  have hM : (leBytes 4 magicNumber).length = 4 := by simp
  have W2 : Written dst (back1 descriptor) (leBytes 4 magicNumber ++ [descriptor.val]) := by
    rw [back1_post]; exact W1.set descriptor 4#usize (by simp) (by omega)
  -- The window descriptor, unless the frame is a single segment.
  apply WP.spec_bind (Pₘ := fun (r : Slice U8 × Usize) =>
    Written dst r.1 (leBytes 4 magicNumber ++ [descriptor.val] ++
      (if single then [] else [(header.window_log.val - 10) * 8])) ∧
    r.2.val = 5 + (if single then 0 else 1))
  · split
    · simp only [List.append_nil, WP.spec_ok]; exact ⟨W2, by simp [pos_post]⟩
    · rename_i hs
      step as ⟨ s3, s3_post ⟩
      step as ⟨ pos2, pos2_post ⟩
      refine ⟨?_, by omega⟩
      rw [s3_post, ← hwb]
      exact W2.set wbyte pos (by simp [pos_post]) (by simp; omega)
  rintro ⟨dst2, pos1⟩ ⟨W3, hpos1⟩
  dsimp only at W3 hpos1
  have hWl : (if single = true then ([] : List Nat) else [(header.window_log.val - 10) * 8]).length =
      (if single = true then 0 else 1) := by cases single <;> simp
  have hW1 : (if single = true then 0 else 1) ≤ 1 := by cases single <;> simp
  have hp1 : pos1.val = (leBytes 4 magicNumber ++ [descriptor.val] ++
      (if single then [] else [(header.window_log.val - 10) * 8])).length := by
    rw [hpos1]; cases single <;> simp
  -- The dictionary ID.
  apply WP.spec_bind (Pₘ := fun (r : Slice U8 × Usize) =>
    Written dst r.1 (leBytes 4 magicNumber ++ [descriptor.val] ++
      (if single then [] else [(header.window_log.val - 10) * 8]) ++
      leBytes (didFieldSize (didCode header.no_dict_id header.dict_id.val)) header.dict_id.val) ∧
    r.2.val = pos1.val + didFieldSize (didCode header.no_dict_id header.dict_id.val))
  · have hroom : pos1.val + 4 ≤ dst.length := by rw [hpos1]; split <;> omega
    split
    · have : didCode header.no_dict_id header.dict_id.val = 0 := by rw [← hdcv]; rfl
      have hz : leBytes (didFieldSize 0) header.dict_id.val = [] := by simp [didFieldSize, leBytes]
      simp only [WP.spec_ok, this, hz, List.append_nil]
      exact ⟨W3, by simp [didFieldSize]⟩
    · have hdc1 : didCode header.no_dict_id header.dict_id.val = 1 := by rw [← hdcv]; rfl
      step as ⟨ b, b_post ⟩
      step as ⟨ s3, s3_post ⟩
      step as ⟨ pos3, pos3_post ⟩
      refine ⟨?_, by rw [hdc1]; simp [didFieldSize, pos3_post]⟩
      have := W3.set b pos1 hp1 (by simp at hp1 ⊢; omega)
      rw [s3_post]
      convert this using 2
      rw [hdc1, b_post, UScalar.cast_val_eq]
      simp [didFieldSize, leBytes]
    · have hdc2 : didCode header.no_dict_id header.dict_id.val = 2 := by rw [← hdcv]; rfl
      step as ⟨ e, e_post ⟩
      step as ⟨ s3, back2, s3_val, s3_len, s3_back ⟩
      step as ⟨ v, v_post ⟩
      step as ⟨ a1, a1_post ⟩
      step as ⟨ s4, s4_post ⟩
      have hl : s3.length = s4.length := by
        rw [s3_len, s4_post, e_post]; simp [Array.to_slice, Slice.length]
      step as ⟨ s5, s5_post ⟩
      refine ⟨?_, by rw [hdc2, e_post]; simp [didFieldSize]⟩
      have := W3.setSlice s5.val (by rw [s5_post, s4_post]; simp at hp1 ⊢; omega)
      have heq : back2 s5 = dst2.setSlice! pos1.val s5.val := by rw [Slice.eq_iff]; simp [s3_back]
      rw [heq, hp1]
      convert this using 2
      rw [hdc2, s5_post, s4_post]
      simp only [Array.to_slice, Slice.from_val, a1_post]
      rw [toLEBytes_vals _ (by decide)]
      simp only [didFieldSize]
      have hv : v.bv.toNat = header.dict_id.val % 256 ^ 2 := by
        change v.val = _
        rw [v_post, UScalar.cast_val_eq]; norm_num
      show leBytes 2 _ = leBytes 2 _
      rw [hv, leBytes_mod]
    · rename_i n0 n1 n2
      have hdc3 : didCode header.no_dict_id header.dict_id.val = 3 := by
        have h0 : dc.val ≠ 0 := fun h => n0 (UScalar.eq_of_val_eq (h.trans rfl))
        have h1 : dc.val ≠ 1 := fun h => n1 (UScalar.eq_of_val_eq (h.trans rfl))
        have h2 : dc.val ≠ 2 := fun h => n2 (UScalar.eq_of_val_eq (h.trans rfl))
        omega
      clear n0 n1 n2
      have hd2 : dst2.length = dst.length := W3.2
      step as ⟨ e, e_post ⟩
      step as ⟨ s3, back2, s3_val, s3_len, s3_back ⟩
      step as ⟨ a1, a1_post ⟩
      step as ⟨ s4, s4_post ⟩
      have hl : s3.length = s4.length := by
        rw [s3_len, s4_post, e_post]; simp [Array.to_slice, Slice.length]
      step as ⟨ s5, s5_post ⟩
      refine ⟨?_, by rw [hdc3, e_post]; simp [didFieldSize]⟩
      have := W3.setSlice s5.val (by rw [s5_post, s4_post]; simp at hp1 ⊢; omega)
      have heq : back2 s5 = dst2.setSlice! pos1.val s5.val := by rw [Slice.eq_iff]; simp [s3_back]
      rw [heq, hp1]
      convert this using 2
      rw [hdc3, s5_post, s4_post]
      simp only [Array.to_slice, Slice.from_val, a1_post]
      rw [toLEBytes_vals _ (by decide)]
      rfl
  rintro ⟨dst3, pos2⟩ ⟨W4, hpos2⟩
  dsimp only at W4 hpos2
  -- The content size; the header as a whole.
  have hD := Nat.le_of_lt_succ (show didFieldSize (didCode header.no_dict_id header.dict_id.val) < 5 by
    have := hdc; generalize didCode header.no_dict_id header.dict_id.val = c at *
    obtain rfl | rfl | rfl | rfl : c = 0 ∨ c = 1 ∨ c = 2 ∨ c = 3 := (by omega) <;> simp [didFieldSize])
  have hroom : pos2.val + 8 ≤ dst.length := by rw [hpos2, hpos1]; omega
  set Pre := leBytes 4 magicNumber ++ [descriptor.val] ++
      (if single then [] else [(header.window_log.val - 10) * 8]) ++
      leBytes (didFieldSize (didCode header.no_dict_id header.dict_id.val)) header.dict_id.val with hPre
  have hp2 : pos2.val = Pre.length := by
    rw [hpos2, hp1, hPre]; simp; omega
  have hH : leBytes 4 magicNumber ++ headerBytes header =
      Pre ++ fcsBytes (fcsCode header.content_size header.pledged.val) single header.pledged.val := by
    rw [hPre, hd, hsv]; simp [headerBytes]
  have hHl : 4 + (headerBytes header).length =
      Pre.length + (fcsBytes (fcsCode header.content_size header.pledged.val) single header.pledged.val).length := by
    have := congrArg List.length hH; simp at this ⊢; omega
  rw [hH, hHl]
  have hd2 : dst3.length = dst.length := W4.2
  split
  · -- `fcsCode = 0`: one byte in a single-segment frame, none otherwise.
    have hfc0 : fcsCode header.content_size header.pledged.val = 0 := by rw [← hfcv]; rfl
    rw [hfc0]
    split
    · rename_i hs
      step as ⟨ b, b_post ⟩
      step as ⟨ s3, s3_post ⟩
      step as ⟨ pos3, pos3_post ⟩
      refine ⟨pos3, rfl, by simp [fcsBytes, hs, pos3_post, hp2], ?_⟩
      have := W4.set b pos2 hp2 (by omega)
      rw [s3_post]
      convert this using 2
      rw [b_post, UScalar.cast_val_eq]
      simp [fcsBytes, hs, leBytes]
    · rename_i hs
      have hsf : single = false := by simpa using hs
      show WP.spec (ok (core.result.Result.Ok pos2, dst3)) _
      simp only [WP.spec_ok]
      refine ⟨pos2, rfl, by simp [fcsBytes, hsf, hp2], ?_⟩
      simpa [fcsBytes, hsf] using W4
  · -- `fcsCode = 1`: two bytes, offset by 256.
    have hfc1 : fcsCode header.content_size header.pledged.val = 1 := by rw [← hfcv]; rfl
    have h256 : 256 ≤ header.pledged.val := by
      unfold fcsCode at hfc1; split_ifs at hfc1 <;> omega
    rw [hfc1]
    step as ⟨ e, e_post ⟩
    step as ⟨ s3, back2, s3_val, s3_len, s3_back ⟩
    step as ⟨ v0, v0_post ⟩
    step as ⟨ v, v_post ⟩
    step as ⟨ a1, a1_post ⟩
    step as ⟨ s4, s4_post ⟩
    have hl : s3.length = s4.length := by
      rw [s3_len, s4_post, e_post]; simp [Array.to_slice, Slice.length]
    step as ⟨ s5, s5_post ⟩
    refine ⟨e, rfl, by simp [fcsBytes, e_post, hp2], ?_⟩
    have := W4.setSlice s5.val (by rw [s5_post, s4_post]; simp; omega)
    have heq : back2 s5 = dst3.setSlice! pos2.val s5.val := by rw [Slice.eq_iff]; simp [s3_back]
    rw [heq, hp2]
    convert this using 2
    rw [s5_post, s4_post]
    simp only [Array.to_slice, Slice.from_val, a1_post]
    rw [toLEBytes_vals _ (by decide)]
    have hv : v.bv.toNat = (header.pledged.val - 256) % 256 ^ 2 := by
      change v.val = _
      rw [v_post, UScalar.cast_val_eq, v0_post]; norm_num
    show leBytes 2 _ = leBytes 2 _
    rw [hv, leBytes_mod]
  · -- `fcsCode = 2`: four bytes.
    have hfc2 : fcsCode header.content_size header.pledged.val = 2 := by rw [← hfcv]; rfl
    rw [hfc2]
    step as ⟨ e, e_post ⟩
    step as ⟨ s3, back2, s3_val, s3_len, s3_back ⟩
    step as ⟨ v, v_post ⟩
    step as ⟨ a1, a1_post ⟩
    step as ⟨ s4, s4_post ⟩
    have hl : s3.length = s4.length := by
      rw [s3_len, s4_post, e_post]; simp [Array.to_slice, Slice.length]
    step as ⟨ s5, s5_post ⟩
    refine ⟨e, rfl, by simp [fcsBytes, e_post, hp2], ?_⟩
    have := W4.setSlice s5.val (by rw [s5_post, s4_post]; simp; omega)
    have heq : back2 s5 = dst3.setSlice! pos2.val s5.val := by rw [Slice.eq_iff]; simp [s3_back]
    rw [heq, hp2]
    convert this using 2
    rw [s5_post, s4_post]
    simp only [Array.to_slice, Slice.from_val, a1_post]
    rw [toLEBytes_vals _ (by decide)]
    have hv : v.bv.toNat = header.pledged.val % 256 ^ 4 := by
      change v.val = _
      rw [v_post, UScalar.cast_val_eq]; norm_num
    show leBytes 4 _ = leBytes 4 _
    rw [hv, leBytes_mod]
  · -- `fcsCode = 3`: eight bytes.
    rename_i n0 n1 n2
    have hfc3 : fcsCode header.content_size header.pledged.val = 3 := by
      have h0 : fc.val ≠ 0 := fun h => n0 (UScalar.eq_of_val_eq (h.trans rfl))
      have h1 : fc.val ≠ 1 := fun h => n1 (UScalar.eq_of_val_eq (h.trans rfl))
      have h2 : fc.val ≠ 2 := fun h => n2 (UScalar.eq_of_val_eq (h.trans rfl))
      omega
    clear n0 n1 n2
    rw [hfc3]
    step as ⟨ e, e_post ⟩
    step as ⟨ s3, back2, s3_val, s3_len, s3_back ⟩
    step as ⟨ a1, a1_post ⟩
    step as ⟨ s4, s4_post ⟩
    have hl : s3.length = s4.length := by
      rw [s3_len, s4_post, e_post]; simp [Array.to_slice, Slice.length]
    step as ⟨ s5, s5_post ⟩
    refine ⟨e, rfl, by simp [fcsBytes, e_post, hp2], ?_⟩
    have := W4.setSlice s5.val (by rw [s5_post, s4_post]; simp; omega)
    have heq : back2 s5 = dst3.setSlice! pos2.val s5.val := by rw [Slice.eq_iff]; simp [s3_back]
    rw [heq, hp2]
    convert this using 2
    rw [s5_post, s4_post]
    simp only [Array.to_slice, Slice.from_val, a1_post]
    rw [toLEBytes_vals _ (by decide)]
    rfl

/-- An index past the first list reads the second. -/
theorem getElem!_append_right' (a b : List Nat) (i : Nat) (h : a.length ≤ i) :
    (a ++ b)[i]! = b[i - a.length]! := by
  simp only [List.getElem!_eq_getElem?_getD, List.getElem?_append_right h]

/-- An index inside the first list reads it. -/
theorem getElem!_append_left' (a b : List Nat) (i : Nat) (h : i < a.length) :
    (a ++ b)[i]! = a[i]! := by
  simp only [List.getElem!_eq_getElem?_getD, List.getElem?_append_left h]

/-- At most 14 header bytes (Table 2), one at least. -/
theorem headerBytes_length_le (h : compressor.core.cctx.FrameHeaderFields) :
    1 ≤ (headerBytes h).length ∧ (headerBytes h).length ≤ 14 := by
  rw [headerBytes_length]
  have := didCode_le h.no_dict_id h.dict_id.val
  have := fcsCode_le h.content_size h.pledged.val
  generalize didCode h.no_dict_id h.dict_id.val = a at *
  generalize fcsCode h.content_size h.pledged.val = c at *
  generalize singleOf h.content_size h.window_log.val h.pledged.val = s
  obtain rfl | rfl | rfl | rfl : a = 0 ∨ a = 1 ∨ a = 2 ∨ a = 3 := (by omega)
  all_goals obtain rfl | rfl | rfl | rfl : c = 0 ∨ c = 1 ∨ c = 2 ∨ c = 3 := (by omega)
  all_goals (cases s <;> simp [didFieldSize, fcsFieldSize])

/-- **RFC 8878 section 3.1.1.1, encoder side.** `write_frame_header` returns
the end `pos` of what it wrote, `4 < pos ≤ 18`, keeps the length of `dst` and
every byte from `pos` on; its first four bytes read little-endian are the
magic number `0xFD2FB528`, and bytes `4..pos` are a frame header that
`Mzstd.Rfc.parseFrameHeader` reads as the declared window, content size,
dictionary ID and checksum flag (`declared`). The descriptor's reserved bit 3
and unused bit 4 are zero, and the `Window_Descriptor`, when present, has a zero
mantissa. -/
theorem write_frame_header_rfc (dst : Slice U8)
    (header : compressor.core.cctx.FrameHeaderFields) (hdst : 18 ≤ dst.length)
    (hw : 10 ≤ header.window_log.val) (hw' : header.window_log.val ≤ 31) :
    compressor.core.cctx.write_frame_header dst header ⦃
      (r : core.result.Result Usize compressor.core.error.CoreError × Slice U8) =>
      ∃ pos : Usize, r.1 = .Ok pos ∧ 4 < pos.val ∧ pos.val ≤ 18 ∧
        r.2.length = dst.length ∧ r.2.val.drop pos.val = dst.val.drop pos.val ∧
        le ((r.2.val.take 4).map UScalar.val) = magicNumber ∧
        parseFrameHeader (((r.2.val.drop 4).take (pos.val - 4)).map UScalar.val) =
          some (declared header) ∧
        reservedBit r.2.val[4]!.val = 0 ∧ r.2.val[4]!.val / 16 % 2 = 0 ∧
        (singleOf header.content_size header.window_log.val header.pledged.val = false →
          r.2.val[5]!.val % 8 = 0) ⦄ := by
  apply WP.spec_mono (compressor.core.cctx.write_frame_header.spec dst header hdst hw hw')
  rintro ⟨res, out⟩ ⟨pos, hres, hpos, hwr, hlen⟩
  dsimp only at hres hpos hwr hlen ⊢
  obtain ⟨hH1, hH14⟩ := headerBytes_length_le header
  have hM : (leBytes 4 magicNumber).length = 4 := by simp
  have hmap : out.val.map UScalar.val =
      leBytes 4 magicNumber ++ headerBytes header ++ (dst.val.drop pos.val).map UScalar.val := by
    rw [hwr, hpos]; simp
  -- Reading the written list at a position.
  have hat : ∀ i, 4 ≤ i → i < pos.val → out.val[i]!.val = (headerBytes header)[i - 4]! := by
    intro i h4 hi
    have hlo : i < out.val.length := by
      have := congrArg List.length hmap; simp at this; simp [Slice.length] at hlen hdst; omega
    have := congrArg (fun l => l[i]!) hmap
    beta_reduce at this
    have key : (out.val.map UScalar.val)[i]! = (out.val[i]!).val := by
      rw [getElem!_pos (out.val.map UScalar.val) i (by simpa using hlo), getElem!_pos out.val i hlo]
      simp
    rw [key] at this
    rw [this, List.append_assoc, getElem!_append_right' _ _ _ (by simp; omega), hM,
      getElem!_append_left' _ _ _ (by omega)]
  refine ⟨pos, hres, by omega, by omega, hlen, ?_, ?_, ?_, ?_, ?_, ?_⟩
  · apply List.map_injective_iff.mpr (fun a b h => UScalar.eq_of_val_eq h)
    rw [List.map_drop, List.map_drop, hmap, hpos, List.drop_left' (by simp), List.map_drop]
  · rw [List.map_take, hmap, List.append_assoc, List.take_left' hM, le_leBytes]
    simp [magicNumber]
  · rw [List.map_take, List.map_drop, hmap, List.append_assoc, List.drop_left' hM,
      hpos, Nat.add_sub_cancel_left, List.take_left' rfl]
    exact parse_headerBytes header hw hw'
  · rw [hat 4 (le_refl _) (by omega)]
    simp only [Nat.sub_self, headerBytes, List.cons_append,
      List.getElem!_cons_zero, reservedBit]
    have := didCode_le header.no_dict_id header.dict_id.val
    have := Bool.toNat_le header.checksum
    have := Bool.toNat_le (singleOf header.content_size header.window_log.val header.pledged.val)
    omega
  · rw [hat 4 (le_refl _) (by omega)]
    simp only [Nat.sub_self, headerBytes, List.cons_append,
      List.getElem!_cons_zero]
    have := didCode_le header.no_dict_id header.dict_id.val
    have := Bool.toNat_le header.checksum
    have := Bool.toNat_le (singleOf header.content_size header.window_log.val header.pledged.val)
    omega
  · intro hs
    have h6 : 6 ≤ pos.val := by
      rw [hpos, headerBytes_length, hs]; simp; omega
    rw [hat 5 (by omega) (by omega)]
    simp [headerBytes, hs]

end mzstd
