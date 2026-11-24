import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import test from "node:test";

// The Rust contracts in crates/interface-contracts and their TypeScript mirror
// in electron/src are written by hand on both sides. This test reads both
// sources and checks, for every Rust serde struct with a same-named TypeScript
// interface, that the wire field names agree after serde renames and that a
// field Rust may leave out is optional in TypeScript (and the reverse for one
// Rust requires). It also checks the channel name sets themselves, not only
// their counts. Anything the parsers cannot read is named in UNCHECKED, so a
// new layout they do not understand fails here instead of passing unread.
// `gate::CROSS_LANGUAGE` lists the files read here so either side reruns it.

const repo = new URL("../../", import.meta.url);
const read = path => readFileSync(new URL(path, repo), "utf8");
const listed = (dir, pattern) => readdirSync(new URL(dir, repo))
  .filter(name => pattern.test(name) && !/\.test\./.test(name)).sort().map(name => `${dir}${name}`);

const RUST_FILES = listed("crates/interface-contracts/src/", /\.rs$/);
const TS_FILES = ["electron/src/shared/", "electron/src/main/", "electron/src/remote/"]
  .flatMap(dir => listed(dir, /\.(ts|cts|tsx)$/));

// Deliberate differences, by `Type.field` and the check that reports it. Each
// says why it is not a bug. An entry that no longer matches anything fails the
// test, so this list cannot outlive its reason.
const ALLOWED = new Map([
  // Renderer-only projection: the renderer adds activity to the host's summary.
  ["TerminalSummary.activity onlyTs", "renderer-side activity, filled in after the host frame arrives"],
  // Host-only launch fields. The host re-derives them for every launch and
  // refuses them from the renderer (apps/interface-host/src/dispatcher.rs).
  ["LaunchOptions.approvalGate onlyRust", "host-owned agent gate; the renderer may not set it"],
  ["LaunchOptions.agentAccess onlyRust", "host-owned agent access; the renderer may not set it"],
  ["LaunchOptions.additionalDirectories onlyRust", "host-owned extra roots; the renderer may not set them"],
]);

// Same-named pairs the parsers do not compare field by field, with the reason.
// The test fails when this set changes: read a new one by hand, then add it.
const UNCHECKED = new Map([
  // A struct that flattens a tagged enum is a TypeScript union of shapes;
  // the fixtures each one names in crates/interface-contracts pin those.
  ["EngineEvent", "flattens EngineEventKind; TypeScript union"],
  ["OutputReviewRequest", "flattens OutputReviewCommand; TypeScript union"],
  ["RecoveryRequest", "flattens RecoveryCommand; TypeScript union"],
  // TypeScript unions of request or result shapes.
  ["OutputPreviewRequest", "TypeScript union"],
  ["PromptContextValidation", "TypeScript union"],
  // Newtypes: a Rust tuple struct is a plain string or number on the wire.
  ["TerminalGeneration", "Rust tuple or unit struct; TypeScript type `number`"],
  ["TerminalId", "Rust tuple or unit struct; TypeScript type `string`"],
]);

const camel = name => name.replace(/_([a-z0-9])/g, (_, c) => c.toUpperCase());
const RENAME_ALL = {
  camelCase: camel,
  snake_case: name => name,
  "kebab-case": name => name.replaceAll("_", "-"),
  lowercase: name => name.toLowerCase(),
};

/** Index just past the TypeScript string or template literal opening at `i`;
 *  a template's `${…}` holds code, which may hold strings of its own. */
function skipQuoted(source, i) {
  const quote = source[i];
  let j = i + 1;
  while (j < source.length && source[j] !== quote) {
    if (source[j] === "\\") j += 2;
    else if (quote === "`" && source.startsWith("${", j)) {
      let depth = 0;
      for (j += 1; j < source.length; j++) {
        if (source.startsWith("//", j)) j = source.indexOf("\n", j);
        else if (source.startsWith("/*", j)) j = source.indexOf("*/", j) + 1;
        else if ("\"'`".includes(source[j])) j = skipQuoted(source, j) - 1;
        else if (source[j] === "{") depth++;
        else if (source[j] === "}" && --depth === 0) break;
      }
      j++;
    } else j++;
  }
  return j + 1;
}

/** Source with comments removed and every string literal replaced by `"<n>"`,
 *  its text kept in `strings[n]`, so braces in strings never count. */
