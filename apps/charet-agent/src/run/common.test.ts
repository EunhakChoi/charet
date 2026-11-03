import assert from "node:assert/strict";
import { test } from "node:test";
import { HIDDEN } from "../secrets.ts";
import { hide, keepSecret } from "./common.ts";

test("what a run reports does not hold its credentials", () => {
  keepSecret(["sk-ant-oat-token", "id.jwt.token"]);
  assert.equal(hide("401 for sk-ant-oat-token"), `401 for ${HIDDEN}`);
  assert.equal(hide("id.jwt.token and sk-ant-oat-token"), `${HIDDEN} and ${HIDDEN}`);
  assert.equal(hide("nothing here"), "nothing here");
  keepSecret([]);
  assert.equal(hide("sk-ant-oat-token"), "sk-ant-oat-token");
});
