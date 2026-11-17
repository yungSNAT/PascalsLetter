import test from "node:test"
import assert from "node:assert/strict"
import { chmodSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { delimiter, join } from "node:path"
import register from "./index.ts"

// A fake rtk that answers `rtk rewrite git status` with `answer`.
function fakeRtk(dir, answer) {
  mkdirSync(dir, { recursive: true })
  const bin = join(dir, "rtk")
  writeFileSync(bin, `#!/bin/sh\n[ "$1" = rewrite ] && echo "${answer}"\n`)
  chmodSync(bin, 0o755)
}

test(
  "rtk runs only from an absolute PATH entry, never from the working folder or a relative entry",
  { skip: process.platform === "win32" && "the fake rtk is a shell script" },
  () => {
    const root = mkdtempSync(join(tmpdir(), "rtk-openclaw-"))
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
        [["rel", "", "."], undefined],
      ]) {
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
  }
)
