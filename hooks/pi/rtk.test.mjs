import test from "node:test"
import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { chmodSync, copyFileSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { delimiter, join } from "node:path"
import plugin from "./rtk.ts"

const WINDOWS = process.platform === "win32"
const RTK = WINDOWS ? "rtk.exe" : "rtk"

// A fake rtk that reports a supported version and answers `rtk rewrite git status`
// with `answer`. On Windows it is a copy of node.exe: its own version passes the
// version check, and it runs the working folder's `rewrite` script (see
// REWRITE_SCRIPT), which prints the answer file beside the copy.
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

// Stands in for Pi: `exec` spawns the program it is given, as Pi does.
function fakePi() {
  const pi = { handlers: {}, ran: [] }
  pi.on = (name, handler) => (pi.handlers[name] = handler)
  pi.exec = async (command, args) => {
    pi.ran.push(command)
    const r = spawnSync(command, args, { encoding: "utf8" })
    return { code: r.status ?? 1, stdout: r.stdout ?? "", stderr: r.stderr ?? "", killed: false }
  }
  return pi
}

test("rtk runs only from an absolute PATH entry, never from the working folder or a relative entry", async () => {
  const root = mkdtempSync(join(tmpdir(), "rtk-pi-"))
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
      const unc = `\\\\localhost\\${real[0]}$${real.slice(2)}`
      cases.push([[unc], "rtk git status", join(unc, RTK)])
    }
    cases.push([[...decoys, real], "rtk git status", join(real, RTK)], [decoys, "git status"])
    process.chdir(work)

    // A bare "rtk", as the extension started it before, reaches a decoy with these entries.
    process.env.PATH = [...decoys, real].join(delimiter)
    const bare = spawnSync("rtk", ["rewrite", "git status"], { encoding: "utf8" })
    assert.match(bare.stdout ?? "", /^decoy/, "bare rtk")

    for (const [entries, expected, bin] of cases) {
      process.env.PATH = entries.join(delimiter)
      const pi = fakePi()
      await plugin(pi)
      const event = { toolName: "bash", input: { command: "git status" } }
      await pi.handlers.tool_call?.(event, {})
      const where = entries.join(delimiter)
      assert.equal(event.input.command, expected, where)
      assert.deepEqual(pi.ran, bin ? [bin, bin] : [], where)
    }
  } finally {
    process.chdir(savedCwd)
    process.env.PATH = savedPath
    rmSync(root, { recursive: true, force: true })
  }
})
