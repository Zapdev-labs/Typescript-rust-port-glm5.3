// Port of tsc/internal/testrunner/options_generated.go @
// ec47d33c23e464a17cdf2475632cba629bee8763 — the compiler vary-by option
// set — extended with the conformance twin's option-marshaling table.
//
// The marshaling table maps each vary-by directive name to the canonical
// (camelCase) tsconfig `compilerOptions` name plus the value kind, so the
// per-case synthetic tsconfig carries the case's own options. The table is
// GENERATED (regenerate with `go run genoptions.go` in
// rust/bench/go-driver, which cross-checks it against Go's
// tsoptions.OptionsDeclarations) and embedded IDENTICALLY in both
// conformance drivers (this crate and rust/bench/go-driver/options_table.go)
// — byte-identical config text is the twin's by-construction guarantee.
//
// PORT notes:
//   - Only the vary-by surface marshals into the synthetic tsconfig:
//     options outside the set (e.g. `lib`, `paths`, `baseUrl`) gate
//     checker/emit behavior or need list/path marshaling and land with the
//     options/checker waves. Both drivers drop them identically.
//   - Vary-by names with no compiler-option declaration ("all", "init",
//     "version" — CLI-only or harness-only names) appear in Go's generated
//     set but not in OptionsDeclarations; the generator drops them from the
//     marshaling table (they never appear in real case files).
//   - Boolean values other than true/false marshal as JSON strings — the
//     tsoptions parsers on both sides then report the identical config
//     error (parity of errors, by construction).

use tsc_testutil::harness::{HARNESS_OPTIONS, TestConfiguration};

/// The value kinds the marshaling table discriminates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OptKind {
    Boolean,
    Enum,
}

/// One marshaling table entry: directive name → (tsconfig name, kind).
type OptEntry = (&'static str, &'static str, OptKind);

/// Go: `var compilerVaryBy = collections.NewSetFromItems(...)` — the
/// generated vary-by set (membership test).
pub fn compiler_vary_by(name: &str) -> bool {
    OPTION_TABLE.iter().any(|(directive, _, _)| *directive == name)
}

