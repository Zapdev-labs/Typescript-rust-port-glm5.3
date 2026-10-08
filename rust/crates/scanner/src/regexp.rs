// Ported from tsc/internal/scanner/regexp.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// Go name mapping:
//   regularExpressionFlags (type)   → plain `i32` (newtypes add nothing here;
//                                     the bits never escape the package)
//   charCodeToRegExpFlag (map)       → [`char_code_to_reg_exp_flag`] (match)
//   regExpFlagToFirstAvailableLanguageVersion (map)
//                                    → [`reg_exp_flag_first_available_language_version`]
//   regExpParser (struct)            → [`RegExpParser`] (holds `&mut Scanner`,
//                                     mirroring the Go `scanner *Scanner` field)
//   checkRegularExpressionFlagAvailability lives on Scanner (scanner.rs),
//                                     exactly as in Go
//
// PORT (byte strings): the Go character-atom helpers (`scanClassAtom`,
// `scanCharacterEscape`, `scanClassSetCharacter`, `scanSourceCharacter`, ...)
// pass around Go `string` values. In non-Unicode mode `scanSourceCharacter`
// splits non-BMP runes into lone surrogate halves via
// `stringutil.EncodeJSStringRune`, which the Rust port stores as the CESU-8
// sentinel bytes (invalid UTF-8 — see the `scanner` module header). The
// character atoms here are therefore `Vec<u8>`, compared and decoded with the
// same byte semantics as Go strings.

use std::borrow::Cow;

use tsc_core::options_generated::ScriptTarget;
use tsc_stringutil as stringutil;

use tsc_diagnostics::{
    A_CHARACTER_CLASS_MUST_NOT_CONTAIN_A_RESERVED_DOUBLE_PUNCTUATOR_DID_YOU_MEAN_TO_ESCAPE_IT_WITH_BACKSLASH,
    A_CHARACTER_CLASS_RANGE_MUST_NOT_BE_BOUNDED_BY_ANOTHER_CHARACTER_CLASS,
    ANYTHING_THAT_WOULD_POSSIBLY_MATCH_MORE_THAN_A_SINGLE_CHARACTER_IS_INVALID_INSIDE_A_NEGATED_CHARACTER_CLASS,
    ANY_UNICODE_PROPERTY_THAT_WOULD_POSSIBLY_MATCH_MORE_THAN_A_SINGLE_CHARACTER_IS_ONLY_AVAILABLE_WHEN_THE_UNICODE_SETS_V_FLAG_IS_SET,
    DID_YOU_MEAN_0, DUPLICATE_NAMED_CAPTURING_GROUPS_ARE_ONLY_AVAILABLE_WHEN_TARGETING_0_OR_LATER,
    DUPLICATE_REGULAR_EXPRESSION_FLAG, EXPECTED_A_CAPTURING_GROUP_NAME,
    EXPECTED_A_CLASS_SET_OPERAND, EXPECTED_A_UNICODE_PROPERTY_NAME,
    EXPECTED_A_UNICODE_PROPERTY_NAME_OR_VALUE, EXPECTED_A_UNICODE_PROPERTY_VALUE,
    INCOMPLETE_QUANTIFIER_DIGIT_EXPECTED,
    NAMED_CAPTURING_GROUPS_ARE_ONLY_AVAILABLE_WHEN_TARGETING_ES2018_OR_LATER,
    NAMED_CAPTURING_GROUPS_WITH_THE_SAME_NAME_MUST_BE_MUTUALLY_EXCLUSIVE_TO_EACH_OTHER,
    NUMBERS_OUT_OF_ORDER_IN_QUANTIFIER, OPERATORS_MUST_NOT_BE_MIXED_WITHIN_A_CHARACTER_CLASS_WRAP_IT_IN_A_NESTED_CLASS_INSTEAD,
    RANGE_OUT_OF_ORDER_IN_CHARACTER_CLASS,
    REGULAR_EXPRESSION_PATTERN_MODIFIERS_ARE_ONLY_AVAILABLE_WHEN_TARGETING_0_OR_LATER,
    SUBPATTERN_FLAGS_MUST_BE_PRESENT_WHEN_THERE_IS_A_MINUS_SIGN,
    THERE_IS_NO_CAPTURING_GROUP_NAMED_0_IN_THIS_REGULAR_EXPRESSION,
    THERE_IS_NOTHING_AVAILABLE_FOR_REPETITION,
    THIS_BACKREFERENCE_REFERS_TO_A_GROUP_THAT_DOES_NOT_EXIST_THERE_ARE_NO_CAPTURING_GROUPS_IN_THIS_REGULAR_EXPRESSION,
    THIS_BACKREFERENCE_REFERS_TO_A_GROUP_THAT_DOES_NOT_EXIST_THERE_ARE_ONLY_0_CAPTURING_GROUPS_IN_THIS_REGULAR_EXPRESSION,
    THIS_REGULAR_EXPRESSION_FLAG_CANNOT_BE_TOGGLED_WITHIN_A_SUBPATTERN,
    UNDETERMINED_CHARACTER_ESCAPE, UNEXPECTED_0_DID_YOU_MEAN_TO_ESCAPE_IT_WITH_BACKSLASH,
    UNKNOWN_REGULAR_EXPRESSION_FLAG, UNKNOWN_UNICODE_PROPERTY_NAME,
    UNKNOWN_UNICODE_PROPERTY_NAME_OR_VALUE, UNKNOWN_UNICODE_PROPERTY_VALUE,
    UNICODE_PROPERTY_VALUE_EXPRESSIONS_ARE_ONLY_AVAILABLE_WHEN_THE_UNICODE_U_FLAG_OR_THE_UNICODE_SETS_V_FLAG_IS_SET,
    X_0_EXPECTED, X_0_MUST_BE_FOLLOWED_BY_A_UNICODE_PROPERTY_VALUE_EXPRESSION_ENCLOSED_IN_BRACES,
    X_C_MUST_BE_FOLLOWED_BY_AN_ASCII_LETTER,
    X_K_MUST_BE_FOLLOWED_BY_A_CAPTURING_GROUP_NAME_ENCLOSED_IN_ANGLE_BRACKETS,
    X_Q_IS_ONLY_AVAILABLE_INSIDE_CHARACTER_CLASS,
    X_Q_MUST_BE_FOLLOWED_BY_STRING_ALTERNATIVES_ENCLOSED_IN_BRACES,
};

use crate::go_shims::{decode_rune_in_string, get_spelling_suggestion_for_strings, script_target_string};
use crate::scanner::{is_word_character, rune_bytes, EscapeSequenceScanningFlags, IdentifierVariant, Scanner};
use crate::unicodeproperties::{
    binary_unicode_properties, binary_unicode_properties_of_strings,
    non_binary_unicode_properties, non_binary_unicode_property_keys,
    values_of_non_binary_unicode_properties,
};

// ────────────────────────────────────────────────────────────────────────────
// Flags
// ────────────────────────────────────────────────────────────────────────────

pub(crate) const REG_EXP_FLAGS_NONE: i32 = 0;
pub(crate) const REG_EXP_FLAGS_HAS_INDICES: i32 = 1 << 0; // d
pub(crate) const REG_EXP_FLAGS_GLOBAL: i32 = 1 << 1; // g
pub(crate) const REG_EXP_FLAGS_IGNORE_CASE: i32 = 1 << 2; // i
pub(crate) const REG_EXP_FLAGS_MULTILINE: i32 = 1 << 3; // m
pub(crate) const REG_EXP_FLAGS_DOT_ALL: i32 = 1 << 4; // s
pub(crate) const REG_EXP_FLAGS_UNICODE: i32 = 1 << 5; // u
pub(crate) const REG_EXP_FLAGS_UNICODE_SETS: i32 = 1 << 6; // v
pub(crate) const REG_EXP_FLAGS_STICKY: i32 = 1 << 7; // y
pub(crate) const REG_EXP_FLAGS_ANY_UNICODE_MODE: i32 = REG_EXP_FLAGS_UNICODE | REG_EXP_FLAGS_UNICODE_SETS;
pub(crate) const REG_EXP_FLAGS_MODIFIERS: i32 =
    REG_EXP_FLAGS_IGNORE_CASE | REG_EXP_FLAGS_MULTILINE | REG_EXP_FLAGS_DOT_ALL;

