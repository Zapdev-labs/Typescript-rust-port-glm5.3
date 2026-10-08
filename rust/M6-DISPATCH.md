# M6 — Checker port dispatch plan

The long pole. This doc fixes the execution order for the checker waves and
carries the checker.go slice survey (the 32,723-line core is split into ten
slices, S1-S10). Workers get one wave each; a wave lands green (build, tests,
clippy, twin/regression checks) before the next same-crate wave dispatches.

## Execution order

| # | Wave | Go source | ~LOC | Notes |
|---|------|-----------|-----|-------|
| 0 | Runner (gate tooling) | testrunner/ + testutil/ baseline subset | ~1.5-2k | In flight. Drives the pipeline only as deep as it exists (parse+bind today); `check` stage unwired behind an explicit depth control; CONFORMANCE.md seeds the climb. Extends depth stage-by-stage as waves land. |
| 1 | Periphery | mapper.go + jsdoc.go + exports.go + symboltracker.go + stringer_generated.go | ~1.2k | In flight. TypeMapperId is the todo_shapes shape this wave replaces. |
| 2 | Engines | relater.go + inference.go + tracer.go | ~7.1k | tracer FIRST inside the wave (`--generateTrace` output is the debugging lifeline and must diff-match Go). S6 stubs `isSignatureApplicable`/`inferTypeArguments` permissively until this lands. |
| 3 | S1 entry & expression spine | checker.go 2240-2585, 7539-8060, 14222-14310 | ~4.0k | Dispatch workers + deferred-nodes machinery + diagnostics entries. Port the deferred queue+flag EXACTLY as written (see risks). |
| 4 | S2 getTypeOfSymbol spine | checker.go 16749-19300, 24194-24700 | ~5.5k | + circularity machinery (typeResolutions/resolutionStart) + binding-element typing. |
| 5 | S3 statement/declaration/class/module checks | checker.go 2649-6293, 6937-7530 | ~5.5k | Replaces the IterationTypes/IterationTypesResolver todo_shapes with real structs. |
| 6 | S4 expression leaves I | checker.go 8039-14220 | ~4.5k | Operators, literals, destructuring, this/super, expando/JS-assignment. |
| 7 | S5 type-node family | checker.go 23295-25536 (minus new*) | ~3.5k | getTypeFromTypeNode + the 20 getTypeFrom*TypeNode arms. |
| 8 | S6 symbol/module/call resolution | checker.go 8614-10258, 14472-16750 | ~5.0k | resolveSignature/resolveCall/chooseOverload + resolveEntityName + resolveExternalModule (306 ln — keep the program/module-layer seam). |
| 9 | S7 type construction/union/instantiation | checker.go 25537-26940, 22559-23290, 21019-21046 | ~4.5k | new* ctors + getUnionType family + removeSubtypes (361 ln, perf-critical) + the instantiate* engine + activeMappers. MUST land before S8. |
| 10 | S8 structured-type members & signatures | checker.go 19253-22154 | ~3.5k | resolveStructuredTypeMembers + resolve*Members family (fills S7's stub) + getUnionSignatures. getReturnTypeFromBody stubs the body path until S9/flow. |
| 11 | S9 contextual typing/constraints/widening | checker.go 29448-31523, 16800-19005 partial | ~4.5k | getContextualType* family + getResolvedBaseConstraint + widening + awaited/thenable. |
| 12 | S10 index types/facts/public tail | checker.go 26964-29448, 29604-29860, 31523-32723 | ~3.5k | getIndexedAccessType* + facts + decorator contexts + GetSymbolAtLocation/GetTypeAtLocation/NewEmitResolver. |
| 13 | W3 flow + grammarchecks + services | flow.go + grammarchecks.go + services.go | ~6.2k | flow pairs with S4's narrowing call sites; services.go = the Check entry points (the runner's check stage wires here). |
| 14 | W4 nodebuilder family | nodebuilder*.go + nodecopy.go + printer.go + symbolaccessibility.go + pseudotypenodebuilder.go | ~9k | Type→syntax + type→string (the .types baseline emitters consume this). |
| 15 | W5 jsx + emitresolver + emitsupport | jsx.go + emitresolver.go + emitsupport.go | ~6.5k | LAST — jsx.go's wave lands against S6's resolveCall (shared overload-resolution engine). |

Parallelism rule: waves on the SAME crate go sequential (todo_shapes.rs and
checker.rs are shared edit surfaces); waves on DISJOINT crates (runner,
binder/ast cleanups) can ride along.