/// The generated marshaling table (88 entries: the 91 vary-by names minus
/// the CLI-only trio "all"/"init"/"version" — see the module header).
static OPTION_TABLE: &[OptEntry] = &[
    ("allowarbitraryextensions", "allowArbitraryExtensions", OptKind::Boolean),
    ("allowimportingtsextensions", "allowImportingTsExtensions", OptKind::Boolean),
    ("allowjs", "allowJs", OptKind::Boolean),
    ("allowsyntheticdefaultimports", "allowSyntheticDefaultImports", OptKind::Boolean),
    ("allowumdglobalaccess", "allowUmdGlobalAccess", OptKind::Boolean),
    ("allowunreachablecode", "allowUnreachableCode", OptKind::Boolean),
    ("allowunusedlabels", "allowUnusedLabels", OptKind::Boolean),
    ("alwaysstrict", "alwaysStrict", OptKind::Boolean),
    ("assumechangesonlyaffectdirectdependencies", "assumeChangesOnlyAffectDirectDependencies", OptKind::Boolean),
    ("checkjs", "checkJs", OptKind::Boolean),
    ("composite", "composite", OptKind::Boolean),
    ("declaration", "declaration", OptKind::Boolean),
    ("declarationmap", "declarationMap", OptKind::Boolean),
    ("deduplicatepackages", "deduplicatePackages", OptKind::Boolean),
    ("diagnostics", "diagnostics", OptKind::Boolean),
    ("disablereferencedprojectload", "disableReferencedProjectLoad", OptKind::Boolean),
    ("disablesizelimit", "disableSizeLimit", OptKind::Boolean),
    ("disablesolutionsearching", "disableSolutionSearching", OptKind::Boolean),
    ("disablesourceofprojectreferenceredirect", "disableSourceOfProjectReferenceRedirect", OptKind::Boolean),
    ("downleveliteration", "downlevelIteration", OptKind::Boolean),
    ("emitbom", "emitBOM", OptKind::Boolean),
    ("emitdeclarationonly", "emitDeclarationOnly", OptKind::Boolean),
    ("emitdecoratormetadata", "emitDecoratorMetadata", OptKind::Boolean),
    ("erasablesyntaxonly", "erasableSyntaxOnly", OptKind::Boolean),
    ("esmoduleinterop", "esModuleInterop", OptKind::Boolean),
    ("exactoptionalpropertytypes", "exactOptionalPropertyTypes", OptKind::Boolean),
    ("experimentaldecorators", "experimentalDecorators", OptKind::Boolean),
    ("explainfiles", "explainFiles", OptKind::Boolean),
    ("extendeddiagnostics", "extendedDiagnostics", OptKind::Boolean),
    ("forceconsistentcasinginfilenames", "forceConsistentCasingInFileNames", OptKind::Boolean),
    ("importhelpers", "importHelpers", OptKind::Boolean),
    ("incremental", "incremental", OptKind::Boolean),
    ("inlinesourcemap", "inlineSourceMap", OptKind::Boolean),
    ("inlinesources", "inlineSources", OptKind::Boolean),
    ("isolateddeclarations", "isolatedDeclarations", OptKind::Boolean),
    ("isolatedmodules", "isolatedModules", OptKind::Boolean),
    ("jsx", "jsx", OptKind::Enum),
    ("libreplacement", "libReplacement", OptKind::Boolean),
    ("listemittedfiles", "listEmittedFiles", OptKind::Boolean),
    ("listfiles", "listFiles", OptKind::Boolean),
    ("module", "module", OptKind::Enum),
    ("moduledetection", "moduleDetection", OptKind::Enum),
    ("moduleresolution", "moduleResolution", OptKind::Enum),
    ("newline", "newLine", OptKind::Enum),
    ("nocheck", "noCheck", OptKind::Boolean),
    ("noemit", "noEmit", OptKind::Boolean),
    ("noemithelpers", "noEmitHelpers", OptKind::Boolean),
    ("noemitonerror", "noEmitOnError", OptKind::Boolean),
    ("noerrortruncation", "noErrorTruncation", OptKind::Boolean),
    ("nofallthroughcasesinswitch", "noFallthroughCasesInSwitch", OptKind::Boolean),
    ("noimplicitany", "noImplicitAny", OptKind::Boolean),
    ("noimplicitoverride", "noImplicitOverride", OptKind::Boolean),
    ("noimplicitreturns", "noImplicitReturns", OptKind::Boolean),
    ("noimplicitthis", "noImplicitThis", OptKind::Boolean),
    ("nolib", "noLib", OptKind::Boolean),
    ("nopropertyaccessfromindexsignature", "noPropertyAccessFromIndexSignature", OptKind::Boolean),
    ("noresolve", "noResolve", OptKind::Boolean),
    ("nouncheckedindexedaccess", "noUncheckedIndexedAccess", OptKind::Boolean),
    ("nouncheckedsideeffectimports", "noUncheckedSideEffectImports", OptKind::Boolean),
    ("nounusedlocals", "noUnusedLocals", OptKind::Boolean),
    ("nounusedparameters", "noUnusedParameters", OptKind::Boolean),
    ("preserveconstenums", "preserveConstEnums", OptKind::Boolean),
    ("preservesymlinks", "preserveSymlinks", OptKind::Boolean),
    ("preservewatchoutput", "preserveWatchOutput", OptKind::Boolean),
    ("pretty", "pretty", OptKind::Boolean),
    ("quiet", "quiet", OptKind::Boolean),
    ("removecomments", "removeComments", OptKind::Boolean),
    ("resolvejsonmodule", "resolveJsonModule", OptKind::Boolean),
    ("resolvepackagejsonexports", "resolvePackageJsonExports", OptKind::Boolean),
    ("resolvepackagejsonimports", "resolvePackageJsonImports", OptKind::Boolean),
    ("rewriterelativeimportextensions", "rewriteRelativeImportExtensions", OptKind::Boolean),
    ("singlethreaded", "singleThreaded", OptKind::Boolean),
    ("skipdefaultlibcheck", "skipDefaultLibCheck", OptKind::Boolean),
    ("skiplibcheck", "skipLibCheck", OptKind::Boolean),
    ("sourcemap", "sourceMap", OptKind::Boolean),
    ("stabletypeordering", "stableTypeOrdering", OptKind::Boolean),
    ("strict", "strict", OptKind::Boolean),
    ("strictbindcallapply", "strictBindCallApply", OptKind::Boolean),
    ("strictbuiltiniteratorreturn", "strictBuiltinIteratorReturn", OptKind::Boolean),
    ("strictfunctiontypes", "strictFunctionTypes", OptKind::Boolean),
    ("strictnullchecks", "strictNullChecks", OptKind::Boolean),
    ("strictpropertyinitialization", "strictPropertyInitialization", OptKind::Boolean),
    ("stripinternal", "stripInternal", OptKind::Boolean),
    ("target", "target", OptKind::Enum),
    ("traceresolution", "traceResolution", OptKind::Boolean),
    ("usedefineforclassfields", "useDefineForClassFields", OptKind::Boolean),
    ("useunknownincatchvariables", "useUnknownInCatchVariables", OptKind::Boolean),
    ("verbatimmodulesyntax", "verbatimModuleSyntax", OptKind::Boolean),
];

