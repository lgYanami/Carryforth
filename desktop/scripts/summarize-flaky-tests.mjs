import { appendFile, readFile, writeFile } from "node:fs/promises";

function collectFlakyTests(suite, out) {
  for (const spec of suite.specs ?? []) {
    for (const test of spec.tests ?? []) {
      if (test.status !== "flaky") continue;
      out.push({
        title: `${suite.file} › ${spec.title}`,
        project: test.projectName,
        attempts: test.results?.length ?? 0,
      });
    }
  }
  for (const child of suite.suites ?? []) {
    collectFlakyTests(child, out);
  }
}

function usage() {
  console.error(
    "Usage: node scripts/summarize-flaky-tests.mjs <report.json> <run-label> [--strict] [--output <path>]",
  );
}

const args = process.argv.slice(2);
const reportPath = args.shift();
const runLabel = args.shift();
let strict = false;
let outputPath;

while (args.length > 0) {
  const option = args.shift();
  if (option === "--strict") {
    strict = true;
  } else if (option === "--output") {
    outputPath = args.shift();
    if (!outputPath) {
      usage();
      process.exit(2);
    }
  } else {
    usage();
    process.exit(2);
  }
}

if (!reportPath || !runLabel) {
  usage();
  process.exit(2);
}

try {
  const raw = await readFile(reportPath, "utf8");
  if (raw.trim().length === 0) {
    throw new Error("Playwright JSON report is empty");
  }
  const report = JSON.parse(raw);
  if (
    report === null ||
    typeof report !== "object" ||
    !Array.isArray(report.suites)
  ) {
    throw new Error("Playwright JSON report does not contain a suites array");
  }

  const flaky = [];
  for (const suite of report.suites) {
    collectFlakyTests(suite, flaky);
  }

  const escapeCell = (value) => String(value).replaceAll("|", "\\|");
  let summary = `### Flaky tests — ${runLabel}\n\n`;
  if (flaky.length === 0) {
    summary += "No test passed only after a retry.\n";
  } else {
    const rows = flaky
      .map(
        (test) =>
          `| ${escapeCell(test.title)} | ${escapeCell(test.project)} | ${test.attempts} |`,
      )
      .join("\n");
    summary +=
      `${flaky.length} test(s) failed at least once before passing on retry:\n\n` +
      "| Test | Project | Attempts |\n| --- | --- | --- |\n" +
      `${rows}\n`;
  }

  console.log(summary);
  if (outputPath) {
    await writeFile(outputPath, `${summary}\n`, "utf8");
  }
  const summaryFile = process.env.GITHUB_STEP_SUMMARY;
  if (summaryFile) {
    await appendFile(summaryFile, `${summary}\n`);
  }
} catch (error) {
  const message = `Unable to summarize flaky tests: ${error.message}`;
  if (strict) {
    console.error(message);
    process.exit(1);
  }
  console.log(message);
}
