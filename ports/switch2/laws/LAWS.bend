import Base
import ./Policy.bend as P

law declined_write:
  for fresh: Bool
  {P.write(False{}, fresh) == False{} : Bool}

law stale_write:
  for approved: Bool
  {P.write(approved, False{}) == False{} : Bool}

law approved_current_write:
  {P.write(True{}, True{}) == True{} : Bool}

law busy_session_locked:
  for picker: Bool
  {P.session_control(True{}, picker) == False{} : Bool}

law picker_owns_input:
  {P.session_control(False{}, True{}) == False{} : Bool}

law idle_session_available:
  {P.session_control(False{}, False{}) == True{} : Bool}

law sixth_call_available:
  {P.remaining(5n) == 1n : Nat}

law no_seventh_call:
  for extra: Nat
  {P.remaining(6n+extra) == 0n : Nat}

# RW1 payload framing after the decimal length and colon have been read.
law session_field_restored:
  for field: List<U32>
  for rest: List<U32>
  {List.take(&1, U32, List.append(&1, U32, field, rest), List.length(&1, U32, field)) == field : List<U32>}

law next_session_field_intact:
  for field: List<U32>
  for rest: List<U32>
  {List.drop(&1, U32, List.append(&1, U32, field, rest), List.length(&1, U32, field)) == rest : List<U32>}
