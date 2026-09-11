/**
 * Rewrites every *relative* import/export specifier in the emitted `dist`
 * to be fully specified (`./cn` → `./cn.js`, `./components` → its
 * `./components/index.js`).
 *
 * `tsc` emits the specifier exactly as written in the source, and this
 * package's source is written for `moduleResolution: "Bundler"` — so the
 * raw output imports `"../../lib/cn"` with no extension. Every bundler
 * resolves that; Node's own ESM resolver does not, and neither does
 * webpack when a consumer enables `fullySpecified`. Since the package is
 * published to npm rather than consumed only from this workspace, that is
 * a real defect for a consumer we cannot see, so it is fixed here rather
 * than documented as a caveat.
 *
 * Checked rather than assumed: the extension appended is the one that
 * actually exists on disk next to the emitted file, and a specifier that
 * resolves to a directory is pointed at that directory's own `index.js`.
 * A specifier that resolves to neither is left alone and reported, so a
 * silent miss is a loud one.
 *
 * Also rewrites the `.d.ts` files, whose specifiers Node and TypeScript's
 * own `Node16`/`NodeNext` consumers resolve by the same rules.
 */

import { existsSync } from "node:fs";
import { readdir, readFile, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";

const root = resolve(process.argv[2] ?? "dist");

/** `from "x"`, `import("x")`, and `export … from "x"` all land here. */
const SPECIFIER = /(\bfrom\s*|\bimport\s*\(\s*)(["'])(\.[^"']*)\2/g;

async function* walk(dir) {
  for (const entry of await readdir(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) yield* walk(path);
    else yield path;
  }
}

/** The extension a `.js` or `.d.ts` file should point at for `spec`. */
function resolved(fromFile, spec, declaration) {
  const base = resolve(dirname(fromFile), spec);
  if (declaration) {
    if (existsSync(`${base}.d.ts`)) return `${spec}.js`;
    if (existsSync(join(base, "index.d.ts"))) return `${spec}/index.js`;
  }
  if (existsSync(`${base}.js`)) return `${spec}.js`;
  if (existsSync(join(base, "index.js"))) return `${spec}/index.js`;
  return null;
}

let rewritten = 0;
const unresolved = [];

for await (const file of walk(root)) {
  if (!/\.(js|d\.ts)$/.test(file)) continue;
  const declaration = file.endsWith(".d.ts");
  const before = await readFile(file, "utf8");
  const after = before.replace(SPECIFIER, (match, lead, quote, spec) => {
    if (/\.(js|mjs|cjs|json|css)$/.test(spec)) return match;
    const next = resolved(file, spec, declaration);
    if (next === null) {
      unresolved.push(`${file}: ${spec}`);
      return match;
    }
    rewritten += 1;
    return `${lead}${quote}${next}${quote}`;
  });
  if (after !== before) await writeFile(file, after);
}

if (unresolved.length > 0) {
  console.error(`specify-extensions: ${unresolved.length} specifier(s) did not resolve:`);
  for (const line of unresolved) console.error(`  ${line}`);
  process.exit(1);
}

const files = [];
for await (const f of walk(root)) if (/\.(js|d\.ts)$/.test(f)) files.push(f);
console.log(`specify-extensions: ${rewritten} specifier(s) across ${files.length} file(s)`);
