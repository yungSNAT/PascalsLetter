// Shared by the OpenClaw and Pi hook tests: a working folder that holds decoy rtks,
// reachable through a bare "rtk" or a non-absolute PATH entry, and a real one.
import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { chmodSync, copyFileSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { delimiter, join } from "node:path"

const WINDOWS = process.platform === "win32"
export const RTK = WINDOWS ? "rtk.exe" : "rtk"

// A fake rtk that reports a supported version and answers `rtk rewrite git status`
// with `answer`. On Windows it is a copy of node.exe: its own version passes Pi's
// version check, and it runs the working folder's `rewrite` script, which prints
// the answer file beside the copy.
function fakeRtk(dir, answer) {
  mkdirSync(dir, { recursive: true })
  if (WINDOWS) {
    copyFileSync(process.execPath, join(dir, RTK))
    writeFileSync(join(dir, "answer"), answer)
    return
  }
  const bin = join(dir, RTK)
  writeFileSync(
    bin,
    `#!/bin/sh\ncase "$1" in --version) echo "rtk 0.50.0" ;; rewrite) echo "${answer}" ;; esac\n`
  )
  chmodSync(bin, 0o755)
}

const REWRITE_SCRIPT = `const { readFileSync } = require("node:fs")
const { dirname, join } = require("node:path")
console.log(readFileSync(join(dirname(process.execPath), "answer"), "utf8"))
`

// Runs `check({ decoys, real, unc })` from inside the working folder, then restores
// the working folder and PATH. `decoys` are PATH entries that must never be used;
// `real` is the absolute folder of the real rtk, which answers "rtk git status";
// `unc` is that folder as a UNC path, on Windows only.
export async function withDecoys(prefix, check) {
  const root = mkdtempSync(join(tmpdir(), prefix))
  const savedPath = process.env.PATH
  const savedCwd = process.cwd()
  try {
    const work = join(root, "work")
    const real = join(root, "real")
    fakeRtk(work, "decoy in the working folder")
    fakeRtk(join(work, "rel"), "decoy in a relative PATH entry")
    fakeRtk(real, "rtk git status")
    const decoys = ["rel", "", "."]
    let unc
    if (WINDOWS) {
      writeFileSync(join(work, "rewrite"), REWRITE_SCRIPT)
      // Drive-relative (the drive's working folder, here `work`) and quoted entries.
      decoys.push(`${work.slice(0, 2)}rel`, `"${real}"`)
      unc = `\\\\localhost\\${real[0]}$${real.slice(2)}`
      // The first start through a share can outlast the hooks' 2-second timeout,
      // so start it once here, with no timeout; this also shows it runs.
      assert.equal(spawnSync(join(unc, RTK), ["--version"]).status, 0, unc)
    }
    process.chdir(work)

    // A bare "rtk", as the hooks started it before, reaches a decoy with these entries.
    process.env.PATH = [...decoys, real].join(delimiter)
    const bare = spawnSync("rtk", ["rewrite", "git status"], { encoding: "utf8" })
    assert.match(bare.stdout ?? "", /^decoy/, "bare rtk")

    await check({ decoys, real, unc })
  } finally {
    process.chdir(savedCwd)
    process.env.PATH = savedPath
    rmSync(root, { recursive: true, force: true })
  }
}
