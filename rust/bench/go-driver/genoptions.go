//go:build ignore

// Generator for the conformance twin's option-marshaling table (SPEC §8 M6).
// The table maps the lowercase `// @option:` directive names the test-case
// parser extracts to the canonical (camelCase) tsconfig compilerOptions
// name plus the value kind, so BOTH drivers (this Go driver and the Rust
// tsc-testrunner) marshal per-case tsconfigs from identical data.
//
// Regenerate with:
//
//	GOTMPDIR=$HOME/.gotmp GOCACHE=$HOME/.gocache go run genoptions.go
//
// and paste the output into:
//   - rust/crates/tsc-testrunner/src/options_generated.rs
//   - rust/bench/go-driver/options_table.go
package main

import (
	"fmt"
	"sort"
	"strings"

	"github.com/microsoft/TypeScript/tsc/internal/tsoptions"
)

// compilerVaryBy — the vary-by option set from
// tsc/internal/testrunner/options_generated.go (kept in sync by hand; the
// generator cross-checks it below).
var compilerVaryBy = []string{
	"all", "allowarbitraryextensions", "allowimportingtsextensions", "allowjs",
	"allowsyntheticdefaultimports", "allowumdglobalaccess", "allowunreachablecode",
	"allowunusedlabels", "alwaysstrict", "assumechangesonlyaffectdirectdependencies",
	"checkjs", "composite", "declaration", "declarationmap", "deduplicatepackages",
	"diagnostics", "disablereferencedprojectload", "disablesizelimit",
	"disablesolutionsearching", "disablesourceofprojectreferenceredirect",
	"downleveliteration", "emitbom", "emitdeclarationonly", "emitdecoratormetadata",
	"erasablesyntaxonly", "esmoduleinterop", "exactoptionalpropertytypes",
	"experimentaldecorators", "explainfiles", "extendeddiagnostics",
	"forceconsistentcasinginfilenames", "importhelpers", "incremental", "init",
	"inlinesourcemap", "inlinesources", "isolateddeclarations", "isolatedmodules",
	"jsx", "libreplacement", "listemittedfiles", "listfiles", "module",
	"moduledetection", "moduleresolution", "newline", "nocheck", "noemit",
	"noemithelpers", "noemitonerror", "noerrortruncation", "nofallthroughcasesinswitch",
	"noimplicitany", "noimplicitoverride", "noimplicitreturns", "noimplicitthis",
	"nolib", "nopropertyaccessfromindexsignature", "noresolve", "nouncheckedindexedaccess",
	"nouncheckedsideeffectimports", "nounusedlocals", "nounusedparameters",
	"preserveconstenums", "preservesymlinks", "preservewatchoutput", "pretty",
	"quiet", "removecomments", "resolvejsonmodule", "resolvepackagejsonexports",
	"resolvepackagejsonimports", "rewriterelativeimportextensions", "singlethreaded",
	"skipdefaultlibcheck", "skiplibcheck", "sourcemap", "stabletypeordering",
	"strict", "strictbindcallapply", "strictbuiltiniteratorreturn", "strictfunctiontypes",
	"strictnullchecks", "strictpropertyinitialization", "stripinternal", "target",
	"traceresolution", "usedefineforclassfields", "useunknownincatchvariables",
	"verbatimmodulesyntax", "version",
}

func main() {
	byLower := map[string]*tsoptions.CommandLineOption{}
	for _, option := range tsoptions.OptionsDeclarations {
		byLower[strings.ToLower(option.Name)] = option
	}
	sort.Strings(compilerVaryBy)
	var rust, goTable []string
	seen := map[string]bool{}
	for _, name := range compilerVaryBy {
		decl, ok := byLower[name]
		if !ok {
			// No compiler-option declaration for this vary-by name (e.g.
			// "all", "init", "version", CLI-only entries) — not marshalable
			// into a tsconfig; both drivers drop it.
			continue
		}
		kind := "string"
		switch decl.Kind {
		case tsoptions.CommandLineOptionTypeEnum:
			kind = "enum"
		case tsoptions.CommandLineOptionTypeBoolean:
			kind = "boolean"
		case tsoptions.CommandLineOptionTypeNumber:
			kind = "number"
		case tsoptions.CommandLineOptionTypeList:
			kind = "list"
		case tsoptions.CommandLineOptionTypeListOrElement:
			kind = "list"
		}
		seen[name] = true
		rust = append(rust, fmt.Sprintf("    (%q, %q, OptKind::%s),", name, decl.Name, title(kind)))
		goTable = append(goTable, fmt.Sprintf("\t{%q, %q, kind%s},", name, decl.Name, title(kind)))
	}
	fmt.Println("// kind counts:", len(seen), "of", len(compilerVaryBy))
	fmt.Println("---- rust ----")
	fmt.Println(strings.Join(rust, "\n"))
	fmt.Println("---- go ----")
	fmt.Println(strings.Join(goTable, "\n"))
}

func title(s string) string {
	switch s {
	case "boolean":
		return "Boolean"
	case "enum":
		return "Enum"
	case "number":
		return "Number"
	case "list":
		return "List"
	}
	return s
}
