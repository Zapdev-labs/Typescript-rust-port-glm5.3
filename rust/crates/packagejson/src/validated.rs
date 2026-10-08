// Ported from tsc/internal/packagejson/validated.go @ ec47d33c23e464a17cdf2475632cba629bee8763

/// TypeValidatedField is the surface Go extracts from `Expected[T]` (via
/// interface satisfaction) for consumers that check a package.json field's
/// presence, parse validity, and actual-vs-expected JSON types.
pub trait TypeValidatedField {
    /// IsPresent reports whether the field appeared in the JSON at all.
    fn is_present(&self) -> bool;

    /// IsValid reports whether the field's value parsed as the expected type.
    fn is_valid(&self) -> bool;

    /// ExpectedJSONType is the JSON type the field's Go type maps to.
    fn expected_json_type(&self) -> &'static str;

    /// ActualJSONType is the JSON type the field's raw bytes classify as.
    fn actual_json_type(&self) -> &str;
}