function mask(source, rust) {
  const strings = [];
  let out = "";
  for (let i = 0; i < source.length;) {
    const rest = source.slice(i);
    if (rest.startsWith("//")) { const end = source.indexOf("\n", i); i = end < 0 ? source.length : end; continue; }
    if (rest.startsWith("/*")) {
      let depth = 0, j = i;
      do {
        if (source.startsWith("/*", j)) { depth++; j += 2; }
        else if (source.startsWith("*/", j)) { depth--; j += 2; }
        else j++;
      } while (depth > 0 && j < source.length);
      i = j; continue;
    }
    const raw = rust ? /^r(#*)"/.exec(rest) : null;
    if (raw) {
      const close = `"${raw[1]}`;
      const end = source.indexOf(close, i + raw[0].length);
      assert(end >= 0, "unterminated raw string");
      strings.push(source.slice(i + raw[0].length, end));
      out += `"${strings.length - 1}"`; i = end + close.length; continue;
    }
    if (rust && source[i] === "'") {
      const char = /^'(?:\\.[^']*|[^\\'])'/.exec(rest);
      if (char) { out += "' '"; i += char[0].length; continue; }
    }
    if (!rust && source[i] === "`") {
      const end = skipQuoted(source, i);
      strings.push(source.slice(i + 1, end - 1));
      out += `"${strings.length - 1}"`; i = end; continue;
    }
    const quote = source[i];
    if (quote === "\"" || (!rust && quote === "'")) {
      let j = i + 1, text = "";
      while (j < source.length && source[j] !== quote) {
        if (source[j] === "\\") { text += source.slice(j, j + 2); j += 2; } else text += source[j++];
      }
      strings.push(text);
      out += `"${strings.length - 1}"`; i = j + 1; continue;
    }
    out += source[i++];
  }
  return { text: out, strings };
}

/** Index just past the bracket that closes the one at `open`. */
function closing(text, open) {
  const pairs = { "{": "}", "(": ")", "[": "]", "<": ">" };
  const start = text[open], end = pairs[start];
  let depth = 0;
  for (let i = open; i < text.length; i++) {
    if (text[i] === start) depth++;
    else if (text[i] === end && !(start === "<" && text[i - 1] === "=") && --depth === 0) return i + 1;
  }
  throw new Error(`unbalanced ${start} at ${open}`);
}

/** Splits at top-level `separators`, ignoring nested brackets. */
function splitTop(text, separators) {
  const parts = [];
  let depth = 0, current = "";
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if ("{([<".includes(c)) depth++;
    else if ("})]".includes(c) || (c === ">" && text[i - 1] !== "=")) depth--;
    if (depth === 0 && separators.includes(c)) { parts.push(current); current = ""; } else current += c;
  }
  parts.push(current);
  return parts;
}

/** `#[...]` attributes at the start of `text`, and the text after them. */
function leadingAttributes(text) {
  const attributes = [];
  let rest = text.trimStart();
  while (rest.startsWith("#[")) {
    const end = closing(rest, 1);
    attributes.push(rest.slice(2, end - 1));
    rest = rest.slice(end).trimStart();
  }
  return { attributes, rest };
}

