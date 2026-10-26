import Mzstd.Basic

/-! # Frame headers: the decoder

`FrameHeader::parse` (`decompressor/core/frame.rs`) and the helpers it calls
read a frame header exactly as RFC 8878 section 3.1.1.1 describes it,
`Mzstd.Rfc.parseFrameHeader`: for every byte string, a header the RFC rejects
(empty, a size other than the one its descriptor declares, or the reserved bit
set) is rejected with `InvalidDataKind::FrameHeader`, and any other header
yields the RFC's `Window_Size`, `Frame_Content_Size`, `Dictionary_ID` and
`Content_Checksum_Flag`. The proofs also show that none of the indexing,
shifts or additions on the way can panic or overflow.
-/

open Aeneas Aeneas.Std Result
open Mzstd.Rfc

namespace mzstd
open decompressor.core.frame

/-- **Spec theorem for `mzstd::decompressor::core::frame::dictionary_id_size`**
`DID_Field_Size` of the descriptor's `Dictionary_ID_Flag` (Table 5). -/
@[step]
theorem dictionary_id_size.spec (descriptor : U8) :
    dictionary_id_size descriptor ⦃ (r : Usize) => r.val = didFieldSize (didFlag descriptor.val) ⦄ := by
  unfold dictionary_id_size
  step as ⟨ i, i_post ⟩
  have hi : i.val = descriptor.val % 4 := by
    rw [i_post, UScalar.val_and]; exact Nat.and_two_pow_sub_one_eq_mod _ 2
  split
  · change 0 = descriptor.val % 4 at hi; simp [didFieldSize, didFlag, ← hi]
  · change 1 = descriptor.val % 4 at hi; simp [didFieldSize, didFlag, ← hi]
  · change 2 = descriptor.val % 4 at hi; simp [didFieldSize, didFlag, ← hi]
  · rename_i h0 h1 h2
    have h3 : descriptor.val % 4 = 3 := by
      have n0 : i.val ≠ 0 := fun h => h0 (UScalar.eq_of_val_eq (h.trans rfl))
      have n1 : i.val ≠ 1 := fun h => h1 (UScalar.eq_of_val_eq (h.trans rfl))
      have n2 : i.val ≠ 2 := fun h => h2 (UScalar.eq_of_val_eq (h.trans rfl))
      omega
    simp [didFieldSize, didFlag, h3]

/-- **Spec theorem for `mzstd::decompressor::core::frame::described_window_size`**
The `Window_Size` formula of section 3.1.1.1.2, without overflow for every
`Window_Descriptor` byte (the largest is `2 ^ 41 + 7 * 2 ^ 38`). -/
@[step]
theorem described_window_size.spec (window : U8) :
    described_window_size window ⦃ (r : U64) => r.val = windowSize window.val ⦄ := by
  have hw := window.hBounds
  unfold described_window_size
  have hw256 : window.val < 256 := by simpa using hw
  step as ⟨ i, i_post ⟩
  have hi : i.val = window.val / 8 := by rw [i_post, Nat.shiftRight_eq_div_pow]
  step as ⟨ i1, i1_post ⟩
  have hi1 : i1.val = window.val / 8 := by
    rw [i1_post, UScalar.cast_val_eq, Nat.mod_eq_of_lt (by scalar_tac), hi]
  step as ⟨ i2, i2_post ⟩
  have hi2 : i2.val = 10 + window.val / 8 := by rw [i2_post, hi1]; simp
  have hlog : i2.val ≤ 41 := by omega
  have hpow : 2 ^ i2.val ≤ 2 ^ 41 := Nat.pow_le_pow_right (by decide) hlog
  step as ⟨ base, base_post ⟩
  have hbase : base.val = 2 ^ (10 + window.val / 8) := by
    rw [base_post, Nat.one_shiftLeft, Nat.mod_eq_of_lt (by scalar_tac), hi2]
  step as ⟨ i3, i3_post ⟩
  step as ⟨ i4, i4_post ⟩
  have hi4 : i4.val = window.val % 8 := by
    rw [i4_post, UScalar.val_and]; exact Nat.and_two_pow_sub_one_eq_mod _ 3
  step as ⟨ i5, i5_post ⟩
  have hi5 : i5.val = window.val % 8 := by
    rw [i5_post, UScalar.cast_val_eq, Nat.mod_eq_of_lt (by scalar_tac), hi4]
  have hb8 : base.val / 8 ≤ 2 ^ 38 := by
    rw [hbase]
    calc 2 ^ (10 + window.val / 8) / 8 ≤ 2 ^ 41 / 8 :=
          Nat.div_le_div_right (Nat.pow_le_pow_right (by decide) (by omega))
      _ = 2 ^ 38 := by norm_num
  have hm : base.val / 8 * (window.val % 8) ≤ 2 ^ 38 * 7 :=
    Nat.mul_le_mul hb8 (by omega)
  step as ⟨ i6, i6_post ⟩
  step as ⟨ r, r_post ⟩
  rw [r_post, i6_post, i3_post, hi5, hbase]
  simp [windowSize]

