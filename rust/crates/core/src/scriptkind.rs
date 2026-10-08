// Ported from tsc/internal/core/scriptkind.go @ ec47d33c23e464a17cdf2475632cba629bee8763

// go:generate npx hereby generate:scriptkind

/// ScriptKind mirrors Go's `type ScriptKind int32`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(i32)]
pub enum ScriptKind {
    #[default]
    Unknown = 0,
    JS = 1,
    JSX = 2,
    TS = 3,
    TSX = 4,

    // Value 5 is reserved (formerly ScriptKindExternal).

    JSON = 6,

    // Value 7 is reserved (formerly ScriptKindDeferred).
}

impl ScriptKind {
    pub fn from_i32(value: i32) -> Option<ScriptKind> {
        match value {
            0 => Some(ScriptKind::Unknown),
            1 => Some(ScriptKind::JS),
            2 => Some(ScriptKind::JSX),
            3 => Some(ScriptKind::TS),
            4 => Some(ScriptKind::TSX),
            6 => Some(ScriptKind::JSON),
            _ => None,
        }
    }
}