/** The `serde(...)` arguments in `attributes`, string placeholders restored. */
function serdeArguments(attributes, strings) {
  return attributes.filter(attribute => /^serde\s*\(/.test(attribute)).flatMap(attribute => {
    const inner = attribute.slice(attribute.indexOf("(") + 1, attribute.lastIndexOf(")"));
    return splitTop(inner, [","]).map(part => part.trim().replace(/"(\d+)"/g, (_, n) => JSON.stringify(strings[n])))
      .filter(Boolean);
  });
}
const serdeValue = (args, key) => {
  for (const arg of args) {
    const match = new RegExp(`^${key}\\s*=\\s*("(?:[^"\\\\]|\\\\.)*")$`).exec(arg);
    if (match) return JSON.parse(match[1]);
  }
  return undefined;
};

function rustStructs() {
  const structs = new Map(), seen = new Set();
  for (const file of RUST_FILES) {
    const { text, strings } = mask(read(file), true);
    // Drop `#[cfg(test)]` items: test modules are not the contract.
    let source = text;
    for (let at; (at = source.indexOf("#[cfg(test)]")) >= 0;) {
      const body = source.indexOf("{", at), semi = source.indexOf(";", at);
      const end = semi >= 0 && semi < body ? semi + 1 : closing(source, body);
      source = source.slice(0, at) + source.slice(end);
    }
    const item = /((?:#\[[^\]]*(?:\[[^\]]*\][^\]]*)*\]\s*)*)(?:pub(?:\([^)]*\))?\s+)?struct\s+(\w+)\s*(<[^{;(]*>)?\s*([{;(])/g;
    for (let match; (match = item.exec(source));) {
      const [, attributeText, name, , opener] = match;
      seen.add(name);
      const { attributes } = leadingAttributes(attributeText);
      if (!attributes.some(attribute => /^derive\s*\(.*\b(Serialize|Deserialize)\b/s.test(attribute))) continue;
      const where = `${file}: ${name}`;
      const container = serdeArguments(attributes, strings);
      const entry = { file, name, fields: new Map(), problems: [], readsAs: undefined, writesAs: undefined };
      if (structs.has(name)) {
        // Two modules' structs share a name; neither is the one TypeScript means for sure.
        structs.get(name).problems.push(`also declared in ${file}`);
        continue;
      }
      structs.set(name, entry);
      if (opener !== "{") { entry.problems.push("Rust tuple or unit struct"); continue; }
      for (const arg of container) {
        if (/^(transparent|remote)\b/.test(arg)) entry.problems.push(`serde(${arg})`);
        else if (!/^(rename_all|deny_unknown_fields|default|rename|bound|try_from|from|into)\b/.test(arg)) {
          throw new Error(`${where}: unknown container attribute serde(${arg})`);
        }
      }
      // `try_from`/`from` read another type's fields; `into` writes another's.
      entry.readsAs = serdeValue(container, "try_from") ?? serdeValue(container, "from");
      entry.writesAs = serdeValue(container, "into");
      const renameAll = serdeValue(container, "rename_all");
      if (container.some(arg => /^rename_all\s*\(/.test(arg))) entry.problems.push("split rename_all");
      if (renameAll !== undefined && !RENAME_ALL[renameAll]) throw new Error(`${where}: rename_all = ${renameAll}`);
      const rename = RENAME_ALL[renameAll ?? "snake_case"];
      const containerDefault = container.includes("default") || container.some(arg => /^default\s*=/.test(arg));
      const open = match.index + match[0].length - 1;
      const body = source.slice(open + 1, closing(source, open) - 1);
      for (const part of splitTop(body, [","])) {
        if (!part.trim()) continue;
        const { attributes: fieldAttributes, rest } = leadingAttributes(part);
        const field = /^(?:pub(?:\([^)]*\))?\s+)?(r#)?(\w+)\s*:\s*([\s\S]+)$/.exec(rest.trim());
        if (!field) throw new Error(`${where}: cannot read field \`${part.trim()}\``);
        const args = serdeArguments(fieldAttributes, strings);
        for (const arg of args) {
          if (!/^(rename|alias|default|skip|skip_serializing|skip_deserializing|skip_serializing_if|flatten|with|serialize_with|deserialize_with|bound)\b/.test(arg)) {
            throw new Error(`${where}.${field[2]}: unknown field attribute serde(${arg})`);
          }
        }
        if (args.includes("skip") || (args.includes("skip_serializing") && args.includes("skip_deserializing"))) continue;
        if (args.some(arg => /^rename\s*\(/.test(arg))) entry.problems.push(`${field[2]}: split rename`);
        const type = field[3].replace(/\s+/g, " ").trim();
        const optionType = /^(?:std::option::)?Option\s*</.test(type);
        const fieldDefault = args.some(arg => /^default\b/.test(arg));
        entry.fields.set(serdeValue(args, "rename") ?? rename(field[2]), {
          type,
          flatten: args.includes("flatten"),
          // Rust may leave it out of what it writes.
          omittedOut: args.some(arg => /^skip_serializing(_if)?\b/.test(arg)),
          // Rust reads it as absent without an error.
          optionalIn: containerDefault || fieldDefault || args.includes("skip_deserializing")
            || (optionType && !args.some(arg => /^(deserialize_)?with\b/.test(arg))),
        });
      }
    }
  }
  return { structs, seen };
}

/** Index of the `;` ending a declaration that starts at `start`, at depth 0. */
function declarationEnd(text, start) {
  let depth = 0;
  for (let i = start; i < text.length; i++) {
    const c = text[i];
    if ("{([<".includes(c)) depth++;
    else if ("})]".includes(c) || (c === ">" && text[i - 1] !== "=")) depth--;
    else if (depth === 0 && (c === ";" || text.startsWith("\nexport ", i))) return i;
  }
  return text.length;
}

/** The property members of an object-type body. */
function parseMembers(body, strings) {
  const fields = new Map(), problems = [];
  // A member starts with a property name; anything else continues the one before.
  const members = [];
  for (const piece of splitTop(body, [";", ",", "\n"])) {
    if (/^\s*(?:readonly\s+)?(?:"\d+"|[\w$]+)\??\s*[:(<]/.test(piece) || /^\s*[[(]/.test(piece)) members.push(piece);
    else if (piece.trim()) {
      if (members.length) members[members.length - 1] += ` ${piece}`;
      else problems.push(`cannot read \`${piece.trim()}\``);
    }
  }
  for (const member of members) {
    if (/^\s*\[/.test(member)) { problems.push("TypeScript index signature"); continue; }
    const field = /^\s*(?:readonly\s+)?(?:"(\d+)"|([\w$]+))(\?)?\s*:\s*([\s\S]*)$/.exec(member);
    if (!field) { problems.push("TypeScript method or call signature"); continue; }
    const key = field[1] !== undefined ? strings[field[1]] : field[2];
    const type = field[4].replace(/\s+/g, " ").trim();
    fields.set(key, { type, optional: !!field[3] || /(^|\|)\s*undefined\s*($|\|)/.test(type) });
  }
  return { fields, problems };
}

function tsDeclarations() {
  const declarations = new Map();
  for (const file of TS_FILES) {
    const { text, strings } = mask(read(file), false);
    const declaration = /export\s+(?:declare\s+)?(interface|type)\s+(\w+)\s*(<[^{=]*?>)?\s*(?:extends\s+([^{]+?))?\s*(\{|=)/g;
    for (let match; (match = declaration.exec(text));) {
      const [, kind, name, , extendsText] = match;
      let entry;
      if (kind === "type") {
        const start = declaration.lastIndex;
        entry = { file, strings, kind, text: text.slice(start, declarationEnd(text, start)).trim() };
      } else {
        const open = declaration.lastIndex - 1;
        const parents = extendsText ? splitTop(extendsText, [","]).map(part => part.trim()).filter(Boolean) : [];
        entry = { file, strings, kind, body: text.slice(open + 1, closing(text, open) - 1), parents };
      }
      if (declarations.has(name)) entry.duplicate = declarations.get(name).file;
      declarations.set(name, entry);
    }
  }
  return declarations;
}

/** Wire fields of a TypeScript type expression, with what kept it from being read. */
function tsTypeFields(declarations, typeText, strings, seen) {
  const fields = new Map(), problems = [];
  const text = typeText.trim().replace(/^\|/, "").trim();
  if (splitTop(text, ["|"]).length > 1) return { fields, problems: ["TypeScript union"] };
  for (const raw of splitTop(text, ["&"])) {
    const part = raw.trim();
    let resolved;
    if (part.startsWith("{") && closing(part, 0) === part.length) resolved = parseMembers(part.slice(1, -1), strings);
    else if (part.startsWith("(") && closing(part, 0) === part.length) {
      resolved = tsTypeFields(declarations, part.slice(1, -1), strings, seen);
    } else if (/^\w+$/.test(part) && declarations.has(part) && !seen.has(part)) {
      resolved = tsFields(declarations, part, seen);
    } else {
      const picked = /^(Omit|Pick)\s*<\s*(\w+)\s*,\s*([\s\S]+)>$/.exec(part);
      if (picked && declarations.has(picked[2]) && !seen.has(picked[2])) {
        const base = tsFields(declarations, picked[2], seen);
        const keys = new Set([...picked[3].matchAll(/"(\d+)"/g)].map(m => strings[m[1]]));
        resolved = { problems: base.problems, fields: new Map([...base.fields]
          .filter(([key]) => keys.has(key) === (picked[1] === "Pick"))) };
      } else resolved = { fields: new Map(), problems: [`TypeScript type \`${part.replace(/\s+/g, " ")}\``] };
    }
    problems.push(...resolved.problems);
    for (const [key, field] of resolved.fields) fields.set(key, field);
  }
  return { fields, problems };
}

/** Wire fields of a named TypeScript interface or object type. */
function tsFields(declarations, name, seen = new Set()) {
  const entry = declarations.get(name);
  const inner = new Set([...seen, name]);
  if (entry.kind === "type") {
    const result = tsTypeFields(declarations, entry.text, entry.strings, inner);
    return entry.duplicate ? { ...result, problems: [...result.problems, `also declared in ${entry.duplicate}`] } : result;
  }
  const fields = new Map(), problems = entry.duplicate ? [`also declared in ${entry.duplicate}`] : [];
  for (const parent of entry.parents) {
    const inherited = tsTypeFields(declarations, parent, entry.strings, inner);
    problems.push(...inherited.problems);
    for (const [key, field] of inherited.fields) fields.set(key, field);
  }
  const own = parseMembers(entry.body, entry.strings);
  problems.push(...own.problems);
  for (const [key, field] of own.fields) fields.set(key, field);
  return { fields, problems };
}

/** Rust wire fields of one struct's own layout, `#[serde(flatten)]` structs merged in. */
function rustLayout(structs, name, seen) {
  const entry = structs.get(name);
  const fields = new Map(), problems = [...entry.problems];
  for (const [key, field] of entry.fields) {
    if (!field.flatten) { fields.set(key, field); continue; }
    const optional = /^Option\s*</.test(field.type);
    const inner = field.type.replace(/^Option\s*<\s*(.*)>$/, "$1").trim();
    if (!structs.has(inner) || seen.has(inner)) { problems.push(`flattens ${field.type}`); continue; }
    const nested = rustFields(structs, inner, new Set([...seen, name]));
    problems.push(...nested.problems);
    for (const [nestedKey, nestedField] of nested.fields) {
      fields.set(nestedKey, optional ? { ...nestedField, omittedOut: true, optionalIn: true } : nestedField);
    }
  }
  return { fields, problems };
}

/** Rust wire fields as Rust writes and reads them, following `try_from`/`from`/`into`. */
function rustFields(structs, name, seen = new Set()) {
  const entry = structs.get(name);
  const inner = new Set([...seen, name]);
  const via = other => (structs.has(other) && !inner.has(other) ? rustFields(structs, other, inner)
    : { fields: new Map(), problems: [`serde through ${other}`] });
  const own = rustLayout(structs, name, inner);
  const written = entry.writesAs ? via(entry.writesAs) : own;
  const readBack = entry.readsAs ? via(entry.readsAs) : own;
  const problems = [...new Set([...written.problems, ...readBack.problems])];
  const names = layout => [...layout.fields.keys()].sort().join(",");
  if (!problems.length && names(written) !== names(readBack)) problems.push("Rust writes and reads different fields");
  const fields = new Map([...written.fields].map(([key, field]) => [key,
    { ...field, optionalIn: readBack.fields.get(key)?.optionalIn ?? true }]));
  return { fields, problems };
}

/** Every difference and every unreadable pair, as stable strings. */
function compare() {
  const { structs, seen } = rustStructs();
  const declarations = tsDeclarations();
  // Every top-level declaration a plain line scan finds must have reached the
  // parsers, so a literal they misread cannot hide a type from the comparison.
  const missed = [
    ...RUST_FILES.flatMap(file => [...read(file).matchAll(/^(?:pub(?:\([^)]*\))? )?struct (\w+)/gm)]
      .map(m => m[1]).filter(name => !seen.has(name)).map(name => `${file}: ${name}`)),
    ...TS_FILES.flatMap(file => [...read(file).matchAll(/^export (?:declare )?(?:interface|type) (\w+)/gm)]
      .map(m => m[1]).filter(name => !declarations.has(name)).map(name => `${file}: ${name}`)),
  ];
  assert.deepEqual(missed, [], "declarations the parsers did not read");
  const differences = [], unchecked = new Map();
  let pairs = 0;
  for (const name of [...structs.keys()].sort()) {
    if (!declarations.has(name)) continue;
    pairs++;
    const rust = rustFields(structs, name);
    const ts = tsFields(declarations, name);
    const problems = [...rust.problems, ...ts.problems];
    if (problems.length) { unchecked.set(name, problems.join("; ")); continue; }
    for (const key of rust.fields.keys()) if (!ts.fields.has(key)) differences.push(`${name}.${key} onlyRust`);
    for (const key of ts.fields.keys()) if (!rust.fields.has(key)) differences.push(`${name}.${key} onlyTs`);
    for (const [key, field] of rust.fields) {
      const mirror = ts.fields.get(key);
      if (!mirror) continue;
      if (field.omittedOut && !mirror.optional) differences.push(`${name}.${key} Rust may omit it, TypeScript requires it`);
      if (!field.optionalIn && mirror.optional) differences.push(`${name}.${key} TypeScript may omit it, Rust requires it`);
    }
  }
  return { structs, declarations, pairs, differences, unchecked };
}

test("every same-named Rust struct and TypeScript interface agree on wire fields", () => {
  const { structs, declarations, pairs, differences, unchecked } = compare();
  // Guard the parsers themselves: a regex that stops matching must not pass by reading nothing.
  assert(structs.size > 250, `read only ${structs.size} Rust serde structs`);
  assert(declarations.size > 250, `read only ${declarations.size} TypeScript declarations`);
  assert(pairs > 200, `compared only ${pairs} same-named pairs`);
  assert.deepEqual(Object.fromEntries(unchecked), Object.fromEntries(UNCHECKED),
    "a same-named pair the parsers cannot read changed; read it by hand and update UNCHECKED");
  const unexpected = differences.filter(difference => !ALLOWED.has(difference));
  assert.deepEqual(unexpected, [], "Rust and TypeScript disagree; fix one side or add a commented ALLOWED entry");
  const stale = [...ALLOWED.keys()].filter(entry => !differences.includes(entry));
  assert.deepEqual(stale, [], "ALLOWED entries that no longer match anything");
});

test("the Rust and TypeScript channel tables name the same channels", () => {
  const rustIpc = mask(read("crates/interface-contracts/src/ipc.rs"), true);
  const values = (text, strings) => [...text.matchAll(/"(\d+)"/g)].map(m => strings[m[1]]).filter(v => v.startsWith("iface:"));
  const macro = rustIpc.text.slice(rustIpc.text.indexOf("channels!(", rustIpc.text.indexOf("macro_rules! channels")));
  const rustShared = values(macro.slice(0, closing(macro, macro.indexOf("("))), rustIpc.strings);
  const extraAt = rustIpc.text.indexOf("pub const EXTRA_CHANNELS");
  const extraOpen = rustIpc.text.indexOf("[", rustIpc.text.indexOf("=", extraAt));
  const rustExtra = values(rustIpc.text.slice(extraOpen, closing(rustIpc.text, extraOpen)), rustIpc.strings);
  const protocol = mask(read("apps/interface-host/src/protocol.rs"), true);
  const pushAt = protocol.text.indexOf("const PUSH_OPERATIONS");
  const pushOpen = protocol.text.indexOf("[", protocol.text.indexOf("=", pushAt));
  const rustPush = values(protocol.text.slice(pushOpen, closing(protocol.text, pushOpen)), protocol.strings);

  const tsTable = (file, name) => {
    const { text, strings } = mask(read(file), false);
    const open = text.indexOf("{", text.indexOf(`export const ${name} =`));
    const entries = splitTop(text.slice(open + 1, closing(text, open) - 1), [","]).filter(part => part.trim());
    return Object.fromEntries(entries.map(part => {
      const entry = /^\s*(\w+)\s*:\s*"(\d+)"\s*$/.exec(part);
      assert(entry, `${file}: cannot read ${name} entry \`${part.trim()}\``);
      return [entry[1], strings[entry[2]]];
    }));
  };
  const tsShared = tsTable("electron/src/shared/ipc.ts", "CHANNELS");
  const tsExtra = tsTable("electron/src/main/bridge.ts", "EXTRA_CHANNELS");
  const client = mask(read("electron/src/main/rust-host-client.ts"), false).text;
  const pushOpenTs = client.indexOf("[", client.indexOf("export const RUST_HOST_PUSH_CHANNELS"));
  const tsPush = [...client.slice(pushOpenTs, closing(client, pushOpenTs)).matchAll(/\b(CHANNELS|EXTRA_CHANNELS)\.(\w+)/g)]
    .map(([, table, key]) => (table === "CHANNELS" ? tsShared : tsExtra)[key]);

  const sorted = list => [...list].sort();
  assert(rustShared.length > 100 && rustExtra.length > 0 && rustPush.length > 0, "the Rust channel tables were not read");
  assert.deepEqual(sorted(Object.values(tsShared)), sorted(rustShared), "shared channel table");
  assert.deepEqual(sorted(Object.values(tsExtra)), sorted(rustExtra), "bridge-owned channel table");
  assert.deepEqual(sorted(tsPush), sorted(rustPush), "push channels");
});
