import test from "node:test"
import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { delimiter, join } from "node:path"
import { RTK, withDecoys } from "../testing/rtk-decoys.mjs"
import plugin from "./rtk.ts"

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

test("rtk runs only from an absolute PATH entry, never from the working folder or a relative entry", () =>
  withDecoys("rtk-pi-", async ({ decoys, real, unc }) => {
    const cases = [
      [[...decoys, real], "rtk git status", join(real, RTK)],
      [decoys, "git status"],
    ]
    if (unc) cases.push([[unc], "rtk git status", join(unc, RTK)])
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
  }))

test("an rtk older than 0.23.0 disables the extension; a newer one, build metadata and all, does not", () =>
  withDecoys("rtk-pi-version-", async ({ real }) => {
    process.env.PATH = real
    for (const [stdout, enabled] of [
      ["rtk 0.22.9\n", false],
      ["rtk 0.23.0\n", true],
      ["rtk 1.2.0+fork.2\n", true],
    ]) {
      const pi = fakePi()
      pi.exec = async () => ({ code: 0, stdout, stderr: "", killed: false })
      await plugin(pi)
      assert.equal(typeof pi.handlers.tool_call === "function", enabled, stdout)
    }
  }))