/// Go: `compilerBaselineRegex = regexp.MustCompile("\\.tsx?$")` (the
/// compiler_runner file-level var).
pub fn is_compiler_baseline_path(path: &str) -> bool {
    path.ends_with(".ts") || path.ends_with(".tsx")
}

/// Marshals the case's test configuration into the synthetic tsconfig's
/// `compilerOptions` JSON object body (no braces): entries in sorted
/// directive order, values per the table kind. Harness directives and
/// non-vary-by options are dropped (see the module header).
pub fn marshal_compiler_options(config: &TestConfiguration, out: &mut String) {
    let mut first = true;
    for (name, value) in config {
        if HARNESS_OPTIONS.contains(&name.as_str()) {
            continue;
        }
        let Some((_, tsconfig_name, kind)) = OPTION_TABLE.iter().find(|(directive, _, _)| directive == name) else {
            continue;
        };
        if !first {
            out.push(',');
        }
        first = false;
        out.push('"');
        out.push_str(tsconfig_name);
        out.push_str("\":");
        match kind {
            OptKind::Boolean => {
                let lowered = value.trim().to_lowercase();
                if lowered == "true" {
                    out.push_str("true");
                } else if lowered == "false" {
                    out.push_str("false");
                } else {
                    // see the module header: identical config-parse errors
                    // on both drivers
                    out.push('"');
                    json_escape(value.trim(), out);
                    out.push('"');
                }
            }
            OptKind::Enum => {
                out.push('"');
                json_escape(value.trim(), out);
                out.push('"');
            }
        }
    }
}

/// The minimal, deliberately-shared JSON string escaper (the driver
/// contract's rules: `"` and `\` and control characters only — identical
/// bytes on both conformance drivers).
pub fn json_escape(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(pairs: &[(&str, &str)]) -> TestConfiguration {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn vary_by_membership_matches_the_generated_set() {
        assert!(compiler_vary_by("target"));
        assert!(compiler_vary_by("verbatimmodulesyntax"));
        assert!(!compiler_vary_by("lib"));
        assert!(!compiler_vary_by("all"));
        assert!(!compiler_vary_by("init"));
        assert!(!compiler_vary_by("version"));
        assert!(!compiler_vary_by("currentdirectory"));
    }

    #[test]
    fn table_has_the_generated_entry_count() {
        assert_eq!(OPTION_TABLE.len(), 88);
        // the six enum kinds (verified against the generator's output)
        assert_eq!(OPTION_TABLE.iter().filter(|(_, _, k)| *k == OptKind::Enum).count(), 6);
    }

    #[test]
    fn marshals_sorted_options_with_kinds() {
        let mut out = String::new();
        marshal_compiler_options(
            &config(&[("target", "esnext"), ("nolib", "true"), ("alwaysstrict", "false")]),
            &mut out,
        );
        assert_eq!(out, r#""alwaysStrict":false,"noLib":true,"target":"esnext""#);
    }

    #[test]
    fn drops_harness_directives_and_unknown_options() {
        let mut out = String::new();
        marshal_compiler_options(
            &config(&[("currentdirectory", "/base"), ("filename", "x.ts"), ("lib", "es5"), ("strict", "true")]),
            &mut out,
        );
        assert_eq!(out, r#""strict":true"#);
    }

    #[test]
    fn non_boolean_values_stay_strings_for_identical_parse_errors() {
        let mut out = String::new();
        marshal_compiler_options(&config(&[("strict", "maybe")]), &mut out);
        assert_eq!(out, r#""strict":"maybe""#);
    }

    #[test]
    fn escapes_json_strings() {
        let mut out = String::new();
        json_escape("a\"b\\c\u{1}", &mut out);
        assert_eq!(out, "a\\\"b\\\\c\\u0001");
    }
}
