import test from "node:test"
import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { chmodSync, copyFileSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { delimiter, join } from "node:path"
import register from "./index.ts"

const WINDOWS = process.platform === "win32"
const RTK = WINDOWS ? "rtk.exe" : "rtk"

// A fake rtk that answers `rtk rewrite git status` with `answer`. On Windows it is
// a copy of node.exe, which runs the working folder's `rewrite` script (see
// REWRITE_SCRIPT); that prints the answer file beside the copy.
function fakeRtk(dir, answer) {
  mkdirSync(dir, { recursive: true })
  if (WINDOWS) {
    copyFileSync(process.execPath, join(dir, RTK))
    writeFileSync(join(dir, "answer"), answer)
    return
  }
  const bin = join(dir, RTK)
  writeFileSync(bin, `#!/bin/sh\n[ "$1" = rewrite ] && echo "${answer}"\n`)
  chmodSync(bin, 0o755)
}

const REWRITE_SCRIPT = `const { readFileSync } = require("node:fs")
const { dirname, join } = require("node:path")
console.log(readFileSync(join(dirname(process.execPath), "answer"), "utf8"))
`

test("rtk runs only from an absolute PATH entry, never from the working folder or a relative entry", () => {
  const root = mkdtempSync(join(tmpdir(), "rtk-openclaw-"))
  const savedPath = process.env.PATH
  const savedCwd = process.cwd()
  try {
    const work = join(root, "work")
    const real = join(root, "real")
    fakeRtk(work, "decoy in the working folder")
    fakeRtk(join(work, "rel"), "decoy in a relative PATH entry")
    fakeRtk(real, "rtk git status")
    const decoys = ["rel", "", "."]
    const cases = []
    if (WINDOWS) {
      writeFileSync(join(work, "rewrite"), REWRITE_SCRIPT)
      // Drive-relative (the drive's working folder, here `work`) and quoted entries.
      decoys.push(`${work.slice(0, 2)}rel`, `"${real}"`)
      // The same folder through a UNC path.
      cases.push([[`\\\\localhost\\${real[0]}$${real.slice(2)}`], "rtk git status"])
    }
    cases.push([[...decoys, real], "rtk git status"], [decoys, undefined])
    process.chdir(work)

    // A bare "rtk", as the hook started it before, reaches a decoy with these entries.
    process.env.PATH = [...decoys, real].join(delimiter)
    const bare = spawnSync("rtk", ["rewrite", "git status"], { encoding: "utf8" })
    assert.match(bare.stdout ?? "", /^decoy/, "bare rtk")

    for (const [entries, expected] of cases) {
      process.env.PATH = entries.join(delimiter)
      const handlers = {}
      register({ config: {}, on: (name, handler) => (handlers[name] = handler) })
      const result = handlers.before_tool_call?.({ toolName: "exec", params: { command: "git status" } })
      assert.equal(result?.params.command, expected, entries.join(delimiter))
    }
  } finally {
    process.chdir(savedCwd)
    process.env.PATH = savedPath
    rmSync(root, { recursive: true, force: true })
  }
})
