import Base
import ./LAWS.bend as L

def L.declined_write(fresh): {==}

def L.stale_write(approved):
  match approved:
    case False{}: {==}
    case True{}: {==}

def L.approved_current_write(): {==}
def L.busy_session_locked(picker): {==}
def L.picker_owns_input(): {==}
def L.idle_session_available(): {==}
def L.sixth_call_available(): {==}
def L.no_seventh_call(extra): {==}

def L.session_field_restored(field, rest):
  match field:
    case Nil{}:
      match rest:
        case Nil{}: {==}
        case h <> t: {==}
    case h <> t:
      %L.session_field_restored(t, rest) : {h <> List.take(&1, U32, List.append(&1, U32, t, rest), List.length(&1, U32, t)) == h <> _ : List<U32>}
      {==}

def L.next_session_field_intact(field, rest):
  match field:
    case Nil{}:
      match rest:
        case Nil{}: {==}
        case h <> t: {==}
    case h <> t: L.next_session_field_intact(t, rest)
