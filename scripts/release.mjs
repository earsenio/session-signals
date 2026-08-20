// Two-step release driver: prepare the version bump on a branch, then tag the
// merge commit on main.
//
// Why two steps. The `main` branch ruleset requires a pull request and four
// green checks. The old flow (`npm version` → commit + tag → push straight to
// main) satisfied neither: it relied on the repository-role bypass, so every
// release commit reached main unreviewed and unchecked. Splitting the bump from
// the tag lets the version commit go through CI like any other change.
//
// Because main squash-merges, the tag cannot be created before the merge — the
// squash produces a new commit, and a tag made on the branch would point at one
// that never lands on main. Hence `tag` runs after, against main's real HEAD.
//
//   npm run release:prepare -- patch    # branch + bump + push, then open a PR
//   npm run release:tag                 # after the PR merges, from main
//
// See docs/VERSIONING.md.

import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");

function run(cmd, args, opts = {}) {
  return execFileSync(cmd, args, { cwd: root, encoding: "utf8", ...opts }).trim();
}

function git(...args) {
  return run("git", args);
}

function die(msg) {
  console.error(`release: ${msg}`);
  process.exit(1);
}

function version() {
  return JSON.parse(readFileSync(resolve(root, "package.json"), "utf8")).version;
}

/** Refuse to act on anything but a clean, current main. */
function assertCleanMain() {
  if (git("status", "--porcelain")) {
    die("working tree is not clean — commit or stash first.");
  }
  const branch = git("rev-parse", "--abbrev-ref", "HEAD");
  if (branch !== "main") {
    die(`expected to be on main, but on "${branch}".`);
  }
  git("fetch", "origin", "main", "--quiet");
  if (git("rev-parse", "HEAD") !== git("rev-parse", "origin/main")) {
    die("main is not in sync with origin/main — pull (or push) first.");
  }
}

function bump(current, kind) {
  const [major, minor, patch] = current.split(".").map(Number);
  if ([major, minor, patch].some(Number.isNaN)) {
    die(`current version "${current}" is not plain X.Y.Z.`);
  }
  switch (kind) {
    case "major":
      return `${major + 1}.0.0`;
    case "minor":
      return `${major}.${minor + 1}.0`;
    case "patch":
      return `${major}.${minor}.${patch + 1}`;
    default:
      die(`unknown bump "${kind}" — expected patch, minor, or major.`);
  }
}

function prepare(kind) {
  assertCleanMain();
  const next = bump(version(), kind);
  const tag = `v${next}`;
  const branch = `release/${tag}`;

  if (git("tag", "--list", tag)) die(`tag ${tag} already exists.`);
  console.log(`release: ${version()} → ${next} on ${branch}`);

  git("checkout", "-b", branch);
  // --no-git-tag-version: bump the files only. The npm `version` lifecycle hook
  // still runs, which is what propagates the number into Cargo.toml/Cargo.lock.
  run("npm", ["version", next, "--no-git-tag-version"]);
  git("add", "package.json", "package-lock.json", "src-tauri/Cargo.toml", "src-tauri/Cargo.lock");
  git("commit", "-m", `chore(release): ${tag}`);
  git("push", "-u", "origin", branch);

  console.log(
    [
      "",
      `Pushed ${branch}. Next:`,
      `  1. Stamp CHANGELOG.md: move [Unreleased] entries under "## [${next}] - <date>"`,
      `     and add the compare link, then amend or add a commit on this branch.`,
      `  2. gh pr create --base main --head ${branch} --title "chore(release): ${tag}"`,
      "  3. Merge once the four checks pass.",
      "  4. git checkout main && git pull && npm run release:tag",
    ].join("\n"),
  );
}

function tag() {
  assertCleanMain();
  const v = version();
  const name = `v${v}`;
  if (git("tag", "--list", name)) die(`tag ${name} already exists locally.`);
  const remote = git("ls-remote", "--tags", "origin", name);
  if (remote) die(`tag ${name} already exists on origin.`);

  // main must actually carry the bump — guards against tagging before the
  // release PR has merged.
  const head = git("show", "-s", "--format=%s", "HEAD");
  console.log(`release: tagging ${name} at ${git("rev-parse", "--short", "HEAD")} (${head})`);

  git("tag", "-a", name, "-m", `Session Signals ${name}`);
  git("push", "origin", name);
  console.log(
    `\nPushed ${name}. The release workflow will build a DRAFT release — review the\n` +
      "installers (and check notifications fire from the bundle) before publishing.",
  );
}

const [, , cmd, arg] = process.argv;
if (cmd === "prepare") prepare(arg);
else if (cmd === "tag") tag();
else die("usage: release.mjs prepare <patch|minor|major> | release.mjs tag");
