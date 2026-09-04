#!/usr/bin/env node
// scripts/check-rust-coverage.mjs
//
// Runs cargo-llvm-cov 0.9.0 against the focused adapter, indexed-library,
// database-policy, and index-coordinator suites, then enforces a branch threshold
// for each production scope.
// Fails immediately with a clear message if the exact tool version is not
// installed; never installs it automatically.

import { spawnSync } from "node:child_process";
import { readFileSync, rmSync } from "node:fs";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const REQUIRED_VERSION = "0.9.0";
const COVERAGE_TOOLCHAIN = "nightly-2025-09-18-x86_64-pc-windows-msvc";
const COVERAGE_TARGETS = [
  {
    label: "Adapter",
    branchThreshold: 80,
    includes: (filename) => /[\\/]src[\\/]adapters[\\/]/.test(filename),
  },
  {
    label: "Indexed-library contract",
    branchThreshold: 90,
    includes: (filename) => /[\\/]src[\\/]indexed_library\.rs$/.test(filename),
  },
  {
    label: "Database",
    branchThreshold: 90,
    includes: (filename) =>
      /[\\/]src[\\/]database[\\/](mod|bootstrap|repository)\.rs$/.test(filename) ||
      /[\\/]src[\\/]database[\\/]repository[\\/]read\.rs$/.test(filename),
  },
  {
    label: "Index coordinator",
    branchThreshold: 90,
    includes: (filename) => /[\\/]src[\\/]discovery[\\/]index\.rs$/.test(filename),
  },
];
const COVERAGE_RUNS = [
  { label: "Adapter", cargoArgs: ["--lib", "--", "adapters::"] },
  {
    label: "Indexed-library contract",
    cargoArgs: ["--lib", "--", "indexed_library::"],
  },
  { label: "Database", cargoArgs: ["--lib", "--", "database::"] },
  {
    label: "Index coordinator",
    cargoArgs: ["--lib", "--", "discovery::index_tests"],
  },
  {
    label: "Database startup",
    cargoArgs: [
      "--test",
      "database_plugin_startup",
      "--features",
      "startup-test",
    ],
  },
];
const IGNORE_FILENAME_REGEX = String.raw`src[\\/](adapters[\\/](tests|codex_tests)\.rs|database[\\/](tests|repository_tests|read_repository_tests|scan_repository_tests)\.rs|commands[\\/].*|discovery[\\/](tests|index_tests)\.rs|export[\\/].*|io[\\/].*|error\.rs|model\.rs|lib\.rs|main\.rs)`;

const __dirname = dirname(fileURLToPath(import.meta.url));
const tauriDir = resolve(__dirname, "..", "src-tauri");
const reportPath = resolve(tauriDir, "target", "domain-coverage.json");

const versionResult = spawnSync("cargo", ["llvm-cov", "--version"], {
  cwd: tauriDir,
  encoding: "utf-8",
});
if (versionResult.status !== 0) {
  console.error(
    `\nERROR: cargo-llvm-cov is not installed.\n` +
      `   Install exactly version ${REQUIRED_VERSION}:\n` +
      `   cargo install cargo-llvm-cov --version ${REQUIRED_VERSION} --locked\n`
  );
  process.exit(1);
}

const match = versionResult.stdout.trim().match(/cargo-llvm-cov\s+(\S+)/);
const installedVersion = match ? match[1] : null;
if (installedVersion !== REQUIRED_VERSION) {
  console.error(
    `\nERROR: cargo-llvm-cov version mismatch: found ${installedVersion}, require exactly ${REQUIRED_VERSION}.\n` +
      `   Install the exact version:\n` +
      `   cargo install cargo-llvm-cov --version ${REQUIRED_VERSION} --locked\n`
  );
  process.exit(1);
}

const toolchainResult = spawnSync(
  "rustup",
  ["run", COVERAGE_TOOLCHAIN, "rustc", "--version"],
  { encoding: "utf-8" }
);
if (toolchainResult.status !== 0) {
  console.error(
    `\nERROR: Rust coverage toolchain ${COVERAGE_TOOLCHAIN} is not installed.\n` +
      "   Install the exact toolchain:\n" +
      `   rustup toolchain install ${COVERAGE_TOOLCHAIN} ` +
      "--profile minimal --component llvm-tools-preview\n"
  );
  process.exit(1);
}

const cleanResult = spawnSync(
  "cargo",
  [
    `+${COVERAGE_TOOLCHAIN}`,
    "llvm-cov",
    "clean",
    "--manifest-path",
    resolve(tauriDir, "Cargo.toml"),
  ],
  { cwd: tauriDir, stdio: "inherit" }
);
if (cleanResult.status !== 0) {
  console.error("\nERROR: Existing coverage artifacts could not be cleaned.");
  process.exit(1);
}

rmSync(reportPath, { force: true });

for (const [index, run] of COVERAGE_RUNS.entries()) {
  const isLastRun = index === COVERAGE_RUNS.length - 1;
  const args = [
    `+${COVERAGE_TOOLCHAIN}`,
    "llvm-cov",
    "--branch",
    ...(isLastRun
      ? [
          "--no-clean",
          "--json",
          "--summary-only",
          "--output-path",
          reportPath,
          "--ignore-filename-regex",
          IGNORE_FILENAME_REGEX,
        ]
      : ["--no-report"]),
    "--manifest-path",
    resolve(tauriDir, "Cargo.toml"),
    ...run.cargoArgs,
  ];
  console.log(`Running: cargo ${args.join(" ")}`);
  const coverageResult = spawnSync("cargo", args, {
    cwd: tauriDir,
    stdio: "inherit",
  });
  if (coverageResult.status !== 0) {
    rmSync(reportPath, { force: true });
    console.error(`\nERROR: ${run.label} coverage run failed.`);
    process.exit(1);
  }
}

let files;
try {
  const report = JSON.parse(readFileSync(reportPath, "utf-8"));
  files = report?.data?.[0]?.files;
} catch (error) {
  console.error(`\nERROR: Coverage report could not be read: ${error.message}`);
  process.exit(1);
} finally {
  rmSync(reportPath, { force: true });
}

for (const target of COVERAGE_TARGETS) {
  const totals = files
    ?.filter((file) => target.includes(file.filename))
    .reduce(
      (sum, file) => ({
        covered: sum.covered + Number(file.summary?.branches?.covered),
        count: sum.count + Number(file.summary?.branches?.count),
      }),
      {covered: 0, count: 0},
    );
  const covered = totals?.covered;
  const count = totals?.count;
  if (
    !Number.isSafeInteger(covered) ||
    !Number.isSafeInteger(count) ||
    covered < 0 ||
    count <= 0 ||
    covered > count
  ) {
    console.error(`\nERROR: Coverage report did not contain valid ${target.label} branch totals.`);
    process.exit(1);
  }

  const branchPercent = (covered / count) * 100;
  if (branchPercent < target.branchThreshold) {
    console.error(
      `\nERROR: ${target.label} branch coverage is ${branchPercent.toFixed(2)}% ` +
        `(${covered}/${count}); require at least ${target.branchThreshold}%.`,
    );
    process.exit(1);
  }

  console.log(
    `\n${target.label} branch coverage is ${branchPercent.toFixed(2)}% ` +
      `(${covered}/${count}; threshold: ${target.branchThreshold}%).`,
  );
}
