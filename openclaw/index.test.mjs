import test from "node:test"
import assert from "node:assert/strict"
import { delimiter } from "node:path"
import { withDecoys } from "../hooks/testing/rtk-decoys.mjs"
import register from "./index.ts"

test("rtk runs only from an absolute PATH entry, never from the working folder or a relative entry", () =>
  withDecoys("rtk-openclaw-", ({ decoys, real, unc }) => {
    const cases = [
      [[...decoys, real], "rtk git status"],
      [decoys, undefined],
    ]
    if (unc) cases.push([[unc], "rtk git status"])
    for (const [entries, expected] of cases) {
      process.env.PATH = entries.join(delimiter)
      const handlers = {}
      register({ config: {}, on: (name, handler) => (handlers[name] = handler) })
      const result = handlers.before_tool_call?.({ toolName: "exec", params: { command: "git status" } })
      assert.equal(result?.params.command, expected, entries.join(delimiter))
    }
  }))
