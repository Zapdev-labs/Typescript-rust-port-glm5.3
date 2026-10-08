// Ported from tsc/internal/core/tristate.go @ ec47d33c23e464a17cdf2475632cba629bee8763

// go:generate npx hereby generate:tristate

// Tristate

/// Tristate mirrors Go's `type Tristate byte`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Tristate {
    #[default]
    TSUnknown = 0,
    TSFalse = 1,
    TSTrue = 2,
}

impl Tristate {
    pub fn from_u8(value: u8) -> Option<Tristate> {
        match value {
            0 => Some(Tristate::TSUnknown),
            1 => Some(Tristate::TSFalse),
            2 => Some(Tristate::TSTrue),
            _ => None,
        }
    }

    pub fn is_true(&self) -> bool {
        *self == Tristate::TSTrue
    }

    pub fn is_true_or_unknown(&self) -> bool {
        *self == Tristate::TSTrue || *self == Tristate::TSUnknown
    }

    pub fn is_false(&self) -> bool {
        *self == Tristate::TSFalse
    }

    pub fn is_false_or_unknown(&self) -> bool {
        *self == Tristate::TSFalse || *self == Tristate::TSUnknown
    }

    pub fn is_unknown(&self) -> bool {
        *self == Tristate::TSUnknown
    }

    pub fn default_if_unknown(&self, value: Tristate) -> Tristate {
        if *self == Tristate::TSUnknown {
            return value;
        }
        *self
    }

    // PORT: Go implements json.Unmarshaler/ json.Marshaler on Tristate; the
    // port keeps the same byte-level behavior as plain methods (the crate
    // does not depend on serde — SPEC §5.11).
    pub fn unmarshal_json(&mut self, data: &[u8]) {
        match data {
            b"true" => *self = Tristate::TSTrue,
            b"false" => *self = Tristate::TSFalse,
            _ => *self = Tristate::TSUnknown,
        }
    }

    // PORT: Go returns ([]byte, error); the port cannot fail, so it returns
    // just the bytes ("true" / "false" / "null").
    pub fn marshal_json(&self) -> Vec<u8> {
        match self {
            Tristate::TSTrue => b"true".to_vec(),
            Tristate::TSFalse => b"false".to_vec(),
            Tristate::TSUnknown => b"null".to_vec(),
        }
    }
}

pub fn bool_to_tristate(b: bool) -> Tristate {
    if b {
        return Tristate::TSTrue;
    }
    Tristate::TSFalse
}
