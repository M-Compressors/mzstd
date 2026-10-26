module

public import Aeneas
public import Mzstd.Code.Types

/-! # Models of the standard-library items the translation calls

Aeneas translates calls into the Rust standard library to Lean definitions from
its own library; the items below have no model there yet, so they are written
here, following the behaviour the standard library documents. They are part of
the trusted base: each restates one documented function and is short enough to
read against its documentation.

`scripts/extract_lean.py` lists the same names in `EXTERNALS` and fails when
the translation needs any other unmodelled item.
-/

@[expose] public section

open Aeneas Aeneas.Std Result

/-- `<[T]>::first`: the first element, or `None` for an empty slice. -/
@[rust_fun "core::slice::{[@T]}::first"]
def core.slice.Slice.first {T : Type _} (s : Slice T) : Result (Option T) :=
  ok s.val.head?

end
