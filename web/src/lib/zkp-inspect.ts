import type { ProofPayload } from "./zkp-types";

type ValidationResult = { ok: true; payload: ProofPayload } | { ok: false; error: string; isInvalid: true };
export type ProofAssessment = { valid: false; status: "unverified" | "unsupported" | "unavailable"; reason: string };

const MAX_PROOF_CHARACTERS = 1_048_576;
const U64_MAX = 18_446_744_073_709_551_615n;

function record(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** Structural inspection only. Neither payload metadata nor server flags are trust anchors. */
export function validateProofJson(rawJson: string): ValidationResult {
  const invalid = (error: string): ValidationResult => ({ ok: false, error, isInvalid: true });
  if (rawJson.length > MAX_PROOF_CHARACTERS) return invalid("Proof exceeds the 1 MiB character limit.");
  let raw: unknown;
  try { raw = JSON.parse(rawJson); }
  catch { return invalid("Invalid JSON — could not parse proof data."); }
  if (!record(raw)) return invalid("Proof must be a JSON object.");
  const id = typeof raw.user_id === "number" && Number.isSafeInteger(raw.user_id)
    ? String(raw.user_id) : raw.user_id;
  if (typeof id !== "string" || !/^[1-9]\d*$/.test(id) || id.length > 20 || BigInt(id) > U64_MAX) {
    return invalid("user_id must be a canonical positive u64 string or a safe positive number.");
  }
  if (typeof raw.root_hash !== "string" || !/^(?:0x)?[0-9a-fA-F]{64}$/.test(raw.root_hash)) {
    return invalid("root_hash must be a 32-byte ASCII hexadecimal value.");
  }
  const version = raw.protocol_version === undefined ? 0 : raw.protocol_version;
  if (typeof version !== "number" || !Number.isSafeInteger(version) || version < 0) {
    return invalid("protocol_version must be a nonnegative integer.");
  }
  if (!record(raw.snark) || typeof raw.snark.scheme !== "string"
      || typeof raw.snark.proof !== "string" || !raw.snark.proof
      || typeof raw.snark.public_inputs !== "string" || !raw.snark.public_inputs) {
    return invalid("Proof bytes, public inputs and scheme are required; a verified flag is insufficient.");
  }
  return { ok: true, payload: {
    protocol_version: version,
    statement: typeof raw.statement === "string" ? raw.statement : "legacy_unknown",
    user_id: id,
    root_hash: raw.root_hash,
    ...(typeof raw.leaf_balance === "string" ? { leaf_balance: raw.leaf_balance } : {}),
    snark: { scheme: raw.snark.scheme, proof: raw.snark.proof, public_inputs: raw.snark.public_inputs },
  } };
}

/** PLAN-04 never claims local cryptographic verification; actual verification is a later gate. */
export async function runWasmVerification(parsed: ProofPayload): Promise<ProofAssessment> {
  if (parsed.protocol_version === 0) return {
    valid: false, status: "unverified", reason: "Legacy payload: view only; no independently verified statement.",
  };
  if (parsed.protocol_version !== 1 || parsed.statement !== "leaf_commitment") return {
    valid: false, status: "unsupported", reason: "Unsupported protocol or statement; verification was not performed.",
  };
  return { valid: false, status: "unavailable", reason:
    "Local SNARK verifier and pinned verification context are not available. Inclusion, aggregate and reserve coverage remain unverified." };
}

export function coverageStatus(_serverResult: unknown): "unverified" {
  // A server comparison, declared reserve amount or query override is not reserve evidence.
  return "unverified";
}
