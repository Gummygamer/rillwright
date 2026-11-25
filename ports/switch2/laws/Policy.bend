import Base

def write(approved: Bool, snapshot_matches: Bool) -> Bool:
  approved && snapshot_matches

def session_control(busy: Bool, picker_open: Bool) -> Bool:
  match busy:
    case True{}: False{}
    case False{}:
      match picker_open:
        case True{}: False{}
        case False{}: True{}

def round_limit() -> Nat:
  6n

def remaining(used: Nat) -> Nat:
  Nat.sub(round_limit(), used)