## Cycles (contained, deliberate)

- S1 ↔ all leaves: by design — leaves are todo_shapes stubs after S1.
- S2 ↔ S7: `getTypeForVariableLikeDeclaration` needs `getUnionType`; union
  reduction needs `getBaseConstraintOfType`. Keep the constraint family in S2,
  union creation in S7; one stub direction per landing order.
- S7 ↔ S8 (the hottest): land S7 first with `resolveStructuredTypeMembers`
  stubbed (empty members), S8 fills it.
- S6 ↔ relater/inference (wave 2): S6 stubs permissively until the engines land.
- S4 ↔ flow.go (wave 13): narrowing delegates stubbed until flow lands.

---

# checker.go slice survey (the source data for the table above)

File: `tsc/internal/checker/checker.go` — 1,454,076 bytes, 32,723 lines.
Package context: the Go `checker` package splits the TS checker across
`checker.go` (the core), `relater.go` (237k), `flow.go` (115k),
`grammarchecks.go` (105k), `inference.go` (78k), `jsx.go` (67k),
`types.go`/`utilities.go`/`links.go` (already ported in the spike), plus
emitresolver/nodebuilder/etc. **This survey covers checker.go only**; the
sibling files are separate M6 waves (see the table above).

Function count: ~700 `func` declarations (enumerated via prefix-bucketed
ripgrep greps, `^func \(c \*Checker\) <prefix>` partitions plus free
functions).

## 1. FUNCTION-FAMILY INVENTORY

Line spans are `first-def .. last-def` of the family; "biggest" = body size in
lines (next-def-line − def-line, approximate).

### check* — ~230 funcs, span 1558–31781
Sub-groups (each is a natural slice unit):
- **checkAndReportErrorFor\*** (name-resolution diagnostics): 1558–11903, ~12 funcs, tiny.
- **checkSource\*** (file/element dispatch + unreachable): 2240–2528, 7 funcs; biggest `checkSourceElementWorker` @2298 (~138).
- **checkDeferred\***: 2528–2547, 3 funcs (see risks).
- **checkJSDoc\***: 2585–2630, 4 small.
- **Type-node checks** (`checkTypeParameter`, `checkSignatureDeclaration`, `checkTypeReferenceNode`, `checkMappedType`, …): 2649–3430, ~30 small.
- **Statement checks** (`checkBlock/If/For*/Switch/Try/Return…`): 3759–4304, ~20 small; biggest `checkReturnStatement` @4117 (~45).
- **Class/interface/enum/module checks**: 4309–5388, ~30; biggest `checkModuleDeclaration` @5245 (~97).
- **Import/export checks**: 5420–5943, ~15.
- **Variable/decorator checks**: 5947–6293, ~10; biggest `checkVariableLikeDeclaration` @5977 (~173).
- **Iteration entry** `checkIteratedTypeOrElementType` @6296.
- **Alias/unused checks**: 6937–7516, ~15.
- **Expression checks** (`checkExpression*` core + per-kind leaves): 7531–11270, ~70; biggest `checkBinaryLikeExpression` @12575 (~216), `checkFunctionOrConstructorSymbolWorker` @3500 (~232), `checkCallExpression` @8529 (~42).
- **Identifier/property-access/this/binary leaves**: 11278–13100, ~40; biggest `checkIdentifier` @11278 (~160), `checkPropertyAccessExpressionOrQualifiedName` @11494 (~134), `checkPropertyAccessibilityAtLocation` @12024 (~115).
- **Truthiness/instanceof/in/object-literal/expando**: 13109–14150, ~25; biggest `checkObjectLiteral` @13388 (~212).
- **Scattered later checks**: `checkDeclarationInitializer` @17183, `checkRightHandSideOfForOf` @18081, return/yield aggregation @20663–20989, `checkNoTypeArguments` @23744, `checkCrossProductUnion` @27182, `checkComputedPropertyName` @27336, `checkExternalEmitHelpers` @29105, `checkGeneratorInstantiationAssignabilityToReturnType` @30246, `checkAwaitedType` @31781.

Top-5 by body size (whole check* family):
1. `checkFunctionOrConstructorSymbolWorker` @3500 — ~232
2. `checkBinaryLikeExpression` @12575 — ~216
3. `checkObjectLiteral` @13388 — ~212
4. `checkVariableLikeDeclaration` @5977 — ~173
5. `checkIdentifier` @11278 — ~160 (then `checkSourceElementWorker` @2298 ~138)

