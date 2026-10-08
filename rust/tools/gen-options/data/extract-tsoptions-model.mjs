// extract-tsoptions-model.mjs — provenance script for tsoptions-model.json.
//
// Regenerates data/tsoptions-model.json from the TRUE source of the option
// tables: tools/scripts/tsc/options.ts (the hand-maintained `OptionsModel`
// literal that tools/scripts/tsc/generate-options.ts consumes to emit
// tsc/internal/tsoptions/{options,declarations}_generated.go). Per SPEC §6,
// the Go output is never a generation input; this script keeps the JSON in
// lockstep with options.ts instead of hand-mirroring it.
//
// Run from the repo root (node >= 23 with default type stripping):
//
//   node rust/tools/gen-options/data/extract-tsoptions-model.mjs
//
// then re-run `cargo run -p gen-options` and commit both the JSON and the
// regenerated outputs. The extraction is a pure `JSON.stringify(options)` —
// no transformation, no Go output involved.

import { writeFileSync } from "node:fs";
import { options } from "../../../tools/scripts/tsc/options.ts";

writeFileSync(
    new URL("./tsoptions-model.json", import.meta.url),
    JSON.stringify(options, null, 1) + "\n",
);
console.log(`extract-tsoptions-model: wrote tsoptions-model.json (${Object.keys(options).length} model sections)`);