/-- `Dictionary_ID` is at most four bytes. -/
theorem descriptor_didFieldSize_le (f : Nat) : didFieldSize f ≤ 4 := by
  unfold didFieldSize
  rcases f with _ | _ | _ | _ | f <;> simp

/-- Bit 5 of a descriptor byte is `Single_Segment_Flag`. -/
theorem descriptor_and_32_ne_zero : ∀ d < 256, (d &&& 32 ≠ 0) ↔ singleSegment d = true := by decide +kernel

/-- Bits 7-6 of a descriptor byte are `Frame_Content_Size_Flag`. -/
theorem descriptor_shift_6 : ∀ d < 256, d >>> 6 = fcsFlag d := by decide +kernel

/-- **Spec theorem for `mzstd::decompressor::core::frame::header_size`**
The header size the descriptor declares (Table 2, with Tables 4 and 5). -/
@[step]
theorem header_size.spec (descriptor : U8) :
    header_size descriptor ⦃ (r : Usize) => r.val = headerSize descriptor.val ⦄ := by
  have hw : descriptor.val < 256 := by simpa using descriptor.hBounds
  unfold header_size
  step as ⟨ i, i_post ⟩
  have hs : i ≠ 0#u8 ↔ singleSegment descriptor.val = true := by
    rw [← descriptor_and_32_ne_zero _ hw]
    constructor
    · intro h h'; apply h; apply UScalar.eq_of_val_eq; rw [i_post, UScalar.val_and]; simpa using h'
    · intro h h'; apply h; have := congrArg UScalar.val h'; rw [i_post, UScalar.val_and] at this
      simpa using this
  have hb : (i != 0#u8) = singleSegment descriptor.val := by
    rw [Bool.eq_iff_iff, bne_iff_ne]; exact hs
  rw [hb]
  have hf := descriptor_shift_6 _ hw
  have hlt : fcsFlag descriptor.val < 4 := by simp [fcsFlag]; omega
  cases hss : singleSegment descriptor.val <;> simp only [Bool.false_eq_true, ite_true, ite_false] <;>
    step as ⟨ dictionary, hdict ⟩ <;> step as ⟨ i1, i1_post ⟩ <;> rw [hf] at i1_post <;>
    have hdid := descriptor_didFieldSize_le (didFlag descriptor.val) <;> split
  all_goals
    first
    | (change 0 = fcsFlag descriptor.val at i1_post)
    | (change 1 = fcsFlag descriptor.val at i1_post)
    | (change 2 = fcsFlag descriptor.val at i1_post)
    | (rename_i h0 h1 h2
       have : fcsFlag descriptor.val = 3 := by
         have n0 : i1.val ≠ 0 := fun h => h0 (UScalar.eq_of_val_eq (h.trans rfl))
         have n1 : i1.val ≠ 1 := fun h => h1 (UScalar.eq_of_val_eq (h.trans rfl))
         have n2 : i1.val ≠ 2 := fun h => h2 (UScalar.eq_of_val_eq (h.trans rfl))
         omega
       clear i1_post
       have i1_post := this.symm)
  all_goals
    step*
    simp [headerSize, windowDescriptorSize, fcsFieldSize, hss, ← i1_post]
    scalar_tac

/-- A little-endian field of `n` bytes is below `2 ^ (8 n)`. -/
theorem read_le_bytes_lt (l : List U8) : le (l.map (·.val)) < 2 ^ (8 * l.length) := by
  induction l with
  | nil => simp [le]
  | cons b l ih =>
    have hb : b.val < 256 := by simpa using b.hBounds
    simp only [List.map_cons, le, List.length_cons]
    rw [show 8 * (l.length + 1) = 8 * l.length + 8 by ring, pow_add]
    have : 2 ^ 8 = 256 := by norm_num
    rw [this]
    have : le (l.map (·.val)) + 1 ≤ 2 ^ (8 * l.length) := ih
    nlinarith

/-- The size of `u64`. -/
theorem read_le_U64_size_eq : U64.size = 2 ^ 64 := by simp [U64.size_def, U64.numBits_def]

/-- Widening a byte keeps its value. -/
@[step]
theorem read_le_widen.spec (x : U8) :
    lift (core.convert.num.FromU64U8.from x) ⦃ (y : U64) => y.val = x.val ⦄ := by
  simp only [lift, WP.spec_ok, core.convert.num.FromU64U8.from_val_eq]

/-- The loop of `read_le` reads the bytes from the end down to `index`:
`value` holds the little-endian value of the suffix from `index`. -/
@[step]
theorem read_le_loop.spec (bytes : Slice U8) (value : U64) (index : Usize)
    (hlen : bytes.length ≤ 8) (hidx : index.val ≤ bytes.length)
    (hv : value.val = le ((bytes.val.drop index.val).map (·.val))) :
    read_le_loop bytes value index ⦃ (r : U64) => r.val = le (bytes.val.map (·.val)) ⦄ := by
  unfold read_le_loop
  split
  · step as ⟨ index1, index1_post ⟩
    step as ⟨ i, i_post ⟩
    step as ⟨ i1, i1_post ⟩
    step as ⟨ i2, i2_post ⟩
    step as ⟨ value1, value1_post ⟩
    apply read_le_loop.spec bytes value1 index1 hlen (by scalar_tac)
    have hdrop : bytes.val.drop index1.val = i1 :: bytes.val.drop index.val := by
      have hlt : index1.val < bytes.val.length := by scalar_tac
      rw [i1_post, List.drop_eq_getElem_cons hlt]
      congr 2; scalar_tac
    have hsuffix := read_le_bytes_lt (bytes.val.drop index.val)
    rw [← hv, List.length_drop] at hsuffix
    have hsmall : value.val * 2 ^ 8 < 2 ^ 64 := by
      have : 8 * (bytes.val.length - index.val) + 8 ≤ 64 := by scalar_tac
      calc value.val * 2 ^ 8 < 2 ^ (8 * (bytes.val.length - index.val)) * 2 ^ 8 :=
            Nat.mul_lt_mul_of_pos_right hsuffix (by decide)
        _ = 2 ^ (8 * (bytes.val.length - index.val) + 8) := by rw [pow_add]
        _ ≤ 2 ^ 64 := Nat.pow_le_pow_right (by decide) this
    have hb : i1.val < 2 ^ 8 := by simpa using i1.hBounds
    have hi2 : i2.val = i1.val := i2_post
    rw [hdrop, List.map_cons, le, value1_post, UScalar.val_or, i_post, hi2, Nat.shiftLeft_eq,
      Nat.mod_eq_of_lt (by rw [read_le_U64_size_eq]; simpa using hsmall), ← hv, Nat.mul_comm, ← Nat.two_pow_add_eq_or_of_lt hb]
    ring
  · have : index.val = 0 := by scalar_tac
    rw [this, List.drop_zero] at hv
    simpa using hv
termination_by index.val
decreasing_by scalar_decr_tac

/-- **Spec theorem for `mzstd::decompressor::core::frame::read_le`**
A field of up to eight bytes, read little-endian. -/
@[step]
theorem read_le.spec (bytes : Slice U8) (hlen : bytes.length ≤ 8) :
    read_le bytes ⦃ (r : U64) => r.val = le (bytes.val.map (·.val)) ⦄ := by
  unfold read_le
  apply read_le_loop.spec bytes 0#u64 _ hlen (by simp)
  simp [le]

/-- Bit 3 of a descriptor byte is the reserved bit. -/
theorem descriptor_and_8_ne_zero : ∀ d < 256, (d &&& 8 ≠ 0) ↔ reservedBit d = 1 := by decide +kernel

/-- Bit 2 of a descriptor byte is `Content_Checksum_Flag`. -/
theorem descriptor_and_4_ne_zero : ∀ d < 256, (d &&& 4 ≠ 0) ↔ checksumFlag d = true := by decide +kernel

/-- What `FrameHeader::parse` returns for a header the RFC reads as `expected`:
the frame header error for a header the RFC rejects, and otherwise the RFC's
four values. -/
def FrameHeaderResult (expected : Option Mzstd.Rfc.FrameHeader)
    (r : core.result.Result decompressor.core.frame.FrameHeader decompressor.error.InvalidDataKind) :
    Prop :=
  match expected with
  | none => r = .Err .FrameHeader
  | some h => ∃ out, r = .Ok out ∧ out.window_size.val = h.windowSize ∧
      out.content_size.map (·.val) = h.contentSize ∧ out.dictionary_id.val = h.dictionaryId ∧
      out.checksum = h.checksum

/-- **Spec theorem for `mzstd::decompressor::core::frame::FrameHeader::parse`**
For every byte string, the result is the RFC's reading of it (RFC 8878 section
3.1.1.1, `Mzstd.Rfc.parseFrameHeader`), and the parse never panics. -/
@[step]
theorem FrameHeader.parse.spec (bytes : Slice U8) :
    FrameHeader.parse bytes ⦃ (r : core.result.Result decompressor.core.frame.FrameHeader
        decompressor.error.InvalidDataKind) =>
      FrameHeaderResult (parseFrameHeader (bytes.val.map (·.val))) r ⦄ := by
  unfold FrameHeader.parse
  step as ⟨ b, b_post ⟩
  split
  · have : bytes.length = 0 := by rwa [← b_post]
    have : bytes.val = [] := by simpa [Slice.length] using this
    simp [this, parseFrameHeader, FrameHeaderResult]
  · have hne : bytes.length ≠ 0 := fun h => ‹¬b = true› (b_post ▸ h)
    obtain ⟨x, tl, hx⟩ : ∃ x tl, bytes.val = x :: tl := by
      cases h : bytes.val with
      | nil => simp [Slice.length, h] at hne
      | cons x tl => exact ⟨x, tl, rfl⟩
    have hxb : x.val < 256 := by simpa using x.hBounds
    step as ⟨ descriptor, d_post ⟩
    have hd : descriptor = x := by simp [d_post, hx]
    subst hd
    step as ⟨ i1, i1_post ⟩
    have hlen : bytes.len.val = tl.length + 1 := by simp [Slice.len, hx]
    have hL : bytes.val.map (·.val) = descriptor.val :: tl.map (·.val) := by simp [hx]
    rw [hL]
    split
    · rename_i hneq
      have : tl.length + 1 ≠ headerSize descriptor.val := by
        rw [← hlen, ← i1_post]; intro h; simp [bne_iff_ne, UScalar.neq_to_neq_val, h] at hneq
      simp [parseFrameHeader, FrameHeaderResult, this]
    · rename_i heq
      have hsize : tl.length + 1 = headerSize descriptor.val := by
        rw [← hlen, ← i1_post]; simpa [bne_iff_ne, UScalar.neq_to_neq_val] using heq
      step as ⟨ i2, i2_post ⟩
      split
      · rename_i hres
        have : reservedBit descriptor.val = 1 := by
          rw [← descriptor_and_8_ne_zero _ hxb]; intro h; apply (bne_iff_ne.mp hres)
          apply UScalar.eq_of_val_eq; rw [i2_post, UScalar.val_and]; simpa using h
        simp [parseFrameHeader, FrameHeaderResult, this]
      · rename_i hres
        have hr0 : reservedBit descriptor.val ≠ 1 := by
          intro h1; have h8 := (descriptor_and_8_ne_zero _ hxb).mpr h1
          apply hres; rw [bne_iff_ne]; intro h'; apply h8
          have := congrArg UScalar.val h'; rw [i2_post, UScalar.val_and] at this
          simpa using this
        step as ⟨ i3, i3_post ⟩
        have hb3 : (i3 != 0#u8) = singleSegment descriptor.val := by
          rw [Bool.eq_iff_iff, bne_iff_ne, ← descriptor_and_32_ne_zero _ hxb]
          constructor
          · intro h h'; apply h; apply UScalar.eq_of_val_eq; rw [i3_post, UScalar.val_and]; simpa using h'
          · intro h h'; apply h; have := congrArg UScalar.val h'; rw [i3_post, UScalar.val_and] at this
            simpa using this
        rw [hb3]
        have hdid := descriptor_didFieldSize_le (didFlag descriptor.val)
        have hhs : headerSize descriptor.val = 1 + windowDescriptorSize (singleSegment descriptor.val) +
            didFieldSize (didFlag descriptor.val) +
            fcsFieldSize (fcsFlag descriptor.val) (singleSegment descriptor.val) := rfl
        have hfcs : fcsFieldSize (fcsFlag descriptor.val) (singleSegment descriptor.val) ≤ 8 := by
          unfold fcsFieldSize; split <;> (try split) <;> omega
        cases hss : singleSegment descriptor.val <;>
          simp only [Bool.false_eq_true, ite_true, ite_false] <;>
          simp only [hss, windowDescriptorSize, Bool.false_eq_true, ite_true, ite_false] at hhs hfcs
        all_goals
          step as ⟨ i4, i4_post ⟩
          step as ⟨ content_start, cs_post ⟩
          step as ⟨ dictionary, dict_post, dict_len ⟩
          step as ⟨ fields, fields_post, fields_len ⟩
          have hbl : bytes.length = tl.length + 1 := by simp [Slice.length, hx]
          -- The two fields are the slices of the header the RFC reads them from.
          have hdict : dictionary.val.map (·.val) =
              ((descriptor.val :: tl.map (·.val)).drop
                (1 + windowDescriptorSize (singleSegment descriptor.val))).take
                (didFieldSize (didFlag descriptor.val)) := by
            rw [dict_post, hx, hss]
            simp [List.slice, windowDescriptorSize, cs_post, i4_post]
          have hfields : fields.val.map (·.val) =
              (descriptor.val :: tl.map (·.val)).drop
                (1 + windowDescriptorSize (singleSegment descriptor.val) +
                  didFieldSize (didFlag descriptor.val)) := by
            rw [fields_post, hx, hss]
            simp [windowDescriptorSize, List.map_drop, cs_post, i4_post]
          have hparse : parseFrameHeader (descriptor.val :: tl.map (·.val)) = some
              { windowSize := if singleSegment descriptor.val then
                    (if fields.val.map (·.val) = [] then none
                      else some (frameContentSize (fields.val.map (·.val)))).getD 0
                  else windowSize ((descriptor.val :: tl.map (·.val)).getD 1 0)
                contentSize := if fields.val.map (·.val) = [] then none
                  else some (frameContentSize (fields.val.map (·.val)))
                dictionaryId := le (dictionary.val.map (·.val))
                checksum := checksumFlag descriptor.val } := by
            rw [hdict, hfields]
            unfold parseFrameHeader
            simp only [List.length_cons, List.length_map, hsize, ne_eq, not_true_eq_false, hr0,
              or_self, ite_false]
          rw [hparse]
          have hflen : fields.len.val = (fields.val.map (·.val)).length := by simp [Slice.len]
          have hfl8 : fields.val.length ≤ 8 := by simp only [Slice.length] at *; omega
          have hdl4 : dictionary.val.length ≤ 4 := by simp only [Slice.length] at *; omega
          have hfle := read_le_bytes_lt fields.val
          have hdle := read_le_bytes_lt dictionary.val
          have hdle32 : le (dictionary.val.map (·.val)) < 2 ^ 32 :=
            lt_of_lt_of_le hdle (Nat.pow_le_pow_right (by decide) (by omega))
          have hck : (descriptor &&& 4#u8).val ≠ 0 ↔ checksumFlag descriptor.val = true := by
            rw [UScalar.val_and]; exact descriptor_and_4_ne_zero _ hxb
          -- `Frame_Content_Size`, absent for an empty field.
          refine WP.spec_bind (Pₘ := fun (cs : Option U64) => cs.map (·.val) =
              if fields.val.map (·.val) = [] then none
              else some (frameContentSize (fields.val.map (·.val)))) ?_ ?_
          · split
            · rename_i hm
              have hf0 : fields.val.map (·.val) = [] := by
                rw [← List.length_eq_zero_iff, ← hflen]; exact hm
              simp [hf0]
            · rename_i hm
              have h2 : fields.val.length = 2 := by simpa using hflen.symm.trans hm
              rw [h2] at hfle
              have hf2 : (fields.val.map (·.val)).length = 2 := by simpa using h2
              have hf0 : fields.val.map (·.val) ≠ [] := by
                intro h; rw [h] at hf2; simp at hf2
              step as ⟨ v, v_post ⟩
              step as ⟨ w, w_post ⟩
              simp [hf0, frameContentSize, hf2, w_post, v_post]
            · rename_i hm0 hm2
              have hf0 : fields.val.map (·.val) ≠ [] := by
                intro h; exact hm0 (by rw [hflen, h]; rfl)
              have hf2 : (fields.val.map (·.val)).length ≠ 2 := by rw [← hflen]; exact hm2
              have hf2' : fields.val.length ≠ 2 := by simpa using hf2
              step as ⟨ v, v_post ⟩
              simp [hf0, frameContentSize, hf2', v_post]
          intro cs hcs
          -- `Window_Size`: the content size in a single segment, the window
          -- descriptor otherwise. Then the dictionary ID and the checksum flag.
          have hwin : ∀ ws : U64, ws.val = (if singleSegment descriptor.val then cs.map (·.val)
              else some (windowSize ((descriptor.val :: tl.map (·.val)).getD 1 0))).getD 0 →
              ws.val = if singleSegment descriptor.val then
                (if fields.val.map (·.val) = [] then none
                  else some (frameContentSize (fields.val.map (·.val)))).getD 0
              else windowSize ((descriptor.val :: tl.map (·.val)).getD 1 0) := by
            intro ws h; rw [h, ← hcs]; split <;> simp
          have hdict_id : ∀ i6 : U64, i6.val = le (dictionary.val.map (·.val)) →
              (UScalar.cast .U32 i6).val = le (dictionary.val.map (·.val)) := by
            intro i6 h
            rw [UScalar.cast_val_eq, Nat.mod_eq_of_lt (by simpa [h] using hdle32), h]
          have hcheck : ∀ i8 : U8, i8.val = (descriptor &&& 4#u8).val →
              (i8 != 0#u8) = checksumFlag descriptor.val := by
            intro i8 h
            rw [Bool.eq_iff_iff, bne_iff_ne, ← hck, ← h]
            exact ⟨fun h h' => h (UScalar.eq_of_val_eq (h'.trans rfl)),
              fun h h' => h (by rw [h']; rfl)⟩
          first
          | (simp only [hss, ite_true] at hwin ⊢
             step as ⟨ i6, i6_post ⟩
             step as ⟨ i7, i7_post ⟩
             step as ⟨ i8, i8_post ⟩
             refine ⟨_, rfl, hwin _ ?_, hcs, ?_, hcheck i8 i8_post⟩
             · cases cs <;> simp
             · rw [i7_post]; exact hdict_id i6 i6_post)
          | (simp only [hss, Bool.false_eq_true, ite_false] at hwin ⊢
             step as ⟨ wd, wd_post ⟩
             step as ⟨ ws, ws_post ⟩
             step as ⟨ i6, i6_post ⟩
             step as ⟨ i7, i7_post ⟩
             step as ⟨ i8, i8_post ⟩
             refine ⟨_, rfl, hwin _ ?_, hcs, ?_, hcheck i8 i8_post⟩
             · have htl : 0 < tl.length := by omega
               rw [ws_post, wd_post]; simp [hx, List.getElem?_eq_getElem htl]
             · rw [i7_post]; exact hdict_id i6 i6_post)


end mzstd