### get* — ~400 funcs, span 1164–~32100 (the giant family)
Sub-groups:
- **Global type/symbol resolvers** (`getGlobalType`, `getGlobal*Symbol`, resolver closures): 1164–1254, ~15 (thin wrappers over the struct's `getGlobalXxx func()` fields).
- **Name-resolution suggestions** (`getSuggested*`, `getSpellingSuggestion*`): 1777–16299, ~15.
- **Iteration family** (`getIteratedTypeOrElementType`, `getIterationType*` ×20): 6307–6915; biggest `getIterationTypesOfMethod` @6742 (~108).
- **Spread/expando** (`getSpreadType`, `getSpreadSymbol`, `getUnionIndexInfos`): 13361–13858; `getSpreadType` @13641 (~110).
- **Type-of-symbol spine** (`getTypeOfSymbol`, `getTypeOfVariableOrParameterOrProperty`(+`Worker`), `getTypeForVariableLikeDeclaration`, `getTypeOfFuncClassEnumModule`, `getTypeOfAccessors`, `getTypeOfAlias`, `getTypeOfParameter`, `getTypeOfMappedSymbol`, `getWriteTypeOf*`): 16787–19005; biggest `getTypeForVariableLikeDeclaration` @17038 (~199).
- **Constraint family** (`getConstraintOfType/OfTypeParameter/OfConditionalType/OfDistributiveConditionalType`, `getInferredTypeParameterConstraint`, `getBaseConstraintOfType`, `getResolvedBaseConstraint`, `getNextBaseConstraint`): 17434–28292; biggest `getNextBaseConstraint` @28127 (~165), `getResolvedBaseConstraint` @27981 (~146).
- **Declared types** (`getDeclaredTypeOfClassOrInterface`, `getDeclaredTypeOf{Symbol,TypeParameter,Enum,EnumMember,Alias}`): 17706–24628; biggest `getDeclaredTypeOfClassOrInterface` @17706 (~381 — the single largest function in the file).
- **Binding patterns** (`getTypeFromBinding{Pattern,ObjectBindingPattern,ArrayBindingPattern,Element}`, `getBindingElementTypeFromParentType`, `getTypeForBindingElement`): 18087–18444.
- **Widening** (`getWidenedType(WithContext/OfObjectLiteral)`, `getWidenedLiteralType*`): 17284–26049.
- **Structured-type accessors** (`getPropertiesOfType`, `getPropertyOfType`(+Ex), `getSignaturesOfType/OfStructuredType/OfSymbol`, `getIndexInfo*`, `getIndexTypeOfType`, `getBaseTypes`, `getSingleCall*Signature`): 19253–20455.
- **Signature accessors** (`getSignatureFromDeclaration`, `getReturnTypeOfSignature`, `getReturnTypeFromBody`, `getTypePredicateFromBody`, `getDefaultConstructSignatures`, `getUnionSignatures`): 20210–21388; biggest `getReturnTypeFromBody` @20530 (~281).
- **Union/intersection property** (`getUnionOrIntersectionProperty`, `getPropertyOfObjectType`, `getPropertyOfUnionOrIntersectionType`): 21807–22087; biggest `getUnionOrIntersectionProperty` @21832 (~255).
- **Apparent/reduced** (`getApparentType*`, `getReducedType/ApparentType`): 22154–22319.
- **Instantiation** (`getObjectTypeInstantiation`, `getConditionalTypeInstantiation`): 22792–23012; `getObjectTypeInstantiation` @22792 (~181).
- **Type-node family** (`getTypeFromTypeNode(Worker)`, `getTypeFrom*TypeNode` ×20, `getTypeReferenceType`, `getTypeFromClassOrInterfaceReference`, `getTypeArgumentsFromNode`, `getTypeAliasInstantiation`): 23295–25184; biggest `getTypeArgumentsFromNode` @23740 (~252).
- **Union/intersection creation** (`getUnionType(Ex/Worker/FromSortedList)`, `getIntersectionType(Ex)`): 26144–26590; biggest `getUnionTypeFromSortedList` @26270 (~316).
- **Index types** (`getIndexType(Ex)`, `getIndexedAccessType*`, `getPropertyTypeForIndexType`, `getIndexedAccessTypeOrUndefined`): 27214–27757; biggest `getPropertyTypeForIndexType` @27535 (~222).
- **Simplified/normalized** (`getSimplifiedType`, `getSimplifiedIndexedAccessType(Worker)`, `getSimplifiedConditionalType`, `getNormalizedType`): 28399–28621.
- **Contextual typing family** (`getContextualType`, `getContextualTypeFor*` ×20, `getApparentTypeOfContextualType`, `getMatchingUnionConstituentForObjectLiteral`, `getTypeOfPropertyOfContextualType*`): 29891–31340, ~30 medium.
- **Facts/awaited tail** (`getTypeFacts(Worker)`, `getTypeWithFacts`, `getAdjustedTypeWithFacts`, `getAwaitedType*`, `getPromisedTypeOfPromiseEx`): 31523–32000.
- **Tail public API** (`GetSymbolAtLocation` @32134, `GetTypeAtLocation` @32712, `NewEmitResolver` @32716, `GetAliasedSymbol` @32720).

Top-5 by body size (whole get* family):
1. `getDeclaredTypeOfClassOrInterface` @17706 — ~381
2. `getUnionTypeFromSortedList` @26270 — ~316
3. `getReturnTypeFromBody` @20530 — ~281
4. `getUnionOrIntersectionProperty` @21832 — ~255
5. `getTypeArgumentsFromNode` @23740 (~252); then `resolveUnionTypeMembers` @21456 (~250, resolve* family)

### resolve* — ~40 funcs, span 2600–21706
Sub-groups: JSDoc (2600), call resolution (`resolveSignature/resolveCallExpression/resolveNewExpression/resolveTaggedTemplate/resolveDecorator/resolveInstanceof/resolveCall/resolveUntypedCall/resolveErrorCall`, 8660–10158), symbol/alias (`resolveSymbol(Ex)`, `resolveAlias`, `resolveEntityName`, `resolveQualifiedName`, 14697–16277), module resolution (`resolveExternalModuleName(Worker)`, `resolveExternalModule` @15453 (~306), `resolveExternalModuleSymbol`, `resolveESModuleSymbol`, `resolveExportByName`, 14731–15954), members (`resolveStructuredTypeMembers`, `resolveClassOrInterfaceMembers`, `resolveTypeReferenceMembers`, `resolveObjectTypeMembers`, `resolveBaseTypesOfClass` @19624, `resolveBaseTypesOfInterface` @19902, `resolveDeclaredMembers`, `resolveAnonymousTypeMembers` @21054 (~244), `resolveMappedTypeMembers`, `resolveUnionTypeMembers` @21456 (~250), `resolveIntersectionTypeMembers`, 19469–21706), import type (`resolveImportSymbolType`).

### is* / has* — ~200 funcs, span 1966–32468
Mostly 2–15-line predicates over types/symbols/nodes. Clusters: block-scope use checks (1966–2150), node-kind predicates (7275–7512), signature predicates (8588–10564), spread/const-context (13758–13977), readonly/expando (14100–14337), array/tuple type predicates (24005–24084, 25402–25471), context-sensitivity (30044, 31446–31500), awaited/thenable (31933–32024), generic-constraint (32058–32110). No function exceeds ~60 lines; they port with the slice that owns their domain.

### add* / create* / instantiate* / infer* — ~95 funcs, span 1496–31959
- `add*`: diagnostics adds (14246–14503), members/optionality (19036, 20002), union/intersection adds (26295–26792).
- `create*`: name resolvers (1510), call-resolution synthetics (9806–10526), signature instantiation (19732), promise/generator types (20752–20838), type references (23820–23875), array/tuple (25231–25309), awaited (31959).
- `instantiate*`: the type-mapper engine — `instantiateType(Worker)` @22559/22708, `instantiateAnonymousType` @22946, `instantiateMappedType*` @23023–23230, `instantiateSignature(Ex)` @21019, `instantiateSymbol(Table)`, `instantiateList` (generic) @23271, `instantiateContextualType` @31366.
- `infer*` (thin; the real engine is inference.go): `inferTypeArguments` @9615, `inferFromAnnotatedParametersAndReturn` @10470, `inferSignatureInstantiationForOverloadFailure` @9800.

### new* — ~35 funcs, span 14341–31073
Symbol ctors (`newSymbol`, `newParameter`, `newProperty`) 14341–14361; type ctors (`newType`, `newIntrinsicType`, `newLiteralType`, `newUniqueESSymbolType`, `newObjectType`, `newAnonymousType`, `newTypeParameter`, `newUnionType`, `newIntersectionType`, `newIndexedAccessType`, `newTemplateLiteralType`, `newStringMappingType`, `newConditionalType`, `newSubstitutionType`, `newSignature`, `newIndexInfo`) 25537–25797; decorator-context ctors 30956–31073.

### report* — ~30 funcs, span 1149–15235. All small diagnostic emitters.
### remove* / unwrap* / widen* / mark* / push* / pop* / set* / filter* / find* / forEach* / append* / late* / lookup* / assign* / distribute* / apply* / discriminate* / combine* / propagate* / substitute* / pad* / map*/type* (misc) — ~120 funcs scattered 1149–32468
Notables: `removeSubtypes` @26468 (~361 — second largest in file), `removeRedundantSupertypes` @26829 (~110), `removeType` @27128 (~240), `substituteIndexedMappedType` @29839, `mapType`/`filterType`/`someType` helpers (27004–27092 + utilities.go), circularity (`pushTypeResolution` @19165, `popTypeResolution` @19182, `findResolutionCycleStartIndex` @19190), contextual stacks (`pushContextualType` @31425, `pushInferenceContext` @31504), `widenTypeInferredFromInitializer` @17269, `unwrapReturnType` @20792.
### Free functions (no receiver) — ~25, all tiny (1131–5947 region).
### Other receivers — 2 (`(iterationTypes *IterationTypes)`, `(r *IterationTypesResolver)`).

## 2. SLICE PLAN DETAIL (line budgets + stub directions)

| # | Slice | Go line budget | Calls INTO later slices (become stubs) |
|---|-------|---------------|------------------------------------------|
| 1 | Entry & expression spine | ~4.0k (2240–2585, 7539–8060, 14222–14310) | Every per-kind leaf checker (S3–S8), resolveSignature (S6), getTypeOfSymbol spine (S2) |
| 2 | getTypeOfSymbol spine + declared types + circularity | ~5.5k (16749–19300, 24194–24466, 24628–24700) | getUnionType/getIntersectionType (S7), getFlowTypeInConstructor (flow wave), getTypeFromTypeNode (S5), getWidenedTypeWithContext (S9) |
| 3 | Statement/declaration/class/module/import-export checks | ~5.5k (2649–6293, 6937–7530, 18081, 20663–20989) | resolveExternalModule* (S6), getResolvedSignature (S6), relater (wave 2), getSpreadType (S4) |
| 4 | Expression leaves I | ~4.5k (8039–14220) | getResolvedSignature/resolveCall (S6), flow narrowing (W3), getUnionType (S7) |
| 5 | Type-node family | ~3.5k (23295–25536 minus new*) | resolveEntityName (S6), getUnionType/instantiateType (S7), createTypeReference (S7 new*) |
| 6 | Symbol & module resolution + call/overload resolution | ~5.0k (8614–10258, 14472–16750) | isSignatureApplicable/inferTypeArguments (wave 2 — permissive stubs), getUnionType (S7), getContextualSignature (S8) |
| 7 | Type construction, union/intersection & reduction, instantiation | ~4.5k (25537–26940, 22559–23290, 19697–19841, 21019–21046) | getBaseConstraintOfType (S2 — deliberate cycle), resolveStructuredTypeMembers (S8 — deliberate cycle, stub empty members), getPropertyOfType (S8) |
| 8 | Structured-type members & signatures | ~3.5k (19253–22154) | instantiateSymbol (S7 — cycle resolved by S7-before-S8), getSpreadType (S4), getReturnTypeFromBody body path (S9/flow) |
| 9 | Contextual typing, constraints, widening | ~4.5k (29448–31523, 16800–19005 partial, 31366–31520) | getTypeOfSymbol (S2), getResolvedSignature (S6), getUnionType (S7), flow (W3) |
| 10 | Index types, facts, misc + public tail | ~3.5k (26964–29448, 29604–29860, 31523–32723) | getBaseConstraintOfType (S2/S9), instantiateType (S7), relater, getApparentType (S8) |

Budget sanity: Σ ≈ 43k nominal against ~31.8k of real lines because slices overlap in the ranges they quote; actual coverage is 1:1.

## 3. ENTRY-POINT MAP (roots → leaves)

```
program → CheckSourceFile/checkSourceFile (2240)                [S1 ROOT]
  ├─ checkSourceElements → checkSourceElement → checkSourceElementWorker (2298)   [S1 dispatch]
  │    ├─ statement/class/module/import/export checks (2649–6293) [S3/S4 leaves]
  │    └─ checkNodeDeferred → sourceFileLinks.deferredNodes queue [S1]
  ├─ checkDeferredNodes → checkDeferredNode (2536/2547)          [S1; dispatches into S3/S4/S6 leaves]
  └─ checkUnusedIdentifiers family (7248–7516)                   [S3]

service roots (LS/emit path):
  getTypeOfExpression (7539) / getQuickTypeOfExpression (7563)   [S1]
    └─ checkExpression(Ex) (7755) → checkExpressionCached(Ex) → checkExpressionWorker (7933) [S1 dispatch]
         └─ ~45 per-kind checkXxxExpression leaves (8039–14150)  [S4; jsx → jsx.go]
  getDiagnostics/GetDiagnostics (14222–14240)                    [S1]
  GetSymbolAtLocation (32134) → getSymbolOfNode (14669)         [S6]
  GetTypeAtLocation (32712)                                     [S10]

type spine (the S2 skeleton):
  getTypeOfSymbol (16879)                                        [S2 ROOT]
    ├─ getTypeOfVariableOrParameterOrProperty (16930) → Worker (16964)
    │    └─ getTypeForVariableLikeDeclaration (17038)            [S2, 199 ln]
    │         ├─ getTypeFromTypeNode (23295) → Worker (23299)    [S5 leaf]
    │         ├─ getTypeForBindingElement* (18087)               [S2]
    │         └─ getWidenedTypeForVariableLikeDeclaration (17033) [S9 leaf]
    ├─ getTypeOfFuncClassEnumModule (17291) → Worker              [S2]
    ├─ getTypeOfAccessors (18918) / getWriteTypeOfAccessors      [S2]
    ├─ getTypeOfEnumMember (18910) / getTypeOfMappedSymbol (21388)[S2]
    ├─ getTypeOfAlias (19005) → getTargetOfAliasDeclaration (16122) [S6]
    └─ getTypeOfInstantiatedSymbol (16914) → instantiateSymbol (21157) [S7]

resolution spines:
  resolveEntityName (16158) → resolveQualifiedName (16214)       [S6 ROOT for name resolution]
  resolveSignature (8660) → resolveCall (9061) → chooseOverload (9246) [S6 ROOT for calls]
  resolveStructuredTypeMembers (19469) → resolve*Members family   [S8 ROOT for members]
  getUnionType (26151) → getUnionTypeWorker (26187) → removeSubtypes (26468) [S7 ROOT]
  getIndexedAccessTypeEx (27461) → getPropertyTypeForIndexType (27535) [S10 ROOT]
```

## 4. STATE SURVEY (Checker struct, checker.go 580–914; types.go for link payloads)

Hot single-ownership state (slice order must keep exactly one owner writing, others reading):

| Field(s) | Writer family (slice) | Readers |
|---|---|---|
| `nodeLinks` (core.LinkStore[NodeLinks]) + `typeNodeLinks`, `signatureLinks`, `symbolNodeLinks` | check\* expression/statement leaves (S1/S3/S4) — `resolvedType`, `flags` | everything (services, emit) |
| `valueSymbolLinks` (symbolArenaLinkStore) | getTypeOfSymbol spine (S2) — `type`, `writeType`, `declaredType` | resolve\*Members, relater, nodebuilder |
| `deferredSymbolLinks` + `resolvingExplicitTypeOfSymbol` + `skipDirectInferenceNodes` | deferred machinery (S1) + getTypeOfSymbolWithDeferredType (S2) | JSX/JS checks (S4, jsx.go) |
| `sourceFileLinks.deferredNodes` (OrderedSet on SourceFileLinks) + `deferredDiagnosticCallbacks` | checkNodeDeferred/checkDeferredNodes (S1) | checkSourceFile drain loop |
| `membersAndExportsLinks`, `lateBoundLinks`, `exportTypeLinks`, `symbolContainerLinks` | module exports resolution (S6: getExportsOfModuleWorker, lateBind\*) | everything symbol-touching |
| `declaredTypeLinks`, `typeAliasLinks` | getDeclaredTypeOf\* (S2) / getTypeAliasInstantiation (S5) | instantiation (S7) |
| `mappedSymbolLinks`, `spreadLinks`, `ReverseMappedSymbolLinks`, `markedAssignmentSymbolLinks` | mapped/spread/expando (S4 expando + S8 mapped members) | contextual typing (S9) |
| `aliasSymbolLinks`, `moduleSymbolLinks` | resolveAlias/getTargetOf\* (S6) | resolveEntityName |
| `jsxElementLinks` | jsx.go wave (NOT checker.go) | S1 dispatch (stubs) |
| `cachedTypes` (map[CachedTypeKey]\*Type) | awaited-type/iteration caches (S9/S3) | getAwaitedType\*, getIterationTypesOf\* |
| `unionTypes`, `intersectionTypes`, `tupleTypes`, `subtypeReductionCache`, `unionOfUnionTypes` | union/intersection construction (S7) | everything type-building |
| `literalTypes` (`stringLiteralTypes`/`numberLiteralTypes`/`bigintLiteralTypes`/`enumLiteralTypes`/`uniqueESSymbolTypes`), `indexedAccessTypes`, `templateLiteralTypes`, `stringMappingTypes`, `errorTypes`, `propertiesTypes` | new\* ctors (S7, S5, S10) | widening (S9), literal widening |
| `typeResolutions` + `resolutionStart` (circularity stack) | pushTypeResolution/popTypeResolution (S2) | getDeclaredTypeOf\*, getBaseConstraint (S9), getUnionType (S7) |
| `instantiationStack`, `activeMappers`, `activeTypeMappersCaches` | instantiate\* engine (S7) | S5/S8/S9 |
| `flowTypeCache`, `flowLoopCache`, `flowNodeReachable`, `lastFlowNode*`, `antecedentTypes`, `freeFlowState` | flow.go wave (not checker.go) | S4 narrowing call sites |
| `contextualInfos`, `inferenceContextInfos`, `freeinferenceState`, `apparentArgumentCount`, `callResolutionStack` | contextual typing (S9) + call resolution (S6) | resolveCall/infer seam (inference.go) |
| diagnostics collections (`diagnostics`, `suggestionDiagnostics`, `nonExistentProperties`, `reportedUnreachableNodes`) | all check\* (via addDiagnostic family, S1) | getDiagnostics |
| globals (`globals`, `moduleSymbols`, `mergedSymbols`, `unresolvedSymbols`, `patternAmbientModules*`) | initializeChecker (S1 lands constructor; S6 merges) | resolveEntityName, getGlobalType |
| ~60 intrinsic/global `*Type` fields + `getGlobalXxx func()` closures | initializeChecker (S1) + getGlobalType (S2/S5) | everything |

Notes:
- The struct field list is huge (~250 fields) but the spike already mirrors it in `rust/crates/checker/src/checker.rs` (grep-parity per SPEC §4.1), so slices only need to *activate* fields, not declare them.
- `nodeLinks`/`symbolLinks` access goes through `core.LinkStore` — the Rust side already has the link-store pattern (`links.rs`); keep the store owner inside `Checker` so no slice takes a copy.
- `lastGetCombinedNodeFlags{Node,Result}` and `lastGetCombinedModifierFlags*` are single-entry caches owned by `getCombinedNodeFlagsCached`/`getCombinedModifierFlagsCached` (S8/S2 utilities) — trivial, but don't let two slices inline their own cache.

## 5. RISK NOTES (ordered by severity)

1. **The typeCache family + circularity machinery (S2 × S7 × S8 triangle).**
   `getDeclaredTypeOfClassOrInterface` (17706, 381 ln), `resolveBaseTypesOfClass/OfInterface` (19624/19902), `pushTypeResolution`/`popTypeResolution`/`reportCircularityError` (19165–19229), and the per-kind caches (`unionTypes`, `tupleTypes`, `indexedAccessTypes`, `cachedTypes`, `subtypeReductionCache`, plus `Type.instantiations` tables from types.go) form one mutual recursion: declared types → base types → members → instantiation → union creation → back to declared types. There is no acyclic slicing; the plan contains it by making S2 own the resolution stack (`typeResolutions`/`resolutionStart`) and cache-miss policy, S7 own the constructors, S8 own member resolution — with exactly two deliberate stub directions (S2↔S7, S7↔S8). Get the cache *keys* (`CacheHashKey`/`CachedTypeKey`, xxh3 in utilities.go) right the first time; they are load-bearing for union interning and can't be changed later without breaking type identity across the whole checker.

2. **Deferred-nodes machinery (S1, must land first).**
   `checkNodeDeferred` (2528) queues into `sourceFileLinks.deferredNodes` while a file is not yet `typeChecked`; `checkSourceFile` drains it via `checkDeferredNodes`/`checkDeferredNode` (2536/2547), which re-dispatches to deferred checkers (`checkFunctionExpressionOrObjectLiteralMethodDeferred`, `checkClassExpressionDeferred`, `checkAssertionDeferred`, `checkTypeParameterDeferred`, `checkJsxElementDeferred`, `resolveUntypedCall`). The ordering between `typeChecked` flag flip and the drain is subtle (nodes queued during the drain are lost-by-design upstream). Port the queue + flag in S1 exactly as written; a Rust refactor to a worklist will silently change diagnostic order and duplicate-check behavior.

3. **JSX: it is NOT in checker.go — it's jsx.go (67,545 bytes, separate wave).**
   checker.go only contains the dispatch arms (`checkExpressionWorker` → `checkJsxElement` etc. at 8023–8034; `checkDeferredNode` → `checkJsxElementDeferred`/`checkJsxSelfClosingElementDeferred` at 2567–2570; `resolveCall` JSX arity special-cases at 8670, 9333–9391, 9482, 9616–9617, 9898; `hasContextualTypeWithNoGenericTypes` JSX guard at ~32090) and the shared state (`jsxElementLinks`, `emptyJsxObjectType`, `_jsxNamespace`, `_jsxFactoryEntity`, `ReferenceHintJsx`, `GetJSXRuntimeImportSpecifier` host method). todo_shapes.rs already carries `JsxElementLinks` for exactly this. Risk: S1/S4 dispatch stubs must match jsx.go's real signatures (`checkJsxElement(node, checkMode)`), and `resolveJsxOpeningLikeElement` shares the overload-resolution engine with S6 — land jsx.go's wave against S6's `resolveCall`, not before.
   Additional named risks: `resolveExternalModule` (15453, 306 ln) has a hard seam to the program/module-resolution layer (S6 — keep the GoWorker seam in Rust); `removeSubtypes`/`getUnionTypeFromSortedList` (361/316 ln) are perf-critical and interwoven with relater's `isTypeRelatedTo` (stub permissive until the relater wave, then re-verify against tsc baselines); `getReturnTypeFromBody` (281 ln) depends on flow.go narrowing + contextual returns (S9), so S8's `getReturnTypeOfSignature` should stub the body path until S9/flow land.

## 6. Pre-existing `TODO(port M6)` banner mapping (rust/crates/checker/src)

The ~110 banners cluster into exactly the slices above:
- **checker.rs:1186–1257** — the `checkExpressionWorker` per-kind dispatch (checkIdentifier … checkJsxAttributes): banner-owners are S1 (dispatch), S4 (operator/literal leaves), and the `checkJsx*` six → jsx.go wave (todo_shapes `JsxElementLinks`).
- **checker.rs:807–834** — `getTypeOfVariableOrParameterOrPropertyWorker` arms (checkPropertyAssignment, checkShorthandPropertyAssignment, checkObjectLiteralMethod, getWidenedTypeForAssignmentDeclaration, getTypeOfEnumMember) → S2 + S4 (expando).
- **checker.rs:890–1112** — getWidenedTypeWithContext, getTypeForBindingElement, getTypeFromTypeNode, for..in/for..of variable types, getBaseTypeOfEnumLikeType, addOptionalityEx, mapType/getUnionType → S2, S5, S7.
- **checker.rs:216–296, 1732–1759** — struct-field-level gaps (booleanType, stringOrNumberType, uniqueLiteralMapper, emptyObjectType family, typeofType, enumLiteralTypes, signatureLinks, jsxElementLinks) → S7 (union construction + new* ctors) and jsx wave.
- **utilities.rs:547–563** — getSignatureFromDeclaration/getEffectiveCallArguments → S8; **utilities.rs:3185–3220** — getResolvedBaseConstraint → S9, module-not-found seam → S6.
- **todo_shapes.rs** shapes → not checker.go: TypeMapperId (mapper.go, wave 1), Tracer, Relation/Relater (relater.go wave 2), Inference\* (inference.go wave 2), FlowState (flow.go wave 13), NodeBuilder, EmitResolverLinks, IterationTypesResolver/IterationTypes (→ slice 3 replaces these two shapes with real structs).
