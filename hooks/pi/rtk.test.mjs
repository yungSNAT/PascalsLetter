import test from "node:test"
import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { chmodSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { delimiter, join } from "node:path"
import plugin from "./rtk.ts"

// A fake rtk that reports a supported version and answers `rtk rewrite git status`
// with `answer`.
function fakeRtk(dir, answer) {
  mkdirSync(dir, { recursive: true })
  const bin = join(dir, "rtk")
  writeFileSync(
    bin,
    `#!/bin/sh\ncase "$1" in --version) echo "rtk 0.50.0" ;; rewrite) echo "${answer}" ;; esac\n`
  )
  chmodSync(bin, 0o755)
}

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

test(
  "rtk runs only from an absolute PATH entry, never from the working folder or a relative entry",
  { skip: process.platform === "win32" && "the fake rtk is a shell script" },
  async () => {
    const root = mkdtempSync(join(tmpdir(), "rtk-pi-"))
    const savedPath = process.env.PATH
    const savedCwd = process.cwd()
    try {
      const work = join(root, "work")
      const real = join(root, "real")
      fakeRtk(work, "decoy in the working folder")
      fakeRtk(join(work, "rel"), "decoy in a relative PATH entry")
      fakeRtk(real, "rtk git status")
      process.chdir(work)

      for (const [entries, expected] of [
        [["rel", "", ".", real], "rtk git status"],
        [["rel", "", "."], "git status"],
      ]) {
        process.env.PATH = entries.join(delimiter)
        const pi = fakePi()
        await plugin(pi)
        const event = { toolName: "bash", input: { command: "git status" } }
        await pi.handlers.tool_call?.(event, {})
        const where = entries.join(delimiter)
        assert.equal(event.input.command, expected, where)
        assert.deepEqual(pi.ran, expected === "git status" ? [] : [join(real, "rtk"), join(real, "rtk")], where)
      }
    } finally {
      process.chdir(savedCwd)
      process.env.PATH = savedPath
      rmSync(root, { recursive: true, force: true })
    }
  }
)
