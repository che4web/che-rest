import assert from "node:assert/strict";
import test from "node:test";
import { recordId, routeIdentity, selectedIdentity } from "../src/admin/templates/recordIdentity.ts";

const uuid = "1d997e81-5d63-4c7c-8a35-de054878cd76";
const model = (type) => ({ fields: [{ source: "id", type }] });

test("UUID edit routes retain identity and create routes have no identity", () => {
  assert.equal(routeIdentity(uuid, model("text")), uuid);
  assert.equal(routeIdentity(undefined, model("text")), undefined);
  assert.equal(routeIdentity("", model("text")), undefined);
  assert.equal(routeIdentity("42", model("integer")), 42);
  assert.equal(routeIdentity("0", model("integer")), 0);
});

test("relation selection preserves UUID strings, integer keys and clearing", () => {
  const options = [{ id: uuid }, { id: 42 }, { id: 0 }];
  assert.equal(recordId(uuid), uuid);
  assert.equal(recordId(42), 42);
  assert.equal(recordId(null), null);
  assert.equal(selectedIdentity(uuid, options), uuid);
  assert.equal(selectedIdentity("42", options), 42);
  assert.equal(selectedIdentity("0", options), 0);
  assert.equal(selectedIdentity("", options), null);
});
