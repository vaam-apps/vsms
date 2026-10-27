// Runs the suites under test/ and fails when too few tests ran.
//
// `node --test` exits 0 when it finds nothing to run, and which files it finds
// depends on the Node version: before 22.18 it does not look for `.ts` files
// unless type stripping is on, so on 22.11-22.17 a bare `node --test` reports
// success over an empty suite. This names the files, turns type stripping on
// (the flag is accepted, and ignored, where it is already the default), and
// reads the number of tests back from a TAP report written beside the readable
// one, so an empty run is a failure and not a pass.
import { spawnSync } from "node:child_process";
import { mkdtempSync, readdirSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

// The suites hold 93 tests. The floor is below that so a few can be deleted on
// purpose, and far above what an undiscovered suite would give (0).
const FLOOR = 70;

const files = readdirSync("test")
  .filter((name) => name.endsWith(".test.ts"))
  .sort()
  .map((name) => join("test", name));
if (files.length === 0) {
  console.error("run-tests: FAIL — no *.test.ts files under test/");
  process.exit(1);
}

const dir = mkdtempSync(join(tmpdir(), "vsms-node-tests-"));
const tap = join(dir, "report.tap");
try {
  const run = spawnSync(
    process.execPath,
    [
      "--experimental-strip-types",
      "--test",
      "--test-reporter=spec",
      "--test-reporter-destination=stdout",
      "--test-reporter=tap",
      `--test-reporter-destination=${tap}`,
      ...files,
    ],
    { stdio: "inherit" },
  );
  let ran = 0;
  try {
    ran = Number(/^# tests (\d+)$/m.exec(readFileSync(tap, "utf8"))?.[1] ?? 0);
  } catch {
    // No report at all: counted as zero tests below.
  }
  if (ran < FLOOR) {
    console.error(`run-tests: FAIL — ${ran} tests ran, at least ${FLOOR} are expected.`);
    process.exit(1);
  }
  process.exit(run.status ?? 1);
} finally {
  rmSync(dir, { recursive: true, force: true });
}
