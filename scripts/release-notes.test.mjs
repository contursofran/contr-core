// node --test scripts/release-notes.test.mjs (CI runs it on each pull request).
import assert from "node:assert/strict";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { assetsIn, main, notesText, release } from "./release-notes.mjs";

const commit = (message) => ({ commit: { message } });

/** A fake GitHub API: the releases list, the compare answer and the commits list. Records each path. */
function fakeApi(releases, compare, commits = []) {
  const paths = [];
  const api = (p) => {
    paths.push(p);
    if (p.startsWith("repos/o/r/releases")) return releases;
    if (p.startsWith("repos/o/r/compare/")) return { commits: compare };
    if (p.startsWith("repos/o/r/commits")) return commits;
    throw new Error(`unexpected ${p}`);
  };
  return { api, paths };
}

const args = (version, more = []) => ["--version", version, "--sha", "ccc", "--date", "2026-11-17", "--repo", "o/r", ...more];

test("lists the commits after the newest published release before, newest first, between the version line and the build lines", () => {
  const releases = [
    { tag_name: "build-2026-11-16-fffffff", published_at: "2026-11-16T10:00:00Z" },
    { tag_name: "build-2026-11-16-aaaaaaa", published_at: "2026-11-16T12:00:00Z" },
    { tag_name: "build-2026-11-15-bbbbbbb", published_at: "2026-11-15T12:00:00Z" },
  ];
  const { api, paths } = fakeApi(releases, [commit("Older change (#1)\n\nbody"), commit("Newer change (#2)")]);
  const text = main(args("0.1.30"), {}, api);
  assert.deepEqual(paths, ["repos/o/r/releases?per_page=100", "repos/o/r/compare/build-2026-11-16-aaaaaaa...ccc"]);
  assert.equal(
    text,
    [
      "contr-core 0.1.30 from ccc.",
      "",
      "## Changes",
      "",
      "- Newer change (#2)",
      "- Older change (#1)",
      "",
      "Build of ccc (2026-11-17) on main.",
      "",
      "Install: unzip, then run `./install.sh` in the folder (`--dry-run` first to see what it does).",
      "",
    ].join("\n"),
  );
});

test("an unsigned build adds the quarantine lines", () => {
  const { api } = fakeApi([], [], []);
  assert.match(
    main(args("0.1.3", ["--unsigned"]), {}, api),
    /`--dry-run` first to see what it does\)\.\n\nThe macOS binaries are not signed \(no certificate in the repository secrets\)\.\nIf macOS blocks them, run `xattr -d com\.apple\.quarantine bin\/\* remote\/\*\/herdr-contr-feed` in the folder\.\n$/,
  );
  assert.equal(main(args("0.1.3"), {}, fakeApi([], [], []).api).includes("not signed"), false);
});

test("skips this release's own tag and releases that are not build-*", () => {
  const { api, paths } = fakeApi([{ tag_name: "build-2026-11-17-ccccccc" }, { tag_name: "v1" }, { tag_name: "build-2026-11-16-aaaaaaa" }], [commit("A (#3)")]);
  main(args("0.1.31", ["--tag", "build-2026-11-17-ccccccc"]), {}, api);
  assert.equal(paths[1], "repos/o/r/compare/build-2026-11-16-aaaaaaa...ccc");
});

