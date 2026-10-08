import fs from "fs";

const goSrc = fs.readFileSync(
  "/home/dih/Typescript-glm5.3/tsc/internal/tsoptions/declarations_generated.go",
  "utf8",
);
const rustSrc = fs.readFileSync("/tmp/genws/crates/tsoptions/src/declarations_generated.rs", "utf8");

// Line-based extraction: Go option blocks start with a line `{` (after optional
// &) and end with a line `},`; string values containing braces can't confuse it.
function goSliceBlocks(name) {
  const re = new RegExp(`var ${name} = \\[\\]\\*CommandLineOption\\{`);
  const m = goSrc.match(re);
  if (!m) throw new Error("no go var " + name);
  const start = goSrc.indexOf("{", m.index);
  let end = -1;
  let depth = 0;
  for (let i = start; i < goSrc.length; i++) {
    if (goSrc[i] === "{") depth++;
    else if (goSrc[i] === "}") {
      depth--;
      if (depth === 0) {
        end = i;
        break;
      }
    }
  }
  const lines = goSrc.slice(start + 1, end).split("\n");
  const blocks = [];
  let cur = null;
  for (const line of lines) {
    const t = line.trim();
    if (cur === null) {
      if (t === "&TscBuildOption,") blocks.push("REF:TSC_BUILD_OPTION");
      else if (/^[{]/.test(t)) cur = [t];
    } else {
      cur.push(t);
      if (t === "},") {
        blocks.push(cur.join("\n"));
        cur = null;
      }
    }
  }
  return blocks.filter((b) => b.length > 0);
}

function rustSliceBlocks(name) {
  const re = new RegExp(`pub static ${name}: &\\[&CommandLineOption\\] = &\\[`);
  const m = rustSrc.match(re);
  if (!m) throw new Error("no rust static " + name);
  const start = rustSrc.indexOf("= &[", m.index) + 4;
  let end = -1;
  let depth = 1; // the initializer's opening bracket is before `start`
  for (let i = start; i < rustSrc.length; i++) {
    if (rustSrc[i] === "[") depth++;
    else if (rustSrc[i] === "]") {
      depth--;
      if (depth === 0) {
        end = i;
        break;
      }
    }
  }
  const lines = rustSrc.slice(start, end).split("\n");
  const blocks = [];
  let cur = null;
  for (const line of lines) {
    const t = line.trim();
    if (cur === null) {
      if (/^&CommandLineOption \{/.test(t)) cur = [t];
      else if (/^&TSC_BUILD_OPTION,$/.test(t)) blocks.push("REF:TSC_BUILD_OPTION");
    } else {
      cur.push(t);
      if (t === "},") {
        blocks.push(cur.join("\n"));
        cur = null;
      }
    }
  }
  return blocks;
}

function parseBlockGo(b) {
  // strip outer braces
  b = b.replace(/^\{/, "").replace(/\}$/, "");
  const out = [];
  const re = /(\w+):\s*(?:"((?:[^"\\]|\\.)*)"|([^\n]+?)),\s*(?=\n|$)/g;
  let m;
  while ((m = re.exec(b))) {
    out.push([m[1], m[2] !== undefined ? m[2] : m[3].trim()]);
  }
  return out;
}

function parseBlockRust(b) {
  b = b.replace(/&CommandLineOption\s*\{/, "").replace(/\}$/, "");
  // re-join rustfmt-wrapped lines: a field is complete when its line ends with ","
  const raw = b.split("\n").map((l) => l.trim()).filter(Boolean);
  const joined = [];
  for (const line of raw) {
    if (joined.length && !joined.at(-1).endsWith(",")) {
      joined[joined.length - 1] += line;
    } else {
      joined.push(line);
    }
  }
  b = joined.join("\n");
  const out = [];
  const re = /(\w+):\s*(?:"((?:[^"\\]|\\.)*)"|([^\n]+?)),\s*(?=\n|$)/g;
  let m;
  while ((m = re.exec(b))) {
    out.push([m[1], m[2] !== undefined ? m[2] : m[3].trim()]);
  }
  return out;
}

const normKey = (k) => k.toLowerCase().replace(/_/g, "");
const normVal = (v) =>
  v
    .replace(/Some\(/g, "(")
    .replace(/DefaultValueDescription::(\w+)\(/g, "(")
    .replace(/CommandLineOptionKind::/g, "")
    .replace(/CommandLineOptionType/g, "")
    .replace(/CommandLineOptionPathKind:?/g, "")
    .replace(/ExtraValidation:?/g, "extraValidation")
    .replace(/Tristate::/g, "core.")
    .replace(/ScriptTarget::/g, "core.ScriptTarget")
    .replace(/&tsc_diagnostics::/g, "diagnostics.")
    .replace(/[\s"():._]/g, "")
    .toUpperCase();

let diffs = 0;
const RUST_STATIC = {
  commonOptionsWithBuild: "COMMON_OPTIONS_WITH_BUILD",
  optionsForCompiler: "OPTIONS_FOR_COMPILER",
  typeAcquisitionDecls: "TYPE_ACQUISITION_DECLS",
  OptionsForBuild: "OPTIONS_FOR_BUILD",
};

function compare(name) {
  const g = goSliceBlocks(name);
  const r = rustSliceBlocks(RUST_STATIC[name]);
  if (g.length !== r.length) {
    console.log(`${name}: LENGTH MISMATCH go=${g.length} rust=${r.length}`);
    diffs++;
    return;
  }
  for (let i = 0; i < g.length; i++) {
    if (g[i] === "REF:TSC_BUILD_OPTION" || r[i] === "REF:TSC_BUILD_OPTION") {
      if (g[i] !== r[i]) {
        console.log(`${name}[${i}]: REF MISMATCH go=${g[i]} rust=${r[i]}`);
        diffs++;
      }
      continue;
    }
    const goFields = new Map(parseBlockGo(g[i]).map(([k, v]) => [normKey(k), v]));
    const rustFields = new Map(
      parseBlockRust(r[i]).map(([k, v]) => [normKey(k), v]),
    );    // ElementOptions / element_options intentionally differ (lazily built)
    goFields.delete("elementoptions");
    rustFields.delete("elementoptions");
    const goKeys = [...goFields.keys()].sort();
    const rustKeys = [...rustFields.keys()].sort();
    // Go emits only the fields options.ts sets; the Rust table emits every
    // field (const literals can't omit fields), so the Rust key set must be a
    // superset whose extras are all zero-valued.
    for (const k of rustKeys) {
      if (
        !goFields.has(k) &&
        !["", "NONE", "0", "FALSE", "EXTRAVALIDATIONNONE"].includes(normVal(rustFields.get(k)))
      ) {
        console.log(
          `${name}[${i}] (${goFields.get("name")}): EXTRA NONZERO FIELD ${k}=${rustFields.get(k)}`,
        );
        diffs++;
      }
    }
    for (const k of goKeys) {
      if (!rustFields.has(k)) {
        console.log(`${name}[${i}] (${goFields.get("name")}): MISSING KEY ${k}`);
        diffs++;
        continue;
      }
      if (normVal(goFields.get(k)) !== normVal(rustFields.get(k))) {
        console.log(
          `${name}[${i}] (${goFields.get("name")}): VALUE MISMATCH ${k}: go=${goFields.get(k)} rust=${rustFields.get(k)}`,
        );
        diffs++;
      }
    }
  }
}

for (const n of ["commonOptionsWithBuild", "optionsForCompiler", "typeAcquisitionDecls", "OptionsForBuild"]) {
  compare(n);
}
console.log(diffs === 0 ? "PARITY OK" : `${diffs} diffs`);
