import assert from "node:assert/strict";
import test from "node:test";
import { coverageStatus, runWasmVerification, validateProofJson } from "../src/lib/zkp-inspect.ts";

const proof = {
  protocol_version: 1, statement: "leaf_commitment", user_id: "1", root_hash: "ab".repeat(32),
  snark: { scheme: "groth16", proof: "fake-bytes", public_inputs: "fake-inputs", verified: true },
};

test("server verified flags and declared reserve amounts never establish trust", async () => {
  for (const verified of [true, false]) {
    const parsed = validateProofJson(JSON.stringify({ ...proof, snark: { ...proof.snark, verified } }));
    assert.equal(parsed.ok, true);
    if (!parsed.ok) throw new Error(parsed.error);
    assert.equal(parsed.payload.snark?.verified, undefined);
    const result = await runWasmVerification(parsed.payload);
    assert.equal(result.valid, false);
    assert.equal(result.status, "unavailable");
  }
  assert.equal(coverageStatus({ liabilities_leq_assets: true, verified: true, cold_wallet_assets: "999999999999999999" }), "unverified");
  assert.equal(validateProofJson(JSON.stringify({ verified: true })).ok, false);
});

test("legacy and unknown protocol or statement stay unverified or unsupported", async () => {
  for (const [version, statement, expected] of [
    [undefined, "leaf_commitment", "unverified"], [99, "leaf_commitment", "unsupported"],
    [1, "aggregate", "unsupported"], [1, "reserve_coverage", "unsupported"],
  ] as const) {
    const parsed = validateProofJson(JSON.stringify({ ...proof, protocol_version: version, statement }));
    if (!parsed.ok) throw new Error(parsed.error);
    const result = await runWasmVerification(parsed.payload);
    assert.equal(result.valid, false);
    assert.equal(result.status, expected);
  }
});

test("malformed shapes, unsafe IDs and oversized input are rejected without throwing", () => {
  for (const raw of ["null", "[]", "true", "1", "{", '"hello"', " ".repeat(1_048_577)]) {
    assert.equal(validateProofJson(raw).ok, false);
  }
  for (const id of [0, -1, 9007199254740992, "00", "01", "0", "18446744073709551616", "💥", "+1"]) {
    assert.equal(validateProofJson(JSON.stringify({ ...proof, user_id: id })).ok, false);
  }
  for (const hash of ["abc", "💥".repeat(32), "g".repeat(64)]) {
    assert.equal(validateProofJson(JSON.stringify({ ...proof, root_hash: hash })).ok, false);
  }
  assert.equal(validateProofJson(JSON.stringify({ ...proof, user_id: "18446744073709551615" })).ok, true);
});