test("with no release before, lists the last 20 commits as the API gives them, newest first", () => {
  const { api, paths } = fakeApi([], [], [commit("Third (#3)"), commit("Second (#2)"), commit("First")]);
  const text = main(args("0.1.1"), {}, api);
  assert.deepEqual(paths, ["repos/o/r/releases?per_page=100", "repos/o/r/commits?sha=ccc&per_page=20"]);
  assert.match(text, /## Changes\n\n- Third \(#3\)\n- Second \(#2\)\n- First\n\nBuild of ccc/);
});

test("with no commits there is no Changes part", () => {
  assert.equal(notesText("0.1.2", "ccc", "2026-11-17", [], false), "contr-core 0.1.2 from ccc.\n\nBuild of ccc (2026-11-17) on main.\n\nInstall: unzip, then run `./install.sh` in the folder (`--dry-run` first to see what it does).\n");
  const { api } = fakeApi([{ tag_name: "build-2026-11-16-aaaaaaa" }], []);
  assert.equal(main(args("0.1.2"), {}, api).includes("## Changes"), false);
});

test("a dry run uses version 0.0.0, today's date and the sha from SHA or GITHUB_SHA", () => {
  const { api } = fakeApi([{ tag_name: "build-2026-11-16-aaaaaaa" }], [commit("A (#3)")]);
  assert.match(main(["--dry-run"], { GITHUB_REPOSITORY: "o/r", GITHUB_SHA: "merge1" }, api), /^contr-core 0\.0\.0 from merge1\.\n/);
  assert.match(main(["--dry-run"], { GITHUB_REPOSITORY: "o/r", SHA: "head1", GITHUB_SHA: "merge1" }, api), /^contr-core 0\.0\.0 from head1\.\n/);
  const { notes } = release(["--dry-run"], { GITHUB_REPOSITORY: "o/r", SHA: "head1" }, api, new Date("2026-11-17T08:00:00Z"));
  assert.match(notes, /Build of head1 \(2026-11-17\) on main\./);
});

test("with no version, sha, date or repository and no dry run it throws the usage", () => {
  assert.throws(() => main(["--version", "0.1.1", "--sha", "ccc", "--date", "2026-11-17"], {}, () => []), /usage/);
  assert.throws(() => main(["--dry-run"], { SHA: "head1" }, () => []), /usage/);
  assert.throws(() => main(["--sha", "ccc", "--date", "2026-11-17"], {}, () => []), /usage/);
  assert.throws(() => main(["--version", "0.1.1", "--date", "2026-11-17"], {}, () => []), /usage/);
  assert.throws(() => main(["--version", "0.1.1", "--sha", "ccc"], {}, () => []), /usage/);
});

const notes = (v, changes = "") => `contr-core ${v} from x.\n\n${changes}Build of x (2026-11-01) on main.\n`;

test("release.json has this release first, then the newest 9 published releases with a version line, by time", () => {
  const releases = [
    { tag_name: "build-2026-11-17-ccccccc", published_at: "2026-11-17T09:00:00Z", body: notes("0.1.30") },
    { tag_name: "build-2026-11-16-ddddddd", draft: true, published_at: "2026-11-16T13:00:00Z", body: notes("0.1.29") },
    { tag_name: "build-2026-11-16-9999999", published_at: "2026-11-16T14:00:00Z", body: "Build of 9999999 (2026-11-16) on main." },
    { tag_name: "build-2026-11-16-8888888", published_at: "2026-11-16T15:00:00Z", body: "contr 0.1.99 from 8888888." },
    { tag_name: "build-2026-11-15-bbbbbbb", published_at: "2026-11-15T12:00:00Z", body: notes("0.1.27") },
    { tag_name: "build-2026-11-16-aaaaaaa", published_at: "2026-11-16T12:00:00Z", body: notes("0.1.28", "## Changes\n\n- Old change (#5)\n\n") },
    ...Array.from({ length: 12 }, (_, i) => ({ tag_name: `build-2026-10-${10 + i}-eeeeeee`, published_at: `2026-10-${10 + i}T00:00:00Z`, body: notes(`0.0.${i}`) })),
  ];
  const { api } = fakeApi(releases, [commit("New change (#6)")]);
  const zip = "herdr-contr-build-2026-11-17-ccccccc.zip";
  const files = { assets: [zip], sha256: { [zip]: "a".repeat(64) } };
  const { file } = release(args("0.1.30", ["--tag", "build-2026-11-17-ccccccc"]), {}, api, new Date("2026-11-17T09:30:00Z"), files);
  assert.deepEqual({ ...file, releases: file.releases.slice(0, 3) }, {
    schema: 1,
    version: "0.1.30",
    tag: "build-2026-11-17-ccccccc",
    sha: "ccc",
    date: "2026-11-17T09:30:00.000Z",
    assets: [zip],
    sha256: { [zip]: "a".repeat(64) },
    releases: [
      { version: "0.1.30", tag: "build-2026-11-17-ccccccc", date: "2026-11-17", changes: ["New change (#6)"] },
      { version: "0.1.28", tag: "build-2026-11-16-aaaaaaa", date: "2026-11-16", changes: ["Old change (#5)"] },
      { version: "0.1.27", tag: "build-2026-11-15-bbbbbbb", date: "2026-11-15", changes: [] },
    ],
  });
  assert.equal(file.releases.length, 10);
  assert.equal(file.releases[9].version, "0.0.5");
});

test("a dry run gives release.json with no assets and only this release when there is no release before", () => {
  const { api } = fakeApi([], [], [commit("A (#3)")]);
  const { file } = release(["--dry-run"], { GITHUB_REPOSITORY: "o/r", SHA: "head1" }, api, new Date("2026-11-17T00:00:00Z"));
  assert.deepEqual(file.releases, [{ version: "0.0.0", tag: "build-dry-run", date: "2026-11-17", changes: ["A (#3)"] }]);
  assert.deepEqual(file.assets, []);
  assert.deepEqual(file.sha256, {});
});

test("assetsIn names the files of the folder, sorted, with the SHA-256 of each", () => {
  const dir = mkdtempSync(join(tmpdir(), "release-notes-"));
  try {
    writeFileSync(join(dir, "b.zip"), "abc");
    writeFileSync(join(dir, "a.txt"), "");
    assert.deepEqual(assetsIn(dir), {
      assets: ["a.txt", "b.zip"],
      sha256: {
        "a.txt": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        "b.zip": "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
      },
    });
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