/// Go: `var charCodeToRegExpFlag = map[rune]regularExpressionFlags{...}`.
pub(crate) fn char_code_to_reg_exp_flag(ch: i32) -> Option<i32> {
    Some(match ch {
        0x64 /* 'd' */ => REG_EXP_FLAGS_HAS_INDICES,
        0x67 /* 'g' */ => REG_EXP_FLAGS_GLOBAL,
        0x69 /* 'i' */ => REG_EXP_FLAGS_IGNORE_CASE,
        0x6D /* 'm' */ => REG_EXP_FLAGS_MULTILINE,
        0x73 /* 's' */ => REG_EXP_FLAGS_DOT_ALL,
        0x75 /* 'u' */ => REG_EXP_FLAGS_UNICODE,
        0x76 /* 'v' */ => REG_EXP_FLAGS_UNICODE_SETS,
        0x79 /* 'y' */ => REG_EXP_FLAGS_STICKY,
        _ => return None,
    })
}

/// Go: `var regExpFlagToFirstAvailableLanguageVersion = map[...]core.ScriptTarget{...}`.
pub(crate) fn reg_exp_flag_first_available_language_version(flag: i32) -> Option<ScriptTarget> {
    match flag {
        REG_EXP_FLAGS_HAS_INDICES => Some(ScriptTarget::ES2022),
        REG_EXP_FLAGS_DOT_ALL => Some(ScriptTarget::ES2018),
        REG_EXP_FLAGS_UNICODE_SETS => Some(ScriptTarget::ES2024),
        _ => None,
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Parser state
// ────────────────────────────────────────────────────────────────────────────

// PORT: Go defines `classSetExpressionTypeUnknown` and
// `classSetExpressionTypeClassUnion` (regexp.go:60-61) but never constructs
// either — the enum is kept complete for structural parity with Go.
#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum ClassSetExpressionType {
    Unknown,
    ClassUnion,
    ClassIntersection,
    ClassSubtraction,
}

/// Go: `type groupNameReference struct`.
struct GroupNameReference {
    pos: usize,
    end: usize,
    name: Vec<u8>,
}

/// Go: `type decimalEscapeValue struct` (`value int` — 64-bit in Go).
struct DecimalEscapeValue {
    pos: usize,
    end: usize,
    value: i64,
}

const RUNE_ERROR: i32 = 0xFFFD;

/// Go: `type regExpParser struct`. The `scanner *Scanner` field becomes
/// `&'p mut Scanner<'a>`; everything else is a direct translation.
pub(crate) struct RegExpParser<'p, 'a> {
    scanner: &'p mut Scanner<'a>,
    end: usize,
    /// Go declares `regExpFlags` (regexp.go:81) but never reads or writes
    /// it; mirrored for structural parity.
    #[allow(dead_code)]
    reg_exp_flags: i32,
    any_unicode_mode: bool,
    unicode_sets_mode: bool,
    annex_b: bool,

    any_unicode_mode_or_non_annex_b: bool,
    named_capture_groups: bool,

    // See scanClassSetExpression.
    may_contain_strings: bool,
    // The number of all (named and unnamed) capturing groups defined in the regex.
    number_of_capturing_groups: i64,
    // All named capturing groups defined in the regex.
    group_specifiers: rustc_hash::FxHashSet<Vec<u8>>,
    // All references to named capturing groups in the regex.
    group_name_references: Vec<GroupNameReference>,
    // All numeric backreferences within the regex.
    decimal_escapes: Vec<DecimalEscapeValue>,
    // A stack of scopes for named capturing groups. See scanGroupName.
    named_capturing_groups: Vec<rustc_hash::FxHashSet<Vec<u8>>>,

    // pendingLowSurrogate holds the low surrogate to emit on the next
    // scanSourceCharacter call when Corsa has to split a non-BMP rune into
    // UTF-16 surrogate code units in non-unicode mode. Strada did not need
    // this bookkeeping because its source text was already indexed as UTF-16.
    pending_low_surrogate: i32,
}

impl<'p, 'a> RegExpParser<'p, 'a> {
    /// The construction site in `Scanner::reScanSlashToken` (Go builds the
    /// struct literal there; the equivalent fields are derived here).
    pub(crate) fn new(
        scanner: &'p mut Scanner<'a>,
        end: usize,
        reg_exp_flags: i32,
        named_capture_groups: bool,
    ) -> Self {
        RegExpParser {
            scanner,
            end,
            reg_exp_flags,
            any_unicode_mode: reg_exp_flags & REG_EXP_FLAGS_ANY_UNICODE_MODE != 0,
            unicode_sets_mode: reg_exp_flags & REG_EXP_FLAGS_UNICODE_SETS != 0,
            annex_b: true,
            any_unicode_mode_or_non_annex_b: false,
            named_capture_groups,
            may_contain_strings: false,
            number_of_capturing_groups: 0,
            group_specifiers: rustc_hash::FxHashSet::default(),
            group_name_references: Vec::new(),
            decimal_escapes: Vec::new(),
            named_capturing_groups: Vec::new(),
            pending_low_surrogate: 0,
        }
    }

    fn pos(&self) -> usize {
        self.scanner.state.pos
    }

    fn inc_pos(&mut self, n: i32) {
        self.scanner.state.pos = (self.scanner.state.pos as i32 + n) as usize;
    }

    fn char(&self) -> i32 {
        self.scanner.char_()
    }

    fn char_at(&self, pos: usize) -> i32 {
        self.scanner.char_at(pos - self.pos())
    }

    fn error(&mut self, msg: &'static tsc_diagnostics::Message, pos: usize, length: usize, args: &[String]) {
        self.scanner.error_at(msg, pos, length, args);
    }

    fn text(&self) -> &'a [u8] {
        self.scanner.bytes()
    }

    // ────────────────────────────────────────────────────────────────────────
    // Disjunction / Alternative
    // ────────────────────────────────────────────────────────────────────────

    /// Disjunction ::= Alternative ('|' Alternative)*
    fn scan_disjunction(&mut self, is_in_group: bool) {
        // Names defined by any of this disjunction's alternatives. Since
        // exactly one alternative is chosen at runtime, these names are
        // unioned together (rather than intersected) and, when this
        // disjunction is nested inside a group, bubbled up into the enclosing
        // alternative's scope once the group closes. This ensures a name
        // defined inside a nested group (e.g. `(?:(?<a>x))`) is still visible
        // to a duplicate check for a sibling group later in the same
        // enclosing alternative (e.g. `(?:(?<a>x))(?<a>z)`).
        let mut disjunction_names: rustc_hash::FxHashSet<Vec<u8>> = rustc_hash::FxHashSet::default();
        loop {
            self.named_capturing_groups.push(rustc_hash::FxHashSet::default());
            self.scan_alternative(is_in_group);
            let alternative_names = self.named_capturing_groups.pop().unwrap();
            for name in alternative_names {
                disjunction_names.insert(name);
            }
            if self.char() != '|' as i32 {
                break;
            }
            self.inc_pos(1);
        }
        if is_in_group {
            if let Some(parent_scope) = self.named_capturing_groups.last_mut() {
                for name in disjunction_names {
                    parent_scope.insert(name);
                }
            }
        }
    }

    /// Alternative ::= Term*
    /// Term ::= Assertion | Atom Quantifier?
    /// Assertion ::= '^' | '$' | '\b' | '\B' | '(=' Disjunction ')' | '(!' Disjunction ')'
    ///               | '(<=' Disjunction ')' | '(<!' Disjunction ')'
    /// Quantifier ::= QuantifierPrefix '?'?
    /// QuantifierPrefix ::= '*' | '+' | '?' | '{' DecimalDigits (',' DecimalDigits?)? '}'
    /// Atom ::= PatternCharacter | '.' | '\' AtomEscape | CharacterClass
    ///          | '(<' RegExpIdentifierName '>' Disjunction ')' | '(?flags:' Disjunction ')'
    /// CharacterClass ::= unicodeMode ? '[' ClassRanges ']' : '[' ClassSetExpression ']'
    fn scan_alternative(&mut self, is_in_group: bool) {
        let mut is_previous_term_quantifiable = false;
        while self.pos() < self.end {
            let start = self.pos();
            let ch = self.char();
            // Go's `case '{': ... fallthrough; case '*', '+', '?':` collapses
            // to this flag (Rust has no fallthrough).
            let mut run_quantifier_case = false;
            match u8::try_from(ch) {
                Ok(b'^' | b'$') => {
                    self.inc_pos(1);
                    is_previous_term_quantifiable = false;
                }
                Ok(b'\\') => {
                    self.inc_pos(1);
                    match u8::try_from(self.char()) {
                        Ok(b'b' | b'B') => {
                            self.inc_pos(1);
                            is_previous_term_quantifiable = false;
                        }
                        _ => {
                            self.scan_atom_escape();
                            is_previous_term_quantifiable = true;
                        }
                    }
                }
                Ok(b'(') => {
                    self.inc_pos(1);
                    if self.char() == '?' as i32 {
                        self.inc_pos(1);
                        match u8::try_from(self.char()) {
                            Ok(b'=' | b'!') => {
                                self.inc_pos(1);
                                // In Annex B, `(?=Disjunction)` and `(?!Disjunction)` are quantifiable
                                is_previous_term_quantifiable = !self.any_unicode_mode_or_non_annex_b;
                            }
                            Ok(b'<') => {
                                let group_name_start = self.pos();
                                self.inc_pos(1);
                                match u8::try_from(self.char()) {
                                    Ok(b'=' | b'!') => {
                                        self.inc_pos(1);
                                        is_previous_term_quantifiable = false;
                                    }
                                    _ => {
                                        self.scan_group_name(false /* is_reference */);
                                        self.scan_expected_char(b'>');
                                        if self.scanner.language_version() < ScriptTarget::ES2018 {
                                            self.error(
                                                &NAMED_CAPTURING_GROUPS_ARE_ONLY_AVAILABLE_WHEN_TARGETING_ES2018_OR_LATER,
                                                group_name_start,
                                                self.pos() - group_name_start,
                                                &[],
                                            );
                                        }
                                        self.number_of_capturing_groups += 1;
                                        is_previous_term_quantifiable = true;
                                    }
                                }
                            }
                            _ => {
                                let flags_start = self.pos();
                                let set_flags = self.scan_pattern_modifiers(REG_EXP_FLAGS_NONE);
                                if self.char() == '-' as i32 {
                                    self.inc_pos(1);
                                    self.scan_pattern_modifiers(set_flags);
                                    if self.pos() == flags_start + 1 {
                                        self.error(
                                            &SUBPATTERN_FLAGS_MUST_BE_PRESENT_WHEN_THERE_IS_A_MINUS_SIGN,
                                            flags_start,
                                            self.pos() - flags_start,
                                            &[],
                                        );
                                    }
                                }
                                // Modifier characters were consumed, so this is `(?flags:` rather than a plain `(?:` group.
                                if self.pos() != flags_start
                                    && self.scanner.language_version() < ScriptTarget::ES2025
                                {
                                    self.error(
                                        &REGULAR_EXPRESSION_PATTERN_MODIFIERS_ARE_ONLY_AVAILABLE_WHEN_TARGETING_0_OR_LATER,
                                        flags_start,
                                        self.pos() - flags_start,
                                        &[script_target_string(ScriptTarget::ES2025).to_lowercase()],
                                    );
                                }
                                self.scan_expected_char(b':');
                                is_previous_term_quantifiable = true;
                            }
                        }
                    } else {
                        self.number_of_capturing_groups += 1;
                        is_previous_term_quantifiable = true;
                    }
                    self.scan_disjunction(true /* is_in_group */);
                    self.scan_expected_char(b')');
                }
                Ok(b'{') => {
                    self.inc_pos(1);
                    let digits_start = self.pos();
                    self.scan_digits();
                    let min_str = self.scanner.state.token_value.to_vec();
                    if !self.any_unicode_mode_or_non_annex_b && min_str.is_empty() {
                        is_previous_term_quantifiable = true;
                        continue;
                    }
                    if self.char() == ',' as i32 {
                        self.inc_pos(1);
                        self.scan_digits();
                        let max_str = self.scanner.state.token_value.to_vec();
                        if min_str.is_empty() {
                            if !max_str.is_empty() || self.char() == '}' as i32 {
                                self.error(&INCOMPLETE_QUANTIFIER_DIGIT_EXPECTED, digits_start, 0, &[]);
                            } else {
                                self.error(
                                    &UNEXPECTED_0_DID_YOU_MEAN_TO_ESCAPE_IT_WITH_BACKSLASH,
                                    start,
                                    1,
                                    &[rune_string_arg(ch)],
                                );
                                is_previous_term_quantifiable = true;
                                continue;
                            }
                        } else if !max_str.is_empty()
                            && compare_decimal_strings(&min_str, &max_str) == std::cmp::Ordering::Greater
                            && (self.any_unicode_mode_or_non_annex_b || self.char() == '}' as i32)
                        {
                            self.error(
                                &NUMBERS_OUT_OF_ORDER_IN_QUANTIFIER,
                                digits_start,
                                self.pos() - digits_start,
                                &[],
                            );
                        }
                    } else if min_str.is_empty() {
                        if self.any_unicode_mode_or_non_annex_b {
                            self.error(
                                &UNEXPECTED_0_DID_YOU_MEAN_TO_ESCAPE_IT_WITH_BACKSLASH,
                                start,
                                1,
                                &[rune_string_arg(ch)],
                            );
                        }
                        is_previous_term_quantifiable = true;
                        continue;
                    }
                    if self.char() != '}' as i32 {
                        if self.any_unicode_mode_or_non_annex_b {
                            self.error(&X_0_EXPECTED, self.pos(), 0, &["}".to_string()]);
                            self.inc_pos(-1);
                        } else {
                            is_previous_term_quantifiable = true;
                            continue;
                        }
                    }
                    run_quantifier_case = true;
                }
                Ok(b'*' | b'+' | b'?') => {
                    run_quantifier_case = true;
                }
                Ok(b'.') => {
                    self.inc_pos(1);
                    is_previous_term_quantifiable = true;
                }
                Ok(b'[') => {
                    self.inc_pos(1);
                    if self.unicode_sets_mode {
                        self.scan_class_set_expression();
                    } else {
                        self.scan_class_ranges();
                        self.pending_low_surrogate = 0;
                    }
                    self.scan_expected_char(b']');
                    is_previous_term_quantifiable = true;
                }
                Ok(b')') if is_in_group => return,
                Ok(b')' | b']' | b'}') => {
                    // Go: `case ')': if isInGroup { return }; fallthrough` —
                    // a ')' outside a group always reports, ']' and '}' only
                    // in any-unicode/non-Annex-B mode.
                    if self.any_unicode_mode_or_non_annex_b || ch == ')' as i32 {
                        self.error(
                            &UNEXPECTED_0_DID_YOU_MEAN_TO_ESCAPE_IT_WITH_BACKSLASH,
                            self.pos(),
                            1,
                            &[rune_string_arg(ch)],
                        );
                    }
                    self.inc_pos(1);
                    is_previous_term_quantifiable = true;
                }
                Ok(b'/' | b'|') => return,
                _ => {
                    self.scan_source_character();
                    is_previous_term_quantifiable = true;
                }
            }
            if run_quantifier_case {
                self.inc_pos(1);
                if self.char() == '?' as i32 {
                    // Non-greedy
                    self.inc_pos(1);
                }
                if !is_previous_term_quantifiable {
                    self.error(&THERE_IS_NOTHING_AVAILABLE_FOR_REPETITION, start, self.pos() - start, &[]);
                }
                is_previous_term_quantifiable = false;
            }
        }
    }

    /// Go: `func (p *regExpParser) scanPatternModifiers(currFlags regularExpressionFlags) regularExpressionFlags`.
    fn scan_pattern_modifiers(&mut self, mut curr_flags: i32) -> i32 {
        while self.pos() < self.end {
            let (ch, size) = decode_rune_in_string(&self.text()[self.pos()..]);
            if ch == RUNE_ERROR || !is_word_character(ch) {
                break;
            }
            match char_code_to_reg_exp_flag(ch) {
                None => {
                    self.error(&UNKNOWN_REGULAR_EXPRESSION_FLAG, self.pos(), size, &[]);
                }
                Some(flag) => {
                    if curr_flags & flag != 0 {
                        self.error(&DUPLICATE_REGULAR_EXPRESSION_FLAG, self.pos(), size, &[]);
                    } else if flag & REG_EXP_FLAGS_MODIFIERS == 0 {
                        self.error(
                            &THIS_REGULAR_EXPRESSION_FLAG_CANNOT_BE_TOGGLED_WITHIN_A_SUBPATTERN,
                            self.pos(),
                            size,
                            &[],
                        );
                    } else {
                        // Modifier syntax itself requires ES2025, which is
                        // later than any flag that can appear here, so the
                        // group's own diagnostic already covers availability.
                        curr_flags |= flag;
                    }
                }
            }
            self.inc_pos(size as i32);
        }
        curr_flags
    }

    // ────────────────────────────────────────────────────────────────────────
    // Escapes
    // ────────────────────────────────────────────────────────────────────────

    /// AtomEscape ::= DecimalEscape | CharacterClassEscape | CharacterEscape
    ///                | 'k<' RegExpIdentifierName '>'
    fn scan_atom_escape(&mut self) {
        tsc_debug::assert_!(self.pos() > 0 && self.text()[self.pos() - 1] == b'\\');
        match u8::try_from(self.char()) {
            Ok(b'k') => {
                self.inc_pos(1);
                if self.char() == '<' as i32 {
                    self.inc_pos(1);
                    self.scan_group_name(true /* is_reference */);
                    self.scan_expected_char(b'>');
                } else if self.any_unicode_mode_or_non_annex_b || self.named_capture_groups {
                    self.error(
                        &X_K_MUST_BE_FOLLOWED_BY_A_CAPTURING_GROUP_NAME_ENCLOSED_IN_ANGLE_BRACKETS,
                        self.pos() - 2,
                        2,
                        &[],
                    );
                }
            }
            // Go: `case 'q': if p.unicodeSetsMode { ...; return }; fallthrough`
            // — outside \v mode, '\q' takes the default escape path.
            Ok(b'q') if self.unicode_sets_mode => {
                self.inc_pos(1);
                self.error(&X_Q_IS_ONLY_AVAILABLE_INSIDE_CHARACTER_CLASS, self.pos() - 2, 2, &[]);
            }
            _ => {
                if !self.scan_character_class_escape() && !self.scan_decimal_escape() {
                    // Regex literals cannot contain line breaks here, so a
                    // character escape must consume something.
                    let value = self.scan_character_escape(true /* atom_escape */);
                    tsc_debug::assert_!(!value.is_empty());
                }
            }
        }
    }

    /// DecimalEscape ::= [1-9] [0-9]*
    fn scan_decimal_escape(&mut self) -> bool {
        tsc_debug::assert_!(self.pos() > 0 && self.text()[self.pos() - 1] == b'\\');
        let ch = self.char();
        if (b'1' as i32..=b'9' as i32).contains(&ch) {
            let start = self.pos();
            self.scan_digits();
            // Go: `strconv.Atoi`; on overflow the value becomes math.MaxInt.
            let value = std::str::from_utf8(&self.scanner.state.token_value)
                .ok()
                .and_then(|s| s.parse::<i64>().ok())
                .unwrap_or(i64::MAX);
            self.decimal_escapes.push(DecimalEscapeValue {
                pos: start,
                end: self.pos(),
                value,
            });
            true
        } else {
            false
        }
    }

    /// CharacterEscape ::= `c` ControlLetter | IdentityEscape | (other
    /// sequences handled by `scanEscapeSequence`)
    ///
    /// IdentityEscape ::= '^' | '$' | '/' | '\' | '.' | '*' | '+' | '?' | '('
    /// | ')' | '[' | ']' | '{' | '}' | '|' | [~AnyUnicodeMode] (any other
    /// non-identifier characters)
    fn scan_character_escape(&mut self, atom_escape: bool) -> Vec<u8> {
        tsc_debug::assert_!(self.pos() > 0 && self.text()[self.pos() - 1] == b'\\');
        let ch = self.char();
        match ch {
            -1 => {
                self.error(&UNDETERMINED_CHARACTER_ESCAPE, self.pos() - 1, 1, &[]);
                b"\\".to_vec()
            }
            0x63 /* 'c' */ => {
                self.inc_pos(1);
                let ch = self.char();
                if stringutil::is_ascii_letter(crate::as_char(ch)) {
                    self.inc_pos(1);
                    rune_bytes(ch & 0x1f)
                } else if self.any_unicode_mode_or_non_annex_b {
                    self.error(&X_C_MUST_BE_FOLLOWED_BY_AN_ASCII_LETTER, self.pos() - 2, 2, &[]);
                    rune_bytes(ch)
                } else if atom_escape {
                    self.inc_pos(-1);
                    b"\\".to_vec()
                } else {
                    rune_bytes(ch)
                }
            }
            0x5E /* ^ */ | 0x24 /* $ */ | 0x2F /* / */ | 0x5C /* \ */ | 0x2E /* . */
            | 0x2A /* * */ | 0x2B /* + */ | 0x3F /* ? */ | 0x28 /* ( */ | 0x29 /* ) */
            | 0x5B /* [ */ | 0x5D /* ] */ | 0x7B /* { */ | 0x7D /* } */ | 0x7C /* | */ => {
                self.inc_pos(1);
                rune_bytes(ch)
            }
            _ => {
                // back up to include the backslash for scanEscapeSequence
                self.inc_pos(-1);
                let mut flags = EscapeSequenceScanningFlags::REGULAR_EXPRESSION;
                if self.annex_b {
                    flags |= EscapeSequenceScanningFlags::ANNEX_B;
                }
                if self.any_unicode_mode {
                    flags |= EscapeSequenceScanningFlags::ANY_UNICODE_MODE;
                }
                if atom_escape {
                    flags |= EscapeSequenceScanningFlags::ATOM_ESCAPE;
                }
                self.scanner.scan_escape_sequence(flags).into_owned()
            }
        }
    }

    /// Go: `func (p *regExpParser) scanGroupName(isReference bool)`.
    fn scan_group_name(&mut self, is_reference: bool) {
        tsc_debug::assert_!(self.pos() > 0 && self.text()[self.pos() - 1] == b'<');
        self.scanner.state.token_start = self.pos();
        if !self.scanner.scan_identifier(0, IdentifierVariant::RegExpGroupName) {
            self.error(&EXPECTED_A_CAPTURING_GROUP_NAME, self.pos(), 0, &[]);
        } else {
            let name = self.scanner.state.token_value.to_vec();
            let token_start = self.scanner.state.token_start;
            if is_reference {
                self.group_name_references.push(GroupNameReference {
                    pos: token_start,
                    end: self.pos(),
                    name,
                });
            } else if self.named_capturing_groups_contains(&name) {
                self.error(
                    &NAMED_CAPTURING_GROUPS_WITH_THE_SAME_NAME_MUST_BE_MUTUALLY_EXCLUSIVE_TO_EACH_OTHER,
                    token_start,
                    self.pos() - token_start,
                    &[],
                );
            } else {
                // A previous definition can only have come from a mutually
                // exclusive alternative. Below ES2018 the group itself is
                // already reported, so don't stack a second error on it.
                if self.group_specifiers.contains(&name)
                    && self.scanner.language_version() >= ScriptTarget::ES2018
                    && self.scanner.language_version() < ScriptTarget::ES2025
                {
                    self.error(
                        &DUPLICATE_NAMED_CAPTURING_GROUPS_ARE_ONLY_AVAILABLE_WHEN_TARGETING_0_OR_LATER,
                        token_start,
                        self.pos() - token_start,
                        &[script_target_string(ScriptTarget::ES2025).to_lowercase()],
                    );
                }
                if let Some(scope) = self.named_capturing_groups.last_mut() {
                    scope.insert(name.clone());
                }
                self.group_specifiers.insert(name);
            }
        }
    }

    fn named_capturing_groups_contains(&self, name: &[u8]) -> bool {
        self.named_capturing_groups
            .iter()
            .any(|group| group.contains(name))
    }

    fn is_class_content_exit(&self, ch: i32) -> bool {
        ch == ']' as i32 || self.pos() >= self.end
    }

    // ────────────────────────────────────────────────────────────────────────
    // Character classes
    // ────────────────────────────────────────────────────────────────────────

    /// ClassRanges ::= '^'? (ClassAtom ('-' ClassAtom)?)*
    fn scan_class_ranges(&mut self) {
        tsc_debug::assert_!(self.pos() > 0 && self.text()[self.pos() - 1] == b'[');
        self.pending_low_surrogate = 0;
        if self.char() == '^' as i32 {
            self.inc_pos(1);
        }
        while self.pos() < self.end {
            let ch = self.char();
            if self.is_class_content_exit(ch) {
                return;
            }
            let min_start = self.pos();
            let min_character = self.scan_class_atom();
            if self.char() == '-' as i32 {
                self.inc_pos(1);
                let ch = self.char();
                if self.is_class_content_exit(ch) {
                    return;
                }
                if min_character.is_empty() && self.any_unicode_mode_or_non_annex_b {
                    self.error(
                        &A_CHARACTER_CLASS_RANGE_MUST_NOT_BE_BOUNDED_BY_ANOTHER_CHARACTER_CLASS,
                        min_start,
                        self.pos() - 1 - min_start,
                        &[],
                    );
                }
                let max_start = self.pos();
                let max_character = self.scan_class_atom();
                if max_character.is_empty() && self.any_unicode_mode_or_non_annex_b {
                    self.error(
                        &A_CHARACTER_CLASS_RANGE_MUST_NOT_BE_BOUNDED_BY_ANOTHER_CHARACTER_CLASS,
                        max_start,
                        self.pos() - max_start,
                        &[],
                    );
                    continue;
                }
                if min_character.is_empty() {
                    continue;
                }
                let (min_character_value, min_size) = stringutil::decode_js_string_rune(&min_character);
                let (max_character_value, max_size) = stringutil::decode_js_string_rune(&max_character);
                if min_character.len() == min_size
                    && max_character.len() == max_size
                    && min_character_value > max_character_value
                {
                    self.error(&RANGE_OUT_OF_ORDER_IN_CHARACTER_CLASS, min_start, self.pos() - min_start, &[]);
                }
            }
        }
    }

    // Static Semantics: MayContainStrings
    //     ClassUnion: ClassSetOperands.some(ClassSetOperand => ClassSetOperand.MayContainStrings)
    //     ClassIntersection: ClassSetOperands.every(ClassSetOperand => ClassSetOperand.MayContainStrings)
    //     ClassSubtraction: ClassSetOperands[0].MayContainStrings
    //     ClassSetOperand:
    //         || ClassStringDisjunctionContents.MayContainStrings
    //         || CharacterClassEscape.UnicodePropertyValueExpression.LoneUnicodePropertyNameOrValue.MayContainStrings
    //     ClassStringDisjunctionContents: ClassStrings.some(ClassString => ClassString.ClassSetCharacters.length !== 1)
    //     LoneUnicodePropertyNameOrValue: isBinaryUnicodePropertyOfStrings(LoneUnicodePropertyNameOrValue)

    /// ClassSetExpression ::= '^'? (ClassUnion | ClassIntersection | ClassSubtraction)
    /// ClassUnion ::= (ClassSetRange | ClassSetOperand)*
    /// ClassIntersection ::= ClassSetOperand ('&&' ClassSetOperand)+
    /// ClassSubtraction ::= ClassSetOperand ('--' ClassSetOperand)+
    /// ClassSetRange ::= ClassSetCharacter '-' ClassSetCharacter
    /// PORT: Go has the same dead store (`expressionMayContainStrings :=
    /// false`, regexp.go:600) — every path that reads it writes it first.
    #[allow(unused_assignments)]
    fn scan_class_set_expression(&mut self) {
        tsc_debug::assert_!(self.pos() > 0 && self.text()[self.pos() - 1] == b'[');
        let mut is_character_complement = false;
        if self.char() == '^' as i32 {
            self.inc_pos(1);
            is_character_complement = true;
        }
        let mut expression_may_contain_strings = false;

        let ch = self.char();
        if self.is_class_content_exit(ch) {
            return;
        }
        let mut start = self.pos();
        let mut operand: Vec<u8> = Vec::new();
        match self.two_chars() {
            Some(b"--") | Some(b"&&") => {
                self.error(&EXPECTED_A_CLASS_SET_OPERAND, self.pos(), 0, &[]);
                self.may_contain_strings = false;
            }
            _ => {
                operand = self.scan_class_set_operand();
            }
        }
        // Go: `switch p.char() { case '-': if ... { ...; return }; case '&':
        // if ... { ...; return }; default: ... }` — the guarded arms that do
        // not match fall through to the default body.
        if self.char() == '-' as i32 && self.pos() + 1 < self.end && self.char_at(self.pos() + 1) == '-' as i32
        {
            if is_character_complement && self.may_contain_strings {
                self.error(
                    &ANYTHING_THAT_WOULD_POSSIBLY_MATCH_MORE_THAN_A_SINGLE_CHARACTER_IS_INVALID_INSIDE_A_NEGATED_CHARACTER_CLASS,
                    start,
                    self.pos() - start,
                    &[],
                );
            }
            expression_may_contain_strings = self.may_contain_strings;
            self.scan_class_set_sub_expression(ClassSetExpressionType::ClassSubtraction);
            self.may_contain_strings = !is_character_complement && expression_may_contain_strings;
            return;
        }
        if self.char() == '&' as i32 && self.pos() + 1 < self.end && self.char_at(self.pos() + 1) == '&' as i32
        {
            self.scan_class_set_sub_expression(ClassSetExpressionType::ClassIntersection);
            if is_character_complement && self.may_contain_strings {
                self.error(
                    &ANYTHING_THAT_WOULD_POSSIBLY_MATCH_MORE_THAN_A_SINGLE_CHARACTER_IS_INVALID_INSIDE_A_NEGATED_CHARACTER_CLASS,
                    start,
                    self.pos() - start,
                    &[],
                );
            }
            expression_may_contain_strings = self.may_contain_strings;
            self.may_contain_strings = !is_character_complement && expression_may_contain_strings;
            return;
        }
        if is_character_complement && self.may_contain_strings {
            self.error(
                &ANYTHING_THAT_WOULD_POSSIBLY_MATCH_MORE_THAN_A_SINGLE_CHARACTER_IS_INVALID_INSIDE_A_NEGATED_CHARACTER_CLASS,
                start,
                self.pos() - start,
                &[],
            );
        }
        expression_may_contain_strings = self.may_contain_strings;

        while self.pos() < self.end {
            let ch = self.char();
            match u8::try_from(ch) {
                Ok(b'-') => {
                    self.inc_pos(1);
                    let ch = self.char();
                    if self.is_class_content_exit(ch) {
                        self.may_contain_strings = !is_character_complement && expression_may_contain_strings;
                        return;
                    }
                    if ch == '-' as i32 {
                        self.inc_pos(1);
                        self.error(
                            &OPERATORS_MUST_NOT_BE_MIXED_WITHIN_A_CHARACTER_CLASS_WRAP_IT_IN_A_NESTED_CLASS_INSTEAD,
                            self.pos() - 2,
                            2,
                            &[],
                        );
                        start = self.pos() - 2;
                        operand = self.text()[start..self.pos()].to_vec();
                        continue;
                    } else {
                        if operand.is_empty() {
                            self.error(
                                &A_CHARACTER_CLASS_RANGE_MUST_NOT_BE_BOUNDED_BY_ANOTHER_CHARACTER_CLASS,
                                start,
                                self.pos() - 1 - start,
                                &[],
                            );
                        }
                        let second_start = self.pos();
                        let second_operand = self.scan_class_set_operand();
                        if is_character_complement && self.may_contain_strings {
                            self.error(
                                &ANYTHING_THAT_WOULD_POSSIBLY_MATCH_MORE_THAN_A_SINGLE_CHARACTER_IS_INVALID_INSIDE_A_NEGATED_CHARACTER_CLASS,
                                second_start,
                                self.pos() - second_start,
                                &[],
                            );
                        }
                        expression_may_contain_strings =
                            expression_may_contain_strings || self.may_contain_strings;
                        if second_operand.is_empty() {
                            self.error(
                                &A_CHARACTER_CLASS_RANGE_MUST_NOT_BE_BOUNDED_BY_ANOTHER_CHARACTER_CLASS,
                                second_start,
                                self.pos() - second_start,
                                &[],
                            );
                        } else if !operand.is_empty() {
                            let (min_character_value, min_size) = stringutil::decode_js_string_rune(&operand);
                            let (max_character_value, max_size) =
                                stringutil::decode_js_string_rune(&second_operand);
                            if operand.len() == min_size
                                && second_operand.len() == max_size
                                && min_character_value > max_character_value
                            {
                                self.error(
                                    &RANGE_OUT_OF_ORDER_IN_CHARACTER_CLASS,
                                    start,
                                    self.pos() - start,
                                    &[],
                                );
                            }
                        }
                    }
                }
                Ok(b'&') if self.pos() + 1 < self.end && self.char_at(self.pos() + 1) == '&' as i32 => {
                    start = self.pos();
                    self.inc_pos(2);
                    self.error(
                        &OPERATORS_MUST_NOT_BE_MIXED_WITHIN_A_CHARACTER_CLASS_WRAP_IT_IN_A_NESTED_CLASS_INSTEAD,
                        self.pos() - 2,
                        2,
                        &[],
                    );
                    if self.char() == '&' as i32 {
                        self.error(
                            &UNEXPECTED_0_DID_YOU_MEAN_TO_ESCAPE_IT_WITH_BACKSLASH,
                            self.pos(),
                            1,
                            &[rune_string_arg(ch)],
                        );
                        self.inc_pos(1);
                    }
                    operand = self.text()[start..self.pos()].to_vec();
                    continue;
                }
                _ => {}
            }
            if self.is_class_content_exit(self.char()) {
                break;
            }
            start = self.pos();
            match self.two_chars() {
                Some(b"--") | Some(b"&&") => {
                    self.error(
                        &OPERATORS_MUST_NOT_BE_MIXED_WITHIN_A_CHARACTER_CLASS_WRAP_IT_IN_A_NESTED_CLASS_INSTEAD,
                        self.pos(),
                        2,
                        &[],
                    );
                    self.inc_pos(2);
                    operand = self.text()[start..self.pos()].to_vec();
                }
                _ => {
                    operand = self.scan_class_set_operand();
                    if is_character_complement && self.may_contain_strings {
                        self.error(
                            &ANYTHING_THAT_WOULD_POSSIBLY_MATCH_MORE_THAN_A_SINGLE_CHARACTER_IS_INVALID_INSIDE_A_NEGATED_CHARACTER_CLASS,
                            start,
                            self.pos() - start,
                            &[],
                        );
                    }
                    expression_may_contain_strings =
                        expression_may_contain_strings || self.may_contain_strings;
                }
            }
        }
        self.may_contain_strings = !is_character_complement && expression_may_contain_strings;
    }

    /// Go: `func (p *regExpParser) scanClassSetSubExpression(expressionType classSetExpressionType)`.
    fn scan_class_set_sub_expression(&mut self, expression_type: ClassSetExpressionType) {
        let mut expression_may_contain_strings = self.may_contain_strings;
        while self.pos() < self.end {
            let ch = self.char();
            if self.is_class_content_exit(ch) {
                break;
            }
            match u8::try_from(ch) {
                Ok(b'-') => {
                    self.inc_pos(1);
                    if self.char() == '-' as i32 {
                        self.inc_pos(1);
                        if expression_type != ClassSetExpressionType::ClassSubtraction {
                            self.error(
                                &OPERATORS_MUST_NOT_BE_MIXED_WITHIN_A_CHARACTER_CLASS_WRAP_IT_IN_A_NESTED_CLASS_INSTEAD,
                                self.pos() - 2,
                                2,
                                &[],
                            );
                        }
                    } else {
                        self.error(
                            &OPERATORS_MUST_NOT_BE_MIXED_WITHIN_A_CHARACTER_CLASS_WRAP_IT_IN_A_NESTED_CLASS_INSTEAD,
                            self.pos() - 1,
                            1,
                            &[],
                        );
                    }
                }
                Ok(b'&') => {
                    self.inc_pos(1);
                    if self.char() == '&' as i32 {
                        self.inc_pos(1);
                        if expression_type != ClassSetExpressionType::ClassIntersection {
                            self.error(
                                &OPERATORS_MUST_NOT_BE_MIXED_WITHIN_A_CHARACTER_CLASS_WRAP_IT_IN_A_NESTED_CLASS_INSTEAD,
                                self.pos() - 2,
                                2,
                                &[],
                            );
                        }
                        if self.char() == '&' as i32 {
                            self.error(
                                &UNEXPECTED_0_DID_YOU_MEAN_TO_ESCAPE_IT_WITH_BACKSLASH,
                                self.pos(),
                                1,
                                &[rune_string_arg(ch)],
                            );
                            self.inc_pos(1);
                        }
                    } else {
                        self.error(
                            &UNEXPECTED_0_DID_YOU_MEAN_TO_ESCAPE_IT_WITH_BACKSLASH,
                            self.pos() - 1,
                            1,
                            &[rune_string_arg(ch)],
                        );
                    }
                }
                _ => match expression_type {
                    ClassSetExpressionType::ClassSubtraction => {
                        self.error(&X_0_EXPECTED, self.pos(), 0, &["--".to_string()]);
                    }
                    ClassSetExpressionType::ClassIntersection => {
                        self.error(&X_0_EXPECTED, self.pos(), 0, &["&&".to_string()]);
                    }
                    _ => {}
                },
            }
            let ch = self.char();
            if self.is_class_content_exit(ch) {
                self.error(&EXPECTED_A_CLASS_SET_OPERAND, self.pos(), 0, &[]);
                break;
            }
            self.scan_class_set_operand();
            if expression_type == ClassSetExpressionType::ClassIntersection {
                expression_may_contain_strings = expression_may_contain_strings && self.may_contain_strings;
            }
        }
        self.may_contain_strings = expression_may_contain_strings;
    }

    /// ClassSetOperand ::= '[' ClassSetExpression ']' | '\' CharacterClassEscape
    ///                    | '\q{' ClassStringDisjunctionContents '}'
    ///                    | ClassSetCharacter
    fn scan_class_set_operand(&mut self) -> Vec<u8> {
        self.may_contain_strings = false;
        match u8::try_from(self.char()) {
            Ok(b'[') => {
                self.inc_pos(1);
                self.scan_class_set_expression();
                self.scan_expected_char(b']');
                Vec::new()
            }
            Ok(b'\\') => {
                self.inc_pos(1);
                if self.scan_character_class_escape() {
                    return Vec::new();
                }
                if self.char() == 'q' as i32 {
                    self.inc_pos(1);
                    if self.char() == '{' as i32 {
                        self.inc_pos(1);
                        self.scan_class_string_disjunction_contents();
                        self.scan_expected_char(b'}');
                        return Vec::new();
                    } else {
                        self.error(
                            &X_Q_MUST_BE_FOLLOWED_BY_STRING_ALTERNATIVES_ENCLOSED_IN_BRACES,
                            self.pos() - 2,
                            2,
                            &[],
                        );
                        return b"q".to_vec();
                    }
                }
                // Go: `p.incPos(-1); fallthrough; default:` — back up over
                // the backslash and scan a class set character.
                self.inc_pos(-1);
                self.scan_class_set_character()
            }
            _ => self.scan_class_set_character(),
        }
    }

    /// ClassStringDisjunctionContents ::= ClassSetCharacter* ('|' ClassSetCharacter*)*
    fn scan_class_string_disjunction_contents(&mut self) {
        tsc_debug::assert_!(self.pos() > 0 && self.text()[self.pos() - 1] == b'{');
        let mut character_count = 0usize;
        while self.pos() < self.end {
            let ch = self.char();
            match u8::try_from(ch) {
                Ok(b'}') => {
                    if character_count != 1 {
                        self.may_contain_strings = true;
                    }
                    return;
                }
                Ok(b'|') => {
                    if character_count != 1 {
                        self.may_contain_strings = true;
                    }
                    self.inc_pos(1);
                    character_count = 0;
                }
                _ => {
                    self.scan_class_set_character();
                    character_count += 1;
                }
            }
        }
    }

    /// ClassSetCharacter ::= SourceCharacter -- ClassSetSyntaxCharacter --
    ///                      ClassSetReservedDoublePunctuator
    ///                      | '\' (CharacterEscape | ClassSetReservedPunctuator | 'b')
    fn scan_class_set_character(&mut self) -> Vec<u8> {
        let ch = self.char();
        if ch == '\\' as i32 {
            self.inc_pos(1);
            let inner_ch = self.char();
            match u8::try_from(inner_ch) {
                Ok(b'b') => {
                    self.inc_pos(1);
                    return b"\x08".to_vec();
                }
                Ok(b'&' | b'-' | b'!' | b'#' | b'%' | b',' | b':' | b';' | b'<' | b'=' | b'>'
                | b'@' | b'`' | b'~') => {
                    self.inc_pos(1);
                    return rune_bytes(inner_ch);
                }
                _ => {
                    return self.scan_character_escape(false /* atom_escape */);
                }
            }
        } else if self.pos() + 1 < self.end && ch == self.char_at(self.pos() + 1) {
            if let Ok(b'&' | b'!' | b'#' | b'%' | b'*' | b'+' | b',' | b'.' | b':' | b';' | b'<'
                | b'=' | b'>' | b'?' | b'@' | b'`' | b'~') = u8::try_from(ch) {
                self.error(
                    &A_CHARACTER_CLASS_MUST_NOT_CONTAIN_A_RESERVED_DOUBLE_PUNCTUATOR_DID_YOU_MEAN_TO_ESCAPE_IT_WITH_BACKSLASH,
                    self.pos(),
                    2,
                    &[],
                );
                self.inc_pos(2);
                let pos = self.pos();
                return self.text()[pos - 2..pos].to_vec();
            }
        }
        match u8::try_from(ch) {
            Ok(b'/' | b'(' | b')' | b'[' | b']' | b'{' | b'}' | b'-' | b'|') => {
                self.error(
                    &UNEXPECTED_0_DID_YOU_MEAN_TO_ESCAPE_IT_WITH_BACKSLASH,
                    self.pos(),
                    1,
                    &[rune_string_arg(ch)],
                );
                self.inc_pos(1);
                rune_bytes(ch)
            }
            _ => self.scan_source_character(),
        }
    }

    /// ClassAtom ::= SourceCharacter but not one of '\' or ']' | '\' ClassEscape
    /// ClassEscape ::= 'b' | '-' | CharacterClassEscape | CharacterEscape
    fn scan_class_atom(&mut self) -> Vec<u8> {
        if self.char() == '\\' as i32 {
            self.inc_pos(1);
            let ch = self.char();
            match u8::try_from(ch) {
                Ok(b'b') => {
                    self.inc_pos(1);
                    b"\x08".to_vec()
                }
                Ok(b'-') => {
                    self.inc_pos(1);
                    rune_bytes(ch)
                }
                _ => {
                    if self.scan_character_class_escape() {
                        return Vec::new();
                    }
                    self.scan_character_escape(false /* atom_escape */)
                }
            }
        } else {
            self.scan_source_character()
        }
    }

    /// CharacterClassEscape ::= 'd' | 'D' | 's' | 'S' | 'w' | 'W'
    ///                           | [+AnyUnicodeMode] ('P' | 'p') '{' UnicodePropertyValueExpression '}'
    fn scan_character_class_escape(&mut self) -> bool {
        tsc_debug::assert_!(self.pos() > 0 && self.text()[self.pos() - 1] == b'\\');
        let mut is_character_complement = false;
        let start = self.pos() - 1;
        let ch = self.char();
        match u8::try_from(ch) {
            Ok(b'd' | b'D' | b's' | b'S' | b'w' | b'W') => {
                self.inc_pos(1);
                return true;
            }
            // Go: `case 'P': isCharacterComplement = true; fallthrough; case 'p':`.
            Ok(b'P') => {
                is_character_complement = true;
            }
            Ok(b'p') => {}
            _ => return false,
        }
        self.inc_pos(1);
        if self.char() == '{' as i32 {
            self.inc_pos(1);
            let property_name_or_value_start = self.pos();
            let property_name_or_value = self.scan_word_characters();
            if self.char() == '=' as i32 {
                let property_name = non_binary_unicode_properties(property_name_or_value);
                if self.pos() == property_name_or_value_start {
                    self.error(&EXPECTED_A_UNICODE_PROPERTY_NAME, self.pos(), 0, &[]);
                } else if property_name.is_none() {
                    self.error(
                        &UNKNOWN_UNICODE_PROPERTY_NAME,
                        property_name_or_value_start,
                        self.pos() - property_name_or_value_start,
                        &[],
                    );
                    let suggestion = self.get_spelling_suggestion_for_unicode_property_name(property_name_or_value);
                    if !suggestion.is_empty() {
                        self.error(
                            &DID_YOU_MEAN_0,
                            property_name_or_value_start,
                            self.pos() - property_name_or_value_start,
                            &[suggestion],
                        );
                    }
                }
                self.inc_pos(1);
                let property_value_start = self.pos();
                let property_value = self.scan_word_characters();
                if self.pos() == property_value_start {
                    self.error(&EXPECTED_A_UNICODE_PROPERTY_VALUE, self.pos(), 0, &[]);
                } else if let Some(property_name) = property_name {
                    if let Some(values) = values_of_non_binary_unicode_properties(property_name) {
                        if !values.has(property_value) {
                            self.error(
                                &UNKNOWN_UNICODE_PROPERTY_VALUE,
                                property_value_start,
                                self.pos() - property_value_start,
                                &[],
                            );
                            let suggestion = self
                                .get_spelling_suggestion_for_unicode_property_value(property_name, property_value);
                            if !suggestion.is_empty() {
                                self.error(
                                    &DID_YOU_MEAN_0,
                                    property_value_start,
                                    self.pos() - property_value_start,
                                    &[suggestion],
                                );
                            }
                        }
                    }
                }
            } else {
                if self.pos() == property_name_or_value_start {
                    self.error(&EXPECTED_A_UNICODE_PROPERTY_NAME_OR_VALUE, self.pos(), 0, &[]);
                } else if binary_unicode_properties_of_strings().has(property_name_or_value) {
                    if !self.unicode_sets_mode {
                        self.error(
                            &ANY_UNICODE_PROPERTY_THAT_WOULD_POSSIBLY_MATCH_MORE_THAN_A_SINGLE_CHARACTER_IS_ONLY_AVAILABLE_WHEN_THE_UNICODE_SETS_V_FLAG_IS_SET,
                            property_name_or_value_start,
                            self.pos() - property_name_or_value_start,
                            &[],
                        );
                    } else if is_character_complement {
                        self.error(
                            &ANYTHING_THAT_WOULD_POSSIBLY_MATCH_MORE_THAN_A_SINGLE_CHARACTER_IS_INVALID_INSIDE_A_NEGATED_CHARACTER_CLASS,
                            property_name_or_value_start,
                            self.pos() - property_name_or_value_start,
                            &[],
                        );
                    } else {
                        self.may_contain_strings = true;
                    }
                } else if !values_of_non_binary_unicode_properties("General_Category")
                    .unwrap()
                    .has(property_name_or_value)
                    && !binary_unicode_properties().has(property_name_or_value)
                {
                    self.error(
                        &UNKNOWN_UNICODE_PROPERTY_NAME_OR_VALUE,
                        property_name_or_value_start,
                        self.pos() - property_name_or_value_start,
                        &[],
                    );
                    let suggestion =
                        self.get_spelling_suggestion_for_unicode_property_name_or_value(property_name_or_value);
                    if !suggestion.is_empty() {
                        self.error(
                            &DID_YOU_MEAN_0,
                            property_name_or_value_start,
                            self.pos() - property_name_or_value_start,
                            &[suggestion],
                        );
                    }
                }
            }
            self.scan_expected_char(b'}');
            if !self.any_unicode_mode {
                self.error(
                    &UNICODE_PROPERTY_VALUE_EXPRESSIONS_ARE_ONLY_AVAILABLE_WHEN_THE_UNICODE_U_FLAG_OR_THE_UNICODE_SETS_V_FLAG_IS_SET,
                    start,
                    self.pos() - start,
                    &[],
                );
            }
        } else if self.any_unicode_mode_or_non_annex_b {
            self.error(
                &X_0_MUST_BE_FOLLOWED_BY_A_UNICODE_PROPERTY_VALUE_EXPRESSION_ENCLOSED_IN_BRACES,
                self.pos() - 2,
                2,
                &[rune_string_arg(ch)],
            );
        } else {
            self.inc_pos(-1);
            return false;
        }
        true
    }

    fn get_spelling_suggestion_for_unicode_property_name(&self, name: &str) -> String {
        get_spelling_suggestion_for_strings(name, non_binary_unicode_property_keys().iter().copied())
    }

    fn get_spelling_suggestion_for_unicode_property_value(
        &self,
        property_name: &str,
        value: &str,
    ) -> String {
        let Some(values) = values_of_non_binary_unicode_properties(property_name) else {
            return String::new();
        };
        get_spelling_suggestion_for_strings(value, values.keys().iter().copied())
    }

    fn get_spelling_suggestion_for_unicode_property_name_or_value(&self, name: &str) -> String {
        let candidates = values_of_non_binary_unicode_properties("General_Category")
            .unwrap()
            .keys()
            .iter()
            .copied()
            .chain(binary_unicode_properties().keys().iter().copied())
            .chain(binary_unicode_properties_of_strings().keys().iter().copied());
        get_spelling_suggestion_for_strings(name, candidates)
    }

    /// Go: `func (p *regExpParser) scanWordCharacters() string`.
    fn scan_word_characters(&mut self) -> &'a str {
        let start = self.pos();
        while self.pos() < self.end {
            let ch = self.char();
            if !is_word_character(ch) {
                break;
            }
            self.inc_pos(1);
        }
        let end = self.pos();
        let text: &'a str = self.scanner.text;
        &text[start..end]
    }

    /// Go: `func (p *regExpParser) scanSourceCharacter() string`.
    fn scan_source_character(&mut self) -> Vec<u8> {
        if self.pos() >= self.end {
            return Vec::new();
        }
        if !self.any_unicode_mode {
            if self.pending_low_surrogate != 0 {
                // Second of two surrogate code units for the same non-BMP
                // character. Now advance past the full UTF-8 sequence (the
                // high surrogate call did not advance).
                let (_, size) = decode_rune_in_string(&self.text()[self.pos()..]);
                self.inc_pos(size as i32);
                let low = self.pending_low_surrogate;
                self.pending_low_surrogate = 0;
                return stringutil::encode_js_string_rune(low);
            }
            let (ch, size) = decode_rune_in_string(&self.text()[self.pos()..]);
            if ch == RUNE_ERROR || size == 0 {
                // Not a valid rune; consume one raw byte.
                self.inc_pos(1);
                let pos = self.pos();
                return vec![self.text()[pos - 1]];
            }
            if (0x10000..=0x10FFFF).contains(&ch) {
                // Non-BMP character: emit the high surrogate first WITHOUT
                // advancing. The low surrogate will be emitted on the next
                // call, which also advances.
                let (high, low) = stringutil::code_point_to_surrogate_pair(ch);
                self.pending_low_surrogate = low;
                return stringutil::encode_js_string_rune(high);
            }
            self.inc_pos(size as i32);
            return rune_bytes(ch);
        }
        let (ch, size) = decode_rune_in_string(&self.text()[self.pos()..]);
        if size == 0 {
            return Vec::new();
        }
        if ch == RUNE_ERROR {
            // Invalid UTF-8; consume the byte to avoid infinite loops.
            self.inc_pos(size as i32);
            return Vec::new();
        }
        self.inc_pos(size as i32);
        rune_bytes(ch)
    }

    fn scan_expected_char(&mut self, ch: u8) {
        if self.char() == ch as i32 {
            self.inc_pos(1);
        } else {
            self.error(&X_0_EXPECTED, self.pos(), 0, &[String::from_utf8_lossy(&[ch]).into_owned()]);
        }
    }

    fn scan_digits(&mut self) {
        let start = self.pos();
        while self.pos() < self.end && stringutil::is_digit(crate::as_char(self.char())) {
            self.inc_pos(1);
        }
        let end = self.pos();
        let bytes = self.scanner.bytes();
        self.scanner.state.token_value = Cow::Borrowed(&bytes[start..end]);
    }

    /// Go: `func (p *regExpParser) twoChars()`-equivalent — the
    /// `p.text()[p.pos():p.pos()+2]` probe (empty when out of range, exactly
    /// as Go's `if p.pos()+1 < p.end` guard produces).
    fn two_chars(&self) -> Option<&'a [u8]> {
        if self.pos() + 1 < self.end {
            Some(&self.text()[self.pos()..self.pos() + 2])
        } else {
            None
        }
    }

    /// Go: `func (p *regExpParser) run()`.
    pub(crate) fn run(&mut self) {
        // Regular expressions are checked more strictly when either in 'u' or
        // 'v' mode, or when not using the looser interpretation of the syntax
        // from ECMA-262 Annex B.
        self.any_unicode_mode_or_non_annex_b = self.any_unicode_mode || !self.annex_b;

        self.scan_disjunction(false /* is_in_group */);

        for reference in std::mem::take(&mut self.group_name_references) {
            if !self.group_specifiers.contains(&reference.name) {
                let name = String::from_utf8_lossy(&reference.name).into_owned();
                self.error(
                    &THERE_IS_NO_CAPTURING_GROUP_NAMED_0_IN_THIS_REGULAR_EXPRESSION,
                    reference.pos,
                    reference.end - reference.pos,
                    std::slice::from_ref(&name),
                );
                if !self.group_specifiers.is_empty() {
                    let candidates: Vec<&str> = self
                        .group_specifiers
                        .iter()
                        .map(|k| std::str::from_utf8(k).unwrap_or(""))
                        .collect();
                    let suggestion = get_spelling_suggestion_for_strings(&name, candidates.into_iter());
                    if !suggestion.is_empty() {
                        self.error(
                            &DID_YOU_MEAN_0,
                            reference.pos,
                            reference.end - reference.pos,
                            &[suggestion],
                        );
                    }
                }
            }
        }
        for escape in std::mem::take(&mut self.decimal_escapes) {
            // Although a DecimalEscape with a value greater than the number
            // of capturing groups is treated as either a
            // LegacyOctalEscapeSequence or an IdentityEscape in Annex B, an
            // error is nevertheless reported since it's most likely a
            // mistake.
            if escape.value > self.number_of_capturing_groups {
                if self.number_of_capturing_groups > 0 {
                    self.error(
                        &THIS_BACKREFERENCE_REFERS_TO_A_GROUP_THAT_DOES_NOT_EXIST_THERE_ARE_ONLY_0_CAPTURING_GROUPS_IN_THIS_REGULAR_EXPRESSION,
                        escape.pos,
                        escape.end - escape.pos,
                        &[self.number_of_capturing_groups.to_string()],
                    );
                } else {
                    self.error(
                        &THIS_BACKREFERENCE_REFERS_TO_A_GROUP_THAT_DOES_NOT_EXIST_THERE_ARE_NO_CAPTURING_GROUPS_IN_THIS_REGULAR_EXPRESSION,
                        escape.pos,
                        escape.end - escape.pos,
                        &[],
                    );
                }
            }
        }
    }
}

/// Go: `func compareDecimalStrings(a string, b string) int`.
fn compare_decimal_strings(a: &[u8], b: &[u8]) -> std::cmp::Ordering {
    fn trim(s: &[u8]) -> &[u8] {
        let mut i = 0;
        while i < s.len() && s[i] == b'0' {
            i += 1;
        }
        &s[i..]
    }
    let a = trim(a);
    let b = trim(b);
    let a = if a.is_empty() { b"0" } else { a };
    let b = if b.is_empty() { b"0" } else { b };
    if a.len() != b.len() {
        return a.len().cmp(&b.len());
    }
    a.cmp(b)
}

/// Go: `string(ch)` in the variadic diagnostic args — the runes reaching
/// these sites are ASCII punctuation (or the -1 EOF sentinel, which encodes
/// as U+FFFD exactly like Go's `string(rune(-1))`).
fn rune_string_arg(ch: i32) -> String {
    String::from_utf8_lossy(&rune_bytes(ch)).into_owned()
}
